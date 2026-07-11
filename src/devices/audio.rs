use alsa::device_name::HintIter;
use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use rodio::{OutputStream, Sink, Source};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub struct AudioStreamHandle {
    alsa_capture_thread: Option<thread::JoinHandle<()>>,
    stop_capture: Arc<AtomicBool>,
    _output_stream_guard: OutputStream,
    _output_stream_handle: rodio::OutputStreamHandle,
    sink: Sink,
}

impl Drop for AudioStreamHandle {
    fn drop(&mut self) {
        self.stop_capture.store(true, Ordering::Relaxed);
        self.sink.stop();
        if let Some(handle) = self.alsa_capture_thread.take() {
            if let Err(e) = handle.join() {
                tracing::warn!("ALSA capture thread join failed: {:?}", e);
            }
        }
    }
}

struct LiveSource {
    consumer: Consumer<f32>,
    channels: u16,
    sample_rate: u32,
    samples_played: Arc<AtomicU64>,
    underruns: Arc<AtomicU64>,
    peak_amplitude_shared: Arc<AtomicU64>,
    audio_latency_ms: Arc<AtomicU64>,
    capture_delay_ms: Arc<AtomicU64>,
    local_buf: Vec<f32>,
    local_idx: usize,
    valid_len: usize,
}

impl Iterator for LiveSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.local_idx >= self.valid_len {
            let mut available = self.consumer.slots();

            // Safety threshold: ~5ms of audio at the current sample rate
            let safety_threshold =
                (self.sample_rate as f32 * 0.005 * self.channels as f32) as usize;

            // If the buffer is below threshold, wait to see if more data arrives.
            // This syncs the consumer thread with the producer and prevents "crackling"
            // where we rapidly toggle between a few samples and silence.
            if available < safety_threshold {
                let mut retries = 0;
                // Wait up to 20ms total (20 * 1ms)
                while retries < 20 && available < safety_threshold {
                    thread::sleep(Duration::from_millis(1));
                    available = self.consumer.slots();
                    retries += 1;
                }
            }

            let queued_samples = available;
            let safe_queued = (queued_samples / self.channels as usize) * self.channels as usize;

            let ring_buffer_ms =
                (safe_queued as f64 / self.channels as f64 / self.sample_rate as f64 * 1000.0)
                    as u64;

            let capture_ms = self.capture_delay_ms.load(Ordering::Relaxed);

            // Heuristic for playback latency: Rodio/CPAL usually buffers 2-3 periods.
            // We'll estimate it as ~20ms as a safe baseline for modern Linux audio stacks (Pulse/PipeWire).
            let playback_ms = 20;

            let total_latency = capture_ms + ring_buffer_ms + playback_ms;
            self.audio_latency_ms
                .store(total_latency, Ordering::Relaxed);

            // Keep latency bounded (clock drift compensation).
            // We use the ring buffer occupancy for this, as it's what we can control.
            if ring_buffer_ms > 100 {
                let target_samples =
                    (self.sample_rate as f64 * self.channels as f64 * 0.08) as usize; // 80ms
                if safe_queued > target_samples {
                    let to_drop = safe_queued - target_samples;
                    let drop_frames = (to_drop / self.channels as usize) * self.channels as usize;
                    if let Ok(chunk) = self.consumer.read_chunk(drop_frames) {
                        chunk.commit_all();
                        tracing::debug!(
                            "Audio latency {}ms: Clock drift compensated by dropping {} samples",
                            total_latency,
                            drop_frames
                        );
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
                let underruns = self.underruns.fetch_add(1, Ordering::Relaxed);
                if underruns.is_multiple_of(100) {
                    tracing::warn!(
                        "Audio underrun! Ringbuffer exhausted (count: {}).",
                        underruns
                    );
                }

                // On underrun, insert a larger chunk of silence (e.g. 10ms)
                // to give the producer a chance to catch up and prevent rapid crackling.
                let silence_samples =
                    (self.sample_rate as f32 * 0.010 * self.channels as f32) as usize;
                let silence_to_insert = std::cmp::min(silence_samples, self.local_buf.len());
                let safe_silence =
                    (silence_to_insert / self.channels as usize) * self.channels as usize;

                self.local_buf[..safe_silence].fill(0.0);
                self.valid_len = safe_silence;
                self.local_idx = 0;
            }
        }

        let sample = self.local_buf[self.local_idx];
        self.local_idx += 1;

        let amp = sample.abs();
        let amp_bits = (amp * 1000.0) as u64;
        self.peak_amplitude_shared
            .fetch_max(amp_bits, Ordering::Relaxed);

        let count = self.samples_played.fetch_add(1, Ordering::Relaxed);
        if count.is_multiple_of(48000) {
            let underrun_count = self.underruns.swap(0, Ordering::Relaxed);
            tracing::debug!(
                "Audio output heartbeats: {} samples played ({} underruns)",
                count,
                underrun_count
            );
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
    s as f32 / 32768.0
}

extern "C" fn alsa_error_handler(
    _file: *const libc::c_char,
    _line: libc::c_int,
    _function: *const libc::c_char,
    _err: libc::c_int,
    _fmt: *const libc::c_char,
) {
}

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
    buffer_size: u32,
    sample_rate: u32,
    sample_format: String,
) -> Result<AudioStreamHandle> {
    // Shared ring buffer for capture-to-playback bridge.
    // Increased to ~1.0s of audio to provide complete buffer safety. We manage ideal latency from the consumer side.
    let ring_size = (sample_rate as f32 * 1.0 * 2.0) as usize;
    let (producer, consumer) = RingBuffer::<f32>::new(ring_size);

    let capture_delay_ms = Arc::new(AtomicU64::new(0));
    let stop_capture = Arc::new(AtomicBool::new(false));

    let (alsa_thread, input_channels, input_sample_rate) = start_alsa_capture(
        source_name,
        producer,
        peak_amplitude_shared.clone(),
        capture_delay_ms.clone(),
        stop_capture.clone(),
        buffer_size,
        sample_rate,
        sample_format,
    )?;

    // Playback via Rodio
    let (output_stream_guard, stream_handle) = match initialize_rodio_playback() {
        Ok(stream) => stream,
        Err(e) => {
            stop_capture.store(true, Ordering::Relaxed);
            if let Some(handle) = alsa_thread {
                let _ = handle.join();
            }
            return Err(e);
        }
    };

    let sink = match Sink::try_new(&stream_handle).context("Failed to create rodio sink") {
        Ok(sink) => sink,
        Err(e) => {
            stop_capture.store(true, Ordering::Relaxed);
            if let Some(handle) = alsa_thread {
                let _ = handle.join();
            }
            return Err(e);
        }
    };
    sink.set_volume(1.0);

    let live_source = LiveSource {
        consumer,
        channels: input_channels,
        sample_rate: input_sample_rate,
        samples_played: Arc::new(AtomicU64::new(0)),
        underruns: Arc::new(AtomicU64::new(0)),
        peak_amplitude_shared,
        audio_latency_ms,
        capture_delay_ms,
        local_buf: vec![0.0; buffer_size as usize * input_channels as usize],
        local_idx: 0,
        valid_len: 0,
    };
    sink.append(live_source);

    Ok(AudioStreamHandle {
        alsa_capture_thread: alsa_thread,
        stop_capture,
        _output_stream_guard: output_stream_guard,
        _output_stream_handle: stream_handle,
        sink,
    })
}

#[allow(clippy::too_many_arguments)]
fn start_alsa_capture(
    source_name: &str,
    mut producer: Producer<f32>,
    peak_amplitude_shared: Arc<AtomicU64>,
    capture_delay_ms: Arc<AtomicU64>,
    stop_capture: Arc<AtomicBool>,
    buffer_size: u32,
    sample_rate: u32,
    sample_format: String,
) -> Result<(Option<thread::JoinHandle<()>>, u16, u32)> {
    use alsa::pcm::{Access, Format, HwParams, PCM};
    use alsa::Direction;

    let pcm_format = match sample_format.as_str() {
        "S32LE" => Format::S32LE,
        "F32LE" => Format::FloatLE,
        _ => Format::S16LE,
    };

    // 1. Probe the device briefly to get its supported rate and channels.
    let (rate, channels) = {
        let pcm_probe = PCM::new(source_name, Direction::Capture, false)
            .map_err(|e| anyhow!("Failed to probe ALSA device {}: {}", source_name, e))?;
        let hwp_probe = HwParams::any(&pcm_probe)?;
        hwp_probe.set_format(pcm_format).with_context(|| {
            format!(
                "ALSA device does not support sample format {}",
                sample_format
            )
        })?;
        let r = hwp_probe.set_rate_near(sample_rate, alsa::ValueOr::Nearest)?;
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

        let hwp = match HwParams::any(&pcm) {
            Ok(hwp) => hwp,
            Err(e) => {
                tracing::error!("Failed to create ALSA hardware params: {}", e);
                return;
            }
        };
        if let Err(e) = hwp.set_access(Access::RWInterleaved) {
            tracing::error!("Failed to set ALSA access mode: {}", e);
            return;
        }
        if let Err(e) = hwp.set_format(pcm_format) {
            tracing::error!("Failed to set ALSA sample format: {}", e);
            return;
        }
        if let Err(e) = hwp.set_rate_near(rate, alsa::ValueOr::Nearest) {
            tracing::error!("Failed to set ALSA sample rate: {}", e);
            return;
        }
        if let Err(e) = hwp.set_channels_near(channels as u32) {
            tracing::error!("Failed to set ALSA channels: {}", e);
            return;
        }
        let _ = hwp.set_period_size_near(buffer_size as alsa::pcm::Frames, alsa::ValueOr::Nearest);
        if let Err(e) = pcm.hw_params(&hwp) {
            tracing::error!("Failed to apply ALSA hardware params: {}", e);
            return;
        }

        match sample_format.as_str() {
            "S32LE" => {
                let io = match pcm.io_i32() {
                    Ok(io) => io,
                    Err(e) => {
                        tracing::error!("Failed to create ALSA i32 reader: {}", e);
                        return;
                    }
                };
                let mut buf = vec![0i32; buffer_size as usize * channels as usize];
                while !stop_capture.load(Ordering::Relaxed) {
                    if let Ok(delay_frames) = pcm.delay() {
                        let delay_ms = (delay_frames as f64 / rate as f64 * 1000.0) as u64;
                        capture_delay_ms.store(delay_ms, Ordering::Relaxed);
                    }
                    match io.readi(&mut buf) {
                        Ok(frames) => {
                            let sample_count = frames * channels as usize;
                            let mut local_max = 0.0f32;
                            let mut f32_buf = Vec::with_capacity(sample_count);
                            for &sample in buf.iter().take(sample_count) {
                                let f = (sample as f64 / 2147483648.0) as f32;
                                local_max = local_max.max(f.abs());
                                f32_buf.push(f);
                            }
                            push_to_ring(
                                &mut producer,
                                &f32_buf,
                                sample_count,
                                channels as usize,
                                local_max,
                                &peak_amplitude_shared,
                                &samples_captured,
                            );
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
            }
            "F32LE" => {
                let io = match pcm.io_f32() {
                    Ok(io) => io,
                    Err(e) => {
                        tracing::error!("Failed to create ALSA f32 reader: {}", e);
                        return;
                    }
                };
                let mut buf = vec![0.0f32; buffer_size as usize * channels as usize];
                while !stop_capture.load(Ordering::Relaxed) {
                    if let Ok(delay_frames) = pcm.delay() {
                        let delay_ms = (delay_frames as f64 / rate as f64 * 1000.0) as u64;
                        capture_delay_ms.store(delay_ms, Ordering::Relaxed);
                    }
                    match io.readi(&mut buf) {
                        Ok(frames) => {
                            let sample_count = frames * channels as usize;
                            let mut local_max = 0.0f32;
                            for &sample in buf.iter().take(sample_count) {
                                local_max = local_max.max(sample.abs());
                            }
                            push_to_ring(
                                &mut producer,
                                &buf,
                                sample_count,
                                channels as usize,
                                local_max,
                                &peak_amplitude_shared,
                                &samples_captured,
                            );
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
            }
            _ => {
                // S16LE
                let io = match pcm.io_i16() {
                    Ok(io) => io,
                    Err(e) => {
                        tracing::error!("Failed to create ALSA i16 reader: {}", e);
                        return;
                    }
                };
                let mut buf = vec![0i16; buffer_size as usize * channels as usize];
                while !stop_capture.load(Ordering::Relaxed) {
                    if let Ok(delay_frames) = pcm.delay() {
                        let delay_ms = (delay_frames as f64 / rate as f64 * 1000.0) as u64;
                        capture_delay_ms.store(delay_ms, Ordering::Relaxed);
                    }
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
                            push_to_ring(
                                &mut producer,
                                &f32_buf,
                                sample_count,
                                channels as usize,
                                local_max,
                                &peak_amplitude_shared,
                                &samples_captured,
                            );
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
            }
        }
        tracing::debug!("ALSA capture thread stopped.");
    });

    Ok((Some(handle), channels, rate))
}

fn push_to_ring(
    producer: &mut Producer<f32>,
    f32_buf: &[f32],
    sample_count: usize,
    channels: usize,
    local_max: f32,
    peak_amplitude_shared: &Arc<AtomicU64>,
    samples_captured: &Arc<AtomicU64>,
) {
    let free = producer.slots();
    let safe_free = (free / channels) * channels;
    let to_push = std::cmp::min(safe_free, sample_count);
    let safe_to_push = (to_push / channels) * channels;

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
        OutputStream::try_from_device(&device).context("Failed to open prioritized output stream")
    } else {
        tracing::info!("Using system default output stream");
        OutputStream::try_default().context("Failed to open default output stream")
    }
}
