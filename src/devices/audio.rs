use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use alsa::device_name::HintIter;
use rtrb::{Consumer, Producer, RingBuffer};
use rodio::{OutputStream, Sink, Source};
use std::time::Duration;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

pub struct AudioStreamHandle {
    _alsa_capture_thread: Option<thread::JoinHandle<()>>,
    _output_stream_guard: OutputStream,
    _output_stream_handle: rodio::OutputStreamHandle,
    _sink: Sink,
}

struct LiveSource {
    consumer: Consumer<f32>,
    channels: u16,
    sample_rate: u32,
    samples_played: Arc<AtomicU64>,
    underruns: Arc<AtomicU64>,
    peak_amplitude_shared: Arc<AtomicU64>,
    audio_latency_ms: Arc<AtomicU64>,
    pause_counter: usize,
    local_buf: Vec<f32>,
    local_idx: usize,
    valid_len: usize,
}

impl Iterator for LiveSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pause_counter > 0 {
            self.pause_counter -= 1;
            return Some(0.0);
        }

        if self.local_idx >= self.valid_len {
            let queued_samples = self.consumer.slots();
            let safe_queued = (queued_samples / self.channels as usize) * self.channels as usize;

            let latency = (safe_queued as f64 / self.channels as f64 / self.sample_rate as f64 * 1000.0) as u64;
            self.audio_latency_ms.store(latency, Ordering::Relaxed);

            // Keep latency bounded (clock drift compensation). 
            // If latency climbs above 100ms, drop down to 80ms to catch up smoothly without giant clicks.
            if latency > 100 {
                let target_samples = (self.sample_rate as f64 * self.channels as f64 * 0.08) as usize; // 80ms
                if safe_queued > target_samples {
                    let to_drop = safe_queued - target_samples;
                    let drop_frames = (to_drop / self.channels as usize) * self.channels as usize;
                    if let Ok(chunk) = self.consumer.read_chunk(drop_frames) {
                        chunk.commit_all();
                        tracing::debug!("Audio latency {}ms: Clock drift compensated by dropping {} samples", latency, drop_frames);
                    }
                }
            }

            let available = self.consumer.slots();
            let safe_to_read = (available / self.channels as usize) * self.channels as usize;
            let to_read = std::cmp::min(safe_to_read, self.local_buf.len());

            if to_read > 0 {
                // Ensure we only ever read a full even frame (no partial channels)
                if let Ok(chunk) = self.consumer.read_chunk(to_read) {
                    let (s1, s2) = chunk.as_slices();
                    let l1 = s1.len();
                    self.local_buf[..l1].copy_from_slice(s1);
                    if !s2.is_empty() {
                        self.local_buf[l1..to_read].copy_from_slice(s2);
                    }
                    chunk.commit_all();
                    self.valid_len = to_read;
                    self.local_idx = 0;
                }
            } else {
                self.underruns.fetch_add(1, Ordering::Relaxed);
                tracing::warn!("Audio underrun! Ringbuffer exhausted.");
                
                // If we underrun, instead of pausing for 40ms, we just return a single 0.0 sample 
                // and try again next iteration. This is more transparent to the player.
                return Some(0.0);
            }
        }

        let sample = self.local_buf[self.local_idx];
        self.local_idx += 1;

        let amp = sample.abs();
        let amp_bits = (amp * 1000.0) as u64;
        self.peak_amplitude_shared.fetch_max(amp_bits, Ordering::Relaxed);

        let count = self.samples_played.fetch_add(1, Ordering::Relaxed);
        if count.is_multiple_of(48000) {
             let underrun_count = self.underruns.swap(0, Ordering::Relaxed);
             tracing::debug!("Audio output heartbeats: {} samples played ({} underruns)", count, underrun_count);
        }
        Some(sample)
    }
}

impl Source for LiveSource {
    fn current_frame_len(&self) -> Option<usize> {
        None 
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None 
    }
}

#[inline(always)]
fn i16_to_f32(s: i16) -> f32 {
    s as f32 / i16::MAX as f32
}

extern "C" fn alsa_error_handler(
    _file: *const libc::c_char,
    _line: libc::c_int,
    _function: *const libc::c_char,
    _err: libc::c_int,
    _fmt: *const libc::c_char,
) {}

pub fn find_audio_devices() -> Result<Vec<(String, String)>> {
    unsafe {
        use libc::{c_int, c_void};
        extern "C" {
            fn snd_lib_error_set_handler(handler: *const c_void) -> c_int;
        }
        snd_lib_error_set_handler(alsa_error_handler as *const c_void);
    }

    let mut sources = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    use alsa::pcm::PCM;
    use alsa::Direction;

    if let Ok(hints) = HintIter::new_str(None, "pcm") {
        for hint in hints {
           if let (Some(name), Some(desc)) = (hint.name, hint.desc) {
               if name == "null" || name.contains("oss") {
                   continue;
               }
               
               if seen_names.contains(&name) {
                   continue;
               }
               
               // Probe for capture capability
               let is_input = match PCM::new(&name, Direction::Capture, true) {
                   Ok(_) => true,
                   Err(e) if e.to_string().contains("busy") => true,
                   _ => false,
               };

               if is_input {
                   let display_name = desc.replace("\n", " - ");
                   sources.push((display_name, name.clone()));
                   seen_names.insert(name);
               }
            }
        }
    }

    sources.sort_by(|(a, _), (b, _)| a.cmp(b));
    Ok(sources)
}

pub fn start_audio_stream(
    source_name: &str, 
    peak_amplitude_shared: Arc<AtomicU64>,
    audio_latency_ms: Arc<AtomicU64>,
) -> Result<AudioStreamHandle> {
    // Shared ring buffer for capture-to-playback bridge.
    // Increased to ~1.0s of audio to provide complete buffer safety. We manage ideal latency from the consumer side.
    let ring_size = (48000.0 * 1.0 * 2.0) as usize; 
    let (producer, consumer) = RingBuffer::<f32>::new(ring_size);

    let (alsa_thread, input_channels, input_sample_rate) = start_alsa_capture(source_name, producer, peak_amplitude_shared.clone())?;

    // Playback via Rodio
    let (output_stream_guard, stream_handle) = initialize_rodio_playback()?;
    
    let sink = Sink::try_new(&stream_handle).context("Failed to create rodio sink")?;
    sink.set_volume(1.0);

    let live_source = LiveSource {
        consumer,
        channels: input_channels,
        sample_rate: input_sample_rate,
        samples_played: Arc::new(AtomicU64::new(0)),
        underruns: Arc::new(AtomicU64::new(0)),
        peak_amplitude_shared,
        audio_latency_ms,
        pause_counter: 0,
        local_buf: vec![0.0; 1024],
        local_idx: 0,
        valid_len: 0,
    };
    sink.append(live_source);

    Ok(AudioStreamHandle {
        _alsa_capture_thread: alsa_thread,
        _output_stream_guard: output_stream_guard,
        _output_stream_handle: stream_handle,
        _sink: sink,
    })
}

fn start_alsa_capture(
    source_name: &str,
    mut producer: Producer<f32>,
    peak_amplitude_shared: Arc<AtomicU64>,
) -> Result<(Option<thread::JoinHandle<()>>, u16, u32)> {
    use alsa::pcm::{PCM, HwParams, Access, Format};
    use alsa::Direction;

    // 1. Probe the device briefly to get its supported rate and channels.
    let (rate, channels) = {
        let pcm_probe = PCM::new(source_name, Direction::Capture, false)
            .map_err(|e| anyhow!("Failed to probe ALSA device {}: {}", source_name, e))?;
        let hwp_probe = HwParams::any(&pcm_probe)?;
        let r = hwp_probe.set_rate_near(48000, alsa::ValueOr::Nearest)?;
        let c = hwp_probe.set_channels_near(2)? as u16;
        (r, c)
    };

    let samples_captured = Arc::new(AtomicU64::new(0));
    let source_name_owned = source_name.to_string();

    let handle = thread::spawn(move || {
        let pcm = match PCM::new(&source_name_owned, Direction::Capture, false) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("Failed to open ALSA device in thread: {}", e);
                return;
            }
        };

        let hwp = HwParams::any(&pcm).unwrap();
        hwp.set_access(Access::RWInterleaved).unwrap();
        hwp.set_format(Format::S16LE).unwrap();
        hwp.set_rate_near(rate, alsa::ValueOr::Nearest).unwrap();
        hwp.set_channels_near(channels as u32).unwrap();
        
        pcm.hw_params(&hwp).unwrap();

        let io = pcm.io_i16().unwrap();
        let mut buf = vec![0i16; 1024 * channels as usize];
        
        loop {
            match io.readi(&mut buf) {
                Ok(frames) => {
                    let sample_count = frames * channels as usize;
                    let mut local_max = 0.0f32;
                    let mut f32_buf = Vec::with_capacity(sample_count);
                    
                    for &sample in buf.iter().take(sample_count) {
                        let f = i16_to_f32(sample);
                        local_max = local_max.max(f.abs());
                        f32_buf.push(f);
                    }
                    
                    // We must NEVER drop individual samples, only full frames (channels).
                    // Otherwise L/R channels swap and create deafening interference.
                    let free = producer.slots();
                    let safe_free = (free / channels as usize) * channels as usize;
                    let to_push = std::cmp::min(safe_free, sample_count);
                    let safe_to_push = (to_push / channels as usize) * channels as usize;
                    
                    if safe_to_push > 0 {
                        if let Ok(mut chunk) = producer.write_chunk(safe_to_push) {
                            let (slice1, slice2) = chunk.as_mut_slices();
                            let split_idx = slice1.len();
                            slice1.copy_from_slice(&f32_buf[..split_idx]);
                            if !slice2.is_empty() {
                                slice2.copy_from_slice(&f32_buf[split_idx..safe_to_push]);
                            }
                            chunk.commit_all();
                        }
                    }
                    
                    let peak_bits = (local_max * 1000.0) as u64;
                    peak_amplitude_shared.fetch_max(peak_bits, Ordering::Relaxed);
                    let _ = samples_captured.fetch_add(sample_count as u64, Ordering::Relaxed);
                }
                Err(e) => {
                    let err_str = e.to_string();
                    if err_str.contains("Broken pipe") || err_str.contains("EPIPE") {
                        let _ = pcm.prepare();
                    } else {
                        tracing::error!("ALSA read error: {}", e);
                        break;
                    }
                }
            }
        }
    });

    Ok((Some(handle), channels, rate))
}



fn initialize_rodio_playback() -> Result<(OutputStream, rodio::OutputStreamHandle)> {
    // On Linux, the "default" host is ALSA, but for output we want to hit the sound server (Pulse/PipeWire).
    // If we can find a Host for PulseAudio or if the default host has a "pulse"/"pipewire" device, we prefer it.
    let host = cpal::default_host();
    let mut target_device = None;
    
    if let Ok(devices) = host.output_devices() {
        for device in devices {
            if let Ok(name) = device.name() {
                let n = name.to_lowercase();
                if n.contains("pulse") {
                    tracing::info!("Found prioritized output device: {}", name);
                    target_device = Some(device);
                    break;
                }
            }
        }
    }

    if let Some(device) = target_device {
        OutputStream::try_from_device(&device)
            .context("Failed to open prioritized output stream")
    } else {
        tracing::info!("Using system default output stream");
        OutputStream::try_default()
            .context("Failed to open default output stream")
    }
}
