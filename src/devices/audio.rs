use alsa::device_name::HintIter;
use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use rodio::{OutputStream, Sink, Source};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const AUDIO_UNDERRUN_SILENCE: Duration = Duration::from_millis(1);
const ALSA_CAPTURE_POLL_TIMEOUT_MS: u32 = 10;
const ALSA_RECONNECT_DELAY: Duration = Duration::from_millis(50);
const ALSA_STALL_TIMEOUT: Duration = Duration::from_secs(2);

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
            // This iterator runs directly in CPAL's real-time output callback. It must never
            // wait for the capture producer: doing so makes an underrun take longer than the
            // audio it produces and can cause an unrecoverable starvation spiral.
            let queued_samples = self.consumer.slots();
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
                    }
                }
            }

            let available = self.consumer.slots();
            let safe_to_read = (available / self.channels as usize) * self.channels as usize;
            let to_read = std::cmp::min(safe_to_read, self.local_buf.len());

            let mut refilled = false;
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
                    refilled = true;
                }
            }

            if !refilled {
                // Use a short chunk so newly captured audio is noticed quickly, while avoiding
                // a ring-buffer check for every individual output sample.
                let silence_samples = (self.sample_rate as f64
                    * AUDIO_UNDERRUN_SILENCE.as_secs_f64()
                    * self.channels as f64) as usize;
                let silence_to_insert = std::cmp::min(silence_samples, self.local_buf.len());
                let safe_silence = ((silence_to_insert / self.channels as usize)
                    * self.channels as usize)
                    .max(self.channels as usize);

                self.local_buf[..safe_silence].fill(0.0);
                self.valid_len = safe_silence;
                self.local_idx = 0;
            }
        }

        let sample = self.local_buf[self.local_idx];
        self.local_idx += 1;
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
        audio_latency_ms,
        capture_delay_ms,
        local_buf: vec![
            0.0;
            (buffer_size as usize * input_channels as usize)
                .max(input_channels as usize)
        ],
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
    use alsa::pcm::{Format, HwParams, PCM};
    use alsa::Direction;

    let pcm_format = match sample_format.as_str() {
        "S32LE" => Format::S32LE,
        "F32LE" => Format::FloatLE,
        _ => Format::S16LE,
    };

    // 1. Probe the device briefly to get its supported rate and channels.
    let (rate, channels) = {
        let pcm_probe = PCM::new(source_name, Direction::Capture, true)
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
        while !stop_capture.load(Ordering::Relaxed) {
            let result = run_alsa_capture_session(
                &source_name_owned,
                &mut producer,
                &peak_amplitude_shared,
                &capture_delay_ms,
                &stop_capture,
                buffer_size,
                rate,
                channels,
                pcm_format,
                &sample_format,
                &samples_captured,
            );

            if stop_capture.load(Ordering::Relaxed) {
                break;
            }

            if let Err(error) = result {
                tracing::warn!("ALSA capture stopped ({}); reopening the device", error);
            }
            sleep_until_reconnect(&stop_capture);
        }
        tracing::debug!("ALSA capture thread stopped.");
    });

    Ok((Some(handle), channels, rate))
}

#[allow(clippy::too_many_arguments)]
fn run_alsa_capture_session(
    source_name: &str,
    producer: &mut Producer<f32>,
    peak_amplitude_shared: &Arc<AtomicU64>,
    capture_delay_ms: &Arc<AtomicU64>,
    stop_capture: &Arc<AtomicBool>,
    buffer_size: u32,
    rate: u32,
    channels: u16,
    pcm_format: alsa::pcm::Format,
    sample_format: &str,
    samples_captured: &Arc<AtomicU64>,
) -> Result<()> {
    use alsa::pcm::{Access, HwParams, PCM};
    use alsa::Direction;

    let pcm = PCM::new(source_name, Direction::Capture, true)
        .with_context(|| format!("Failed to open ALSA device {source_name}"))?;
    let hwp = HwParams::any(&pcm)?;
    hwp.set_access(Access::RWInterleaved)?;
    hwp.set_format(pcm_format)?;
    hwp.set_rate_near(rate, alsa::ValueOr::Nearest)?;
    hwp.set_channels_near(channels as u32)?;
    let _ = hwp.set_period_size_near(buffer_size as alsa::pcm::Frames, alsa::ValueOr::Nearest);
    pcm.hw_params(&hwp)?;
    drop(hwp);

    match sample_format {
        "S32LE" => run_alsa_capture_loop(
            &pcm,
            pcm.io_i32()?,
            producer,
            peak_amplitude_shared,
            capture_delay_ms,
            stop_capture,
            buffer_size,
            rate,
            channels,
            samples_captured,
            |sample| (sample as f64 / 2_147_483_648.0) as f32,
        ),
        "F32LE" => run_alsa_capture_loop(
            &pcm,
            pcm.io_f32()?,
            producer,
            peak_amplitude_shared,
            capture_delay_ms,
            stop_capture,
            buffer_size,
            rate,
            channels,
            samples_captured,
            |sample| sample,
        ),
        _ => run_alsa_capture_loop(
            &pcm,
            pcm.io_i16()?,
            producer,
            peak_amplitude_shared,
            capture_delay_ms,
            stop_capture,
            buffer_size,
            rate,
            channels,
            samples_captured,
            i16_to_f32,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_alsa_capture_loop<S, F>(
    pcm: &alsa::pcm::PCM,
    io: alsa::pcm::IO<'_, S>,
    producer: &mut Producer<f32>,
    peak_amplitude_shared: &Arc<AtomicU64>,
    capture_delay_ms: &Arc<AtomicU64>,
    stop_capture: &Arc<AtomicBool>,
    buffer_size: u32,
    rate: u32,
    channels: u16,
    samples_captured: &Arc<AtomicU64>,
    convert: F,
) -> Result<()>
where
    S: Copy + Default,
    F: Fn(S) -> f32,
{
    let buffer_samples = buffer_size as usize * channels as usize;
    let mut input_buf = vec![S::default(); buffer_samples];
    let mut converted_buf = vec![0.0f32; buffer_samples];
    let mut last_capture_progress = Instant::now();

    while !stop_capture.load(Ordering::Relaxed) {
        if let Ok(delay_frames) = pcm.delay() {
            let delay_ms = (delay_frames.max(0) as f64 / rate as f64 * 1000.0) as u64;
            capture_delay_ms.store(delay_ms, Ordering::Relaxed);
        }

        match io.readi(&mut input_buf) {
            Ok(frames) => {
                let sample_count = frames * channels as usize;
                if sample_count == 0 {
                    continue;
                }
                last_capture_progress = Instant::now();
                let mut local_max = 0.0f32;
                for (output, input) in converted_buf
                    .iter_mut()
                    .zip(input_buf.iter())
                    .take(sample_count)
                {
                    let sample = convert(*input);
                    local_max = local_max.max(sample.abs());
                    *output = sample;
                }
                push_to_ring(
                    producer,
                    &converted_buf,
                    sample_count,
                    channels as usize,
                    local_max,
                    peak_amplitude_shared,
                    samples_captured,
                );
            }
            Err(error) if error.errno() == alsa::nix::errno::Errno::EAGAIN => {
                if last_capture_progress.elapsed() >= ALSA_STALL_TIMEOUT {
                    return Err(anyhow!(
                        "ALSA capture produced no samples for {:?}",
                        ALSA_STALL_TIMEOUT
                    ));
                }
                match pcm.wait(Some(ALSA_CAPTURE_POLL_TIMEOUT_MS)) {
                    Ok(_) => {}
                    Err(wait_error) => recover_alsa_capture(pcm, wait_error)?,
                }
            }
            Err(error) => recover_alsa_capture(pcm, error)?,
        }
    }

    Ok(())
}

fn recover_alsa_capture(pcm: &alsa::pcm::PCM, error: alsa::Error) -> Result<()> {
    if error.errno() == alsa::nix::errno::Errno::EAGAIN {
        return Ok(());
    }

    pcm.try_recover(error, true)
        .with_context(|| format!("Could not recover from ALSA capture error: {error}"))
}

fn sleep_until_reconnect(stop_capture: &AtomicBool) {
    let retry_count = (ALSA_RECONNECT_DELAY.as_millis() / 10).max(1);
    for _ in 0..retry_count {
        if stop_capture.load(Ordering::Relaxed) {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn underrun_uses_short_silence_and_accepts_new_audio_without_waiting() {
        let (mut producer, consumer) = RingBuffer::<f32>::new(256);
        let mut source = LiveSource {
            consumer,
            channels: 2,
            sample_rate: 48_000,
            audio_latency_ms: Arc::new(AtomicU64::new(0)),
            capture_delay_ms: Arc::new(AtomicU64::new(0)),
            local_buf: vec![0.0; 256],
            local_idx: 0,
            valid_len: 0,
        };

        assert_eq!(source.next(), Some(0.0));
        assert_eq!(source.valid_len, 96, "underrun recovery should cover 1 ms");

        producer.push(0.25).unwrap();
        producer.push(-0.25).unwrap();
        for _ in 1..source.valid_len {
            assert_eq!(source.next(), Some(0.0));
        }

        assert_eq!(source.next(), Some(0.25));
        assert_eq!(source.next(), Some(-0.25));
    }
}
