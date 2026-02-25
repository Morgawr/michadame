use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use alsa::device_name::HintIter;
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;
use rodio::{OutputStream, Sink, Source};
use std::time::Duration;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

pub struct AudioStreamHandle {
    #[cfg(target_os = "linux")]
    _alsa_capture_thread: Option<thread::JoinHandle<()>>,
    #[cfg(not(target_os = "linux"))]
    _input_stream: cpal::Stream,
    
    _output_stream_guard: OutputStream,
    _output_stream_handle: rodio::OutputStreamHandle,
    _sink: Sink,
}

struct LiveSource<C: Consumer<Item = f32>> {
    consumer: C,
    channels: u16,
    sample_rate: u32,
    samples_played: Arc<AtomicU64>,
    underruns: Arc<AtomicU64>,
    peak_amplitude_shared: Arc<AtomicU64>,
}

impl<C: Consumer<Item = f32>> Iterator for LiveSource<C> {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let sample_opt = self.consumer.try_pop();
        if sample_opt.is_none() {
            self.underruns.fetch_add(1, Ordering::Relaxed);
        }
        let sample = sample_opt.unwrap_or(0.0);
        
        let amp = sample.abs();
        let amp_bits = (amp * 1000.0) as u64;
        self.peak_amplitude_shared.fetch_max(amp_bits, Ordering::Relaxed);

        let count = self.samples_played.fetch_add(1, Ordering::Relaxed);
        if count % 48000 == 0 {
             let underrun_count = self.underruns.swap(0, Ordering::Relaxed);
             tracing::debug!("Audio output heartbeats: {} samples played ({} underruns)", count, underrun_count);
        }
        Some(sample)
    }
}

impl<C: Consumer<Item = f32>> Source for LiveSource<C> {
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

#[cfg(target_os = "linux")]
extern "C" fn alsa_error_handler(
    _file: *const libc::c_char,
    _line: libc::c_int,
    _function: *const libc::c_char,
    _err: libc::c_int,
    _fmt: *const libc::c_char,
) {}

pub fn find_audio_devices() -> Result<Vec<(String, String)>> {
    #[cfg(target_os = "linux")]
    {
        unsafe {
            use libc::{c_int, c_void};
            extern "C" {
                fn snd_lib_error_set_handler(handler: *const c_void) -> c_int;
            }
            snd_lib_error_set_handler(alsa_error_handler as *const c_void);
        }
    }

    let mut sources = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    #[cfg(target_os = "linux")]
    {
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
    }

    #[cfg(not(target_os = "linux"))]
    {
        use cpal::traits::{HostTrait, DeviceTrait};
        let host = cpal::default_host();
        if let Ok(devices) = host.input_devices() {
            for device in devices {
                if let Ok(name) = device.name() {
                    sources.push((name.clone(), name));
                }
            }
        }
    }

    sources.sort_by(|(a, _), (b, _)| a.cmp(b));
    Ok(sources)
}

pub fn start_audio_stream(source_name: &str, peak_amplitude_shared: Arc<AtomicU64>) -> Result<AudioStreamHandle> {
    // Shared ring buffer for capture-to-playback bridge.
    // Increased to ~250ms of audio (48k * 0.25s * 2 channels) to provide jitter stability.
    let ring_size = (48000.0 * 0.25 * 2.0) as usize; 
    let ring = HeapRb::<f32>::new(ring_size);
    let (mut producer, consumer) = ring.split();

    #[cfg(target_os = "linux")]
    let (alsa_thread, input_channels, input_sample_rate) = {
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
        let peak_amplitude = Arc::new(AtomicU64::new(0));
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
                        for i in 0..sample_count {
                            let f = i16_to_f32(buf[i]);
                            local_max = local_max.max(f.abs());
                            let _ = producer.try_push(f);
                        }
                        let peak_bits = (local_max * 1000.0) as u64;
                        peak_amplitude.fetch_max(peak_bits, Ordering::Relaxed);
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

        (Some(handle), channels, rate)
    };

    #[cfg(not(target_os = "linux"))]
    let (input_stream, input_channels, input_sample_rate) = {
        use cpal::traits::{HostTrait, DeviceTrait};
        let host = cpal::default_host();
        let input_device = host.input_devices()?
            .find(|d| d.name().unwrap_or_default() == source_name)
            .ok_or_else(|| anyhow!("Input device not found"))?;
        let config = input_device.default_input_config()?;
        let channels = config.channels();
        let rate = config.sample_rate().0;
        
        let err_fn = |err| tracing::error!("CPAL input error: {}", err);
        let stream = input_device.build_input_stream(
            &config.into(),
            move |data: &[f32], _: &_| {
                let _ = producer.push_slice(data);
            },
            err_fn,
            None,
        )?;
        stream.play()?;
        (stream, channels, rate)
    };

    // Playback via Rodio
    // On Linux, the "default" host is ALSA, but for output we want to hit the sound server (Pulse/PipeWire).
    // If we can find a Host for PulseAudio or if the default host has a "pulse"/"pipewire" device, we prefer it.
    let (output_stream_guard, stream_handle) = {
        let host = cpal::default_host();
        let mut target_device = None;
        
        #[cfg(target_os = "linux")]
        {
            if let Ok(devices) = host.output_devices() {
                for device in devices {
                    if let Ok(name) = device.name() {
                        let n = name.to_lowercase();
                        if n.contains("pipewire") || n.contains("pulse") {
                            tracing::info!("Found prioritized output device: {}", name);
                            target_device = Some(device);
                            break;
                        }
                    }
                }
            }
        }

        if let Some(device) = target_device {
            OutputStream::try_from_device(&device)
                .context("Failed to open prioritized output stream")?
        } else {
            tracing::info!("Using system default output stream");
            OutputStream::try_default()
                .context("Failed to open default output stream")?
        }
    };
    
    let sink = Sink::try_new(&stream_handle).context("Failed to create rodio sink")?;
    sink.set_volume(1.0);

    let live_source = LiveSource {
        consumer,
        channels: input_channels,
        sample_rate: input_sample_rate,
        samples_played: Arc::new(AtomicU64::new(0)),
        underruns: Arc::new(AtomicU64::new(0)),
        peak_amplitude_shared,
    };
    sink.append(live_source);

    Ok(AudioStreamHandle {
        #[cfg(target_os = "linux")]
        _alsa_capture_thread: alsa_thread,
        #[cfg(not(target_os = "linux"))]
        _input_stream: input_stream,
        
        _output_stream_guard: output_stream_guard,
        _output_stream_handle: stream_handle,
        _sink: sink,
    })
}
