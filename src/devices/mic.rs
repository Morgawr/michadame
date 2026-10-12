use anyhow::{Context, Result};
use rodio::{OutputStream, Sink, Source};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::MicConfig;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateState {
    Closed,
    Opening,
    Open,
    Holding,
    Closing,
}

#[derive(Clone, Debug)]
pub struct NoiseGateFilter {
    state: GateState,
    gain: f32,
    hold_countdown: usize,
    sample_rate: f32,
}

impl NoiseGateFilter {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            state: GateState::Closed,
            gain: 0.0,
            hold_countdown: 0,
            sample_rate,
        }
    }

    #[inline]
    pub fn process_sample(
        &mut self,
        sample: f32,
        close_threshold_db: f32,
        open_threshold_db: f32,
        attack_time_ms: f32,
        hold_time_ms: f32,
        release_time_ms: f32,
    ) -> f32 {
        let level = sample.abs();
        let level_db = if level > 1e-6 {
            20.0 * level.log10()
        } else {
            -120.0
        };

        // Attack and release rates per sample
        let attack_samples = (attack_time_ms * 0.001 * self.sample_rate).max(0.0);
        let attack_step = if attack_samples > 0.0 {
            1.0 / attack_samples
        } else {
            1.0
        };

        let release_samples = (release_time_ms * 0.001 * self.sample_rate).max(0.0);
        let release_step = if release_samples > 0.0 {
            1.0 / release_samples
        } else {
            1.0
        };

        let hold_total_samples = (hold_time_ms * 0.001 * self.sample_rate).max(0.0) as usize;

        // Gate state machine with hysteresis
        if level_db >= open_threshold_db {
            // Signal above open threshold -> open or opening
            if self.gain >= 1.0 {
                self.state = GateState::Open;
                self.gain = 1.0;
            } else {
                self.state = GateState::Opening;
                self.gain = (self.gain + attack_step).min(1.0);
                if self.gain >= 1.0 {
                    self.state = GateState::Open;
                }
            }
            self.hold_countdown = hold_total_samples;
        } else if self.state == GateState::Open || self.state == GateState::Holding {
            if level_db >= close_threshold_db {
                // Signal is between close and open threshold while already open: keep open
                self.state = GateState::Open;
                self.gain = 1.0;
                self.hold_countdown = hold_total_samples;
            } else {
                // Signal dropped below close threshold: enter hold state
                self.state = GateState::Holding;
                if self.hold_countdown > 0 {
                    self.hold_countdown -= 1;
                    self.gain = 1.0;
                } else {
                    // Hold expired -> start closing
                    self.state = GateState::Closing;
                    self.gain = (self.gain - release_step).max(0.0);
                    if self.gain <= 0.0 {
                        self.state = GateState::Closed;
                    }
                }
            }
        } else if self.state == GateState::Closing {
            self.gain = (self.gain - release_step).max(0.0);
            if self.gain <= 0.0 {
                self.state = GateState::Closed;
            }
        } else if self.state == GateState::Opening {
            self.gain = (self.gain + attack_step).min(1.0);
            if self.gain >= 1.0 {
                self.state = GateState::Open;
            }
        } else {
            // Closed
            self.gain = 0.0;
        }

        sample * self.gain
    }
}

#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub struct GainFilter {}

#[cfg(test)]
impl GainFilter {
    #[inline]
    pub fn process_sample(sample: f32, gain_db: f32) -> f32 {
        let linear = 10.0f32.powf(gain_db / 20.0);
        sample * linear
    }
}

#[derive(Clone, Debug)]
pub struct CompressorFilter {
    env: f32,
    sample_rate: f32,
}

impl CompressorFilter {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            env: 0.0,
            sample_rate,
        }
    }

    #[inline]
    pub fn process_sample(
        &mut self,
        sample: f32,
        ratio: f32,
        threshold_db: f32,
        attack_time_ms: f32,
        release_time_ms: f32,
        output_gain_db: f32,
    ) -> f32 {
        let level = sample.abs();
        let attack_sec = (attack_time_ms.max(0.1) * 0.001).max(1.0 / self.sample_rate);
        let release_sec = (release_time_ms.max(1.0) * 0.001).max(1.0 / self.sample_rate);

        let attack_coeff = (-1.0 / (attack_sec * self.sample_rate)).exp();
        let release_coeff = (-1.0 / (release_sec * self.sample_rate)).exp();

        if level > self.env {
            self.env = attack_coeff * self.env + (1.0 - attack_coeff) * level;
        } else {
            self.env = release_coeff * self.env + (1.0 - release_coeff) * level;
        }

        let env_safe = self.env.max(1e-6);
        let env_db = 20.0 * env_safe.log10();

        let gain_reduction_db = if env_db > threshold_db {
            let ratio_clamped = ratio.max(1.0);
            let overshoot = env_db - threshold_db;
            let compressed = threshold_db + (overshoot / ratio_clamped);
            compressed - env_db // negative dB
        } else {
            0.0
        };

        let total_gain_db = gain_reduction_db + output_gain_db;
        let gain_linear = 10.0f32.powf(total_gain_db / 20.0);

        sample * gain_linear
    }
}

pub struct MicDspPipeline {
    gate: NoiseGateFilter,
    compressor: CompressorFilter,
}

impl MicDspPipeline {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            gate: NoiseGateFilter::new(sample_rate),
            compressor: CompressorFilter::new(sample_rate),
        }
    }

    /// Process interleaved audio samples through the pipeline in order:
    /// Raw Input -> Noise Gate -> Gain -> Compressor -> Master Volume
    pub fn process_interleaved(&mut self, samples: &mut [f32], channels: usize, config: &MicConfig) {
        if samples.is_empty() {
            return;
        }

        let num_ch = channels.max(1);
        let vol = config.volume.clamp(0.0, 2.0);
        let gain_linear = 10.0f32.powf(config.gain.gain_db / 20.0);

        // Process frame by frame
        for frame in samples.chunks_exact_mut(num_ch) {
            // For multi-channel, compute max level to drive envelope consistently
            let frame_max = frame.iter().fold(0.0f32, |acc, &s| acc.max(s.abs()));

            // 1. Noise Gate
            if config.noise_gate.enabled {
                let _ = self.gate.process_sample(
                    frame_max,
                    config.noise_gate.close_threshold_db,
                    config.noise_gate.open_threshold_db,
                    config.noise_gate.attack_time_ms,
                    config.noise_gate.hold_time_ms,
                    config.noise_gate.release_time_ms,
                );
                let gate_gain = self.gate.gain;
                for s in frame.iter_mut() {
                    *s *= gate_gain;
                }
            }

            // 2. Gain
            if config.gain.enabled {
                for s in frame.iter_mut() {
                    *s *= gain_linear;
                }
            }

            // 3. Compressor
            if config.compressor.enabled {
                let gated_gain_max = frame.iter().fold(0.0f32, |acc, &s| acc.max(s.abs()));
                let _ = self.compressor.process_sample(
                    gated_gain_max,
                    config.compressor.ratio,
                    config.compressor.threshold_db,
                    config.compressor.attack_time_ms,
                    config.compressor.release_time_ms,
                    config.compressor.output_gain_db,
                );
                let env_safe = self.compressor.env.max(1e-6);
                let env_db = 20.0 * env_safe.log10();
                let gain_reduction_db = if env_db > config.compressor.threshold_db {
                    let r = config.compressor.ratio.max(1.0);
                    let overshoot = env_db - config.compressor.threshold_db;
                    (config.compressor.threshold_db + overshoot / r) - env_db
                } else {
                    0.0
                };
                let total_gain_db = gain_reduction_db + config.compressor.output_gain_db;
                let comp_gain = 10.0f32.powf(total_gain_db / 20.0);
                for s in frame.iter_mut() {
                    *s *= comp_gain;
                }
            }

            // 4. Master Mic Volume (kept within full scale so the mic can never
            // overdrive the replay mix or the monitor output)
            for s in frame.iter_mut() {
                *s = (*s * vol).clamp(-1.0, 1.0);
            }
        }
    }
}

/// Source for Rodio monitor playback reading from an rtrb lock-free ring buffer
struct MicMonitorSource {
    consumer: Consumer<f32>,
    stop: Arc<AtomicBool>,
    channels: u16,
    sample_rate: u32,
}

impl Iterator for MicMonitorSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if let Ok(sample) = self.consumer.pop() {
            Some(sample)
        } else if self.stop.load(Ordering::Relaxed) {
            None
        } else {
            Some(0.0)
        }
    }
}

impl Source for MicMonitorSource {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.consumer.slots())
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

pub struct MicStreamHandle {
    pub device_name: String,
    pub stop: Arc<AtomicBool>,
    pub thread: Option<thread::JoinHandle<()>>,
    pub config: Arc<RwLock<MicConfig>>,
    _monitor_guard: Option<(OutputStream, rodio::OutputStreamHandle, Sink)>,
}

fn reap_mic_capture_thread(handle: thread::JoinHandle<()>) {
    if handle.is_finished() {
        if let Err(error) = handle.join() {
            tracing::warn!("Mic capture thread join failed: {:?}", error);
        }
        return;
    }

    // ALSA and audio driver calls can occasionally fail to return promptly.
    // Never make GUI or process shutdown wait on them.
    if let Err(error) = thread::Builder::new()
        .name("mic-capture-reaper".to_string())
        .spawn(move || {
            if let Err(error) = handle.join() {
                tracing::warn!("Mic capture thread join failed: {:?}", error);
            }
        })
    {
        tracing::warn!("Could not start mic capture reaper: {}", error);
    }
}

impl Drop for MicStreamHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some((_, _, sink)) = &self._monitor_guard {
            sink.stop();
        }
        if let Some(handle) = self.thread.take() {
            reap_mic_capture_thread(handle);
        }
    }
}

pub fn start_mic_stream(
    device_name: &str,
    initial_config: MicConfig,
    current_level_db: Arc<AtomicU32>,
    peak_level_db: Arc<AtomicU32>,
    is_clipping: Arc<AtomicBool>,
    replay: Arc<crate::replay::AudioTap>,
) -> Result<MicStreamHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let config = Arc::new(RwLock::new(initial_config));

    // Monitor playback output stream (Rodio)
    let (monitor_output, mut monitor_producer) = match initialize_monitor_playback(&stop) {
        Ok((guard, handle, sink, prod)) => (Some((guard, handle, sink)), Some(prod)),
        Err(e) => {
            tracing::warn!("Failed to initialize mic monitor audio output: {}", e);
            (None, None)
        }
    };

    let stop_clone = stop.clone();
    let config_clone = config.clone();
    let level_clone = current_level_db.clone();
    let peak_clone = peak_level_db.clone();
    let clipping_clone = is_clipping.clone();
    let dev_name = device_name.to_string();

    let thread = thread::Builder::new()
        .name("michadame-mic-capture".to_string())
        .spawn(move || {
            run_mic_capture_worker(
                &dev_name,
                stop_clone,
                config_clone,
                level_clone,
                peak_clone,
                clipping_clone,
                &mut monitor_producer,
                replay,
            );
        })
        .context("Failed to spawn mic capture thread")?;

    Ok(MicStreamHandle {
        device_name: device_name.to_string(),
        stop,
        thread: Some(thread),
        config,
        _monitor_guard: monitor_output,
    })
}

fn initialize_monitor_playback(stop: &Arc<AtomicBool>) -> Result<(
    OutputStream,
    rodio::OutputStreamHandle,
    Sink,
    Producer<f32>,
)> {
    let (guard, handle) = crate::devices::audio::initialize_rodio_playback()?;
    let sink = Sink::try_new(&handle).context("Failed to create monitor rodio sink")?;
    sink.set_volume(1.0);

    // 48 kHz stereo ring buffer (~150 ms capacity)
    let ring_size = (48000.0 * 2.0 * 0.15) as usize;
    let (producer, consumer) = RingBuffer::<f32>::new(ring_size);

    let source = MicMonitorSource {
        consumer,
        stop: Arc::clone(stop),
        channels: 2,
        sample_rate: 48000,
    };
    sink.append(source);

    Ok((guard, handle, sink, producer))
}

fn sleep_until_reconnect(stop: &Arc<AtomicBool>) {
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[allow(clippy::too_many_arguments)]
fn run_mic_capture_worker(
    device_name: &str,
    stop: Arc<AtomicBool>,
    config: Arc<RwLock<MicConfig>>,
    current_level_db: Arc<AtomicU32>,
    peak_level_db: Arc<AtomicU32>,
    is_clipping: Arc<AtomicBool>,
    monitor_producer: &mut Option<Producer<f32>>,
    replay: Arc<crate::replay::AudioTap>,
) {
    use alsa::pcm::{Access, Format, HwParams, PCM};
    use alsa::Direction;

    let mut last_peak_instant = Instant::now();
    let mut current_peak_val = -100.0f32;

    while !stop.load(Ordering::Relaxed) {
        replay.mark_mic_discontinuity();

        // Open ALSA PCM device
        let pcm = match PCM::new(device_name, Direction::Capture, true) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Could not open mic device {}: {}; retrying...", device_name, e);
                sleep_until_reconnect(&stop);
                continue;
            }
        };

        let hwp = match HwParams::any(&pcm) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("Mic HwParams::any failed: {}; retrying...", e);
                sleep_until_reconnect(&stop);
                continue;
            }
        };

        if let Err(e) = hwp.set_access(Access::RWInterleaved) {
            tracing::warn!("Mic set_access failed: {}", e);
            sleep_until_reconnect(&stop);
            continue;
        }

        // Negotiate sample format: try FloatLE, S32LE, or S16LE
        let (_pcm_format, sample_format_str) = if hwp.set_format(Format::FloatLE).is_ok() {
            (Format::FloatLE, "F32LE")
        } else if hwp.set_format(Format::S32LE).is_ok() {
            (Format::S32LE, "S32LE")
        } else if hwp.set_format(Format::S16LE).is_ok() {
            (Format::S16LE, "S16LE")
        } else {
            tracing::warn!("Mic device supports neither FloatLE, S32LE, nor S16LE");
            sleep_until_reconnect(&stop);
            continue;
        };

        // Negotiate rate near 48000 Hz
        let rate = match hwp.set_rate_near(48000, alsa::ValueOr::Nearest) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Mic set_rate_near failed: {}", e);
                sleep_until_reconnect(&stop);
                continue;
            }
        };

        // Negotiate channels: prefer 1 or 2
        let channels = match hwp.set_channels_near(1) {
            Ok(c) => c as u16,
            Err(e) => {
                tracing::warn!("Mic set_channels_near failed: {}", e);
                sleep_until_reconnect(&stop);
                continue;
            }
        };

        let period_frames = (rate / 100).clamp(128, 1024); // ~10-20ms period
        let _ = hwp.set_period_size_near(period_frames as alsa::pcm::Frames, alsa::ValueOr::Nearest);

        if let Err(e) = pcm.hw_params(&hwp) {
            tracing::warn!("Mic hw_params failed: {}", e);
            sleep_until_reconnect(&stop);
            continue;
        }
        drop(hwp);

        tracing::info!(
            "Mic capture started on '{}' ({} Hz, {} channels, format {})",
            device_name,
            rate,
            channels,
            sample_format_str
        );

        let mut pipeline = MicDspPipeline::new(rate as f32);
        let buffer_samples = period_frames as usize * channels as usize;
        let mut f32_buf = vec![0.0f32; buffer_samples];
        // Conversion buffers and the config snapshot are reused across periods
        // so the capture thread does not allocate every ~10 ms.
        let mut i32_buf = vec![0i32; if sample_format_str == "S32LE" { buffer_samples } else { 0 }];
        let mut i16_buf = vec![0i16; if sample_format_str == "S16LE" { buffer_samples } else { 0 }];
        let mut cfg = config.read().map(|c| c.clone()).unwrap_or_default();

        // Capture loop
        while !stop.load(Ordering::Relaxed) {
            let read_res = match sample_format_str {
                "F32LE" => {
                    let io = match pcm.io_f32() {
                        Ok(io) => io,
                        Err(_) => break,
                    };
                    io.readi(&mut f32_buf)
                }
                "S32LE" => {
                    let io = match pcm.io_i32() {
                        Ok(io) => io,
                        Err(_) => break,
                    };
                    match io.readi(&mut i32_buf) {
                        Ok(frames) => {
                            let count = frames * channels as usize;
                            for (out, &inp) in f32_buf.iter_mut().zip(&i32_buf[..count]) {
                                *out = (inp as f64 / 2_147_483_648.0) as f32;
                            }
                            Ok(frames)
                        }
                        Err(e) => Err(e),
                    }
                }
                _ => {
                    let io = match pcm.io_i16() {
                        Ok(io) => io,
                        Err(_) => break,
                    };
                    match io.readi(&mut i16_buf) {
                        Ok(frames) => {
                            let count = frames * channels as usize;
                            for (out, &inp) in f32_buf.iter_mut().zip(&i16_buf[..count]) {
                                *out = (inp as f32) / 32768.0;
                            }
                            Ok(frames)
                        }
                        Err(e) => Err(e),
                    }
                }
            };

            match read_res {
                Ok(frames) if frames > 0 => {
                    let sample_count = frames * channels as usize;
                    let active_buf = &mut f32_buf[..sample_count];

                    // Never block on the UI thread's writer; keep the last snapshot.
                    if let Ok(latest) = config.try_read() {
                        cfg.clone_from(&latest);
                    }

                    // Apply filter pipeline
                    pipeline.process_interleaved(active_buf, channels as usize, &cfg);

                    // Compute RMS/peak for GUI meter
                    let mut max_abs = 0.0f32;
                    for &s in active_buf.iter() {
                        max_abs = max_abs.max(s.abs());
                    }

                    let cur_db = if max_abs > 1e-5 {
                        20.0 * max_abs.log10()
                    } else {
                        -100.0
                    };
                    current_level_db.store(cur_db.to_bits(), Ordering::Relaxed);

                    // Peak hold decay: hold for 1.2s, then decay smoothly
                    if cur_db > current_peak_val || last_peak_instant.elapsed() > Duration::from_millis(1200) {
                        current_peak_val = cur_db;
                        last_peak_instant = Instant::now();
                    } else {
                        current_peak_val = (current_peak_val - 0.3).max(-100.0);
                    }
                    peak_level_db.store(current_peak_val.to_bits(), Ordering::Relaxed);

                    is_clipping.store(max_abs >= 0.999, Ordering::Relaxed);

                    // Submit to monitor if enabled
                    if cfg.monitor {
                        if let Some(prod) = monitor_producer {
                            if channels == 1 {
                                for &s in active_buf.iter() {
                                    let _ = prod.push(s);
                                    let _ = prod.push(s);
                                }
                            } else {
                                for &s in active_buf.iter() {
                                    let _ = prod.push(s);
                                }
                            }
                        }
                    }

                    // Submit to replay buffer
                    if replay.is_enabled() {
                        let delay = pcm.delay().unwrap_or(0).max(0);
                        let at = crate::replay::now_us()
                            - (delay + frames as i64) * 1_000_000 / rate as i64;
                        replay.submit_mic(active_buf, rate, channels, at);
                    }
                }
                Ok(_) => {
                    let _ = pcm.wait(Some(20));
                }
                Err(e) => {
                    if e.errno() == alsa::nix::errno::Errno::EAGAIN {
                        let _ = pcm.wait(Some(20));
                    } else {
                        let _ = pcm.try_recover(e, true);
                        let _ = pcm.wait(Some(20));
                    }
                }
            }
        }
    }

    current_level_db.store((-100.0f32).to_bits(), Ordering::Relaxed);
    peak_level_db.store((-100.0f32).to_bits(), Ordering::Relaxed);
    is_clipping.store(false, Ordering::Relaxed);
    tracing::debug!("Mic capture thread stopped.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gain_filter() {
        let sample = 0.5f32;
        let unity = GainFilter::process_sample(sample, 0.0);
        assert!((unity - 0.5).abs() < 1e-5);

        // +6.02 dB should approximately double amplitude
        let boosted = GainFilter::process_sample(sample, 6.0206);
        assert!((boosted - 1.0).abs() < 1e-3);

        // -6.02 dB should halve amplitude
        let attenuated = GainFilter::process_sample(sample, -6.0206);
        assert!((attenuated - 0.25).abs() < 1e-3);
    }

    #[test]
    fn test_noise_gate_attenuation_and_opening() {
        let fs = 48000.0;
        let mut gate = NoiseGateFilter::new(fs);

        // Signal below close threshold (-52 dB = 0.0025 linear)
        // With input = 0.001 linear (~ -60 dB), gate should be closed (gain = 0.0)
        let quiet = 0.001;
        let out = gate.process_sample(quiet, -52.0, -42.0, 0.0, 400.0, 0.0);
        assert_eq!(out, 0.0);
        assert_eq!(gate.state, GateState::Closed);

        // Signal above open threshold (-42 dB = 0.0079 linear)
        // With input = 0.1 linear (~ -20 dB), gate should open instantaneously (attack = 0ms)
        let loud = 0.1;
        let out_loud = gate.process_sample(loud, -52.0, -42.0, 0.0, 400.0, 0.0);
        assert!((out_loud - loud).abs() < 1e-5);
        assert_eq!(gate.state, GateState::Open);

        // Signal drops below close threshold: should remain holding for 400ms (19200 samples)
        let _ = gate.process_sample(0.0001, -52.0, -42.0, 0.0, 400.0, 0.0);
        assert_eq!(gate.state, GateState::Holding);
    }

    #[test]
    fn test_compressor_above_threshold() {
        let fs = 48000.0;
        let mut comp = CompressorFilter::new(fs);

        // Warm up compressor with a loud signal at 0 dB (1.0 amplitude)
        // Threshold is -18 dB, Ratio is 4:1, Output gain is 0 dB
        for _ in 0..4800 {
            comp.process_sample(1.0, 4.0, -18.0, 1.0, 50.0, 0.0);
        }

        // Overshoot above -18 dB is 18 dB.
        // With 4:1 ratio, 18 dB overshoot is compressed to 18 / 4 = 4.5 dB.
        // Total level is -18 + 4.5 = -13.5 dB (linear ~ 0.211).
        let out = comp.process_sample(1.0, 4.0, -18.0, 1.0, 50.0, 0.0);
        let out_db = 20.0 * out.log10();
        assert!((out_db - (-13.5)).abs() < 1.0, "Expected ~ -13.5 dB, got {:.2}", out_db);
    }

    #[test]
    fn test_mic_dsp_pipeline_ordering() {
        let mut pipeline = MicDspPipeline::new(48000.0);
        let mut config = MicConfig::default();

        // 1. When mic quiet, noise gate gates it to 0.0
        let mut quiet_buf = vec![0.001; 64];
        pipeline.process_interleaved(&mut quiet_buf, 1, &config);
        assert_eq!(quiet_buf[0], 0.0);

        // 2. Disable noise gate, enable gain +6dB, master vol 1.0
        config.noise_gate.enabled = false;
        config.compressor.enabled = false;
        config.gain.enabled = true;
        config.gain.gain_db = 6.0206;
        config.volume = 1.0;

        let mut buf = vec![0.5; 64];
        pipeline.process_interleaved(&mut buf, 1, &config);
        assert!((buf[0] - 1.0).abs() < 1e-3);

        // 3. Test master volume scaling
        config.volume = 0.5;
        let mut buf2 = vec![0.5; 64];
        pipeline.process_interleaved(&mut buf2, 1, &config);
        assert!((buf2[0] - 0.5).abs() < 1e-3);
    }
}
