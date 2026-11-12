use alsa::device_name::HintIter;
use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use rodio::{OutputStream, Source};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const AUDIO_UNDERRUN_SILENCE: Duration = Duration::from_millis(1);
const AUDIO_DECLICK_DURATION: Duration = Duration::from_millis(5);
const DC_BLOCKER_CUTOFF_HZ: f64 = 5.0;
const RESAMPLE_FP_ONE: u64 = 1u64 << 32;
const MAX_ADAPTIVE_RATE_TRIM: f64 = 0.005;
const PREFERRED_OUTPUT_SAMPLE_RATE: cpal::SampleRate = cpal::SampleRate(48_000);
const ALSA_CAPTURE_POLL_TIMEOUT_MS: u32 = 10;
const ALSA_RECONNECT_DELAY: Duration = Duration::from_millis(50);
const ALSA_STALL_TIMEOUT: Duration = Duration::from_secs(2);
const ALSA_RECOVERY_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Default)]
pub struct PlaybackStatistics {
    pub queued_frames: AtomicU64,
    pub silence_frames: AtomicU64,
    pub drift_dropped_frames: AtomicU64,
}

pub struct AudioStreamHandle {
    pub statistics: Arc<PlaybackStatistics>,
    pub sample_rate: u32,
    pub channels: u16,
    alsa_capture_thread: Option<thread::JoinHandle<()>>,
    capture_failure_receiver: crossbeam_channel::Receiver<String>,
    stop_capture: Arc<AtomicBool>,
    stop_playback: Arc<AtomicBool>,
    volume_bits: Arc<AtomicU32>,
    _output_stream_guard: SharedPlaybackGuard,
    _output_stream_handle: rodio::OutputStreamHandle,
}

impl Drop for AudioStreamHandle {
    fn drop(&mut self) {
        self.stop_capture.store(true, Ordering::Relaxed);
        self.stop_playback.store(true, Ordering::Relaxed);
        if let Some(handle) = self.alsa_capture_thread.take() {
            reap_alsa_capture_thread(handle);
        }
    }
}

impl AudioStreamHandle {
    pub fn take_capture_failure(&self) -> Option<String> {
        self.capture_failure_receiver.try_recv().ok()
    }

    pub fn set_volume(&self, volume: f32) {
        let sanitized = if volume.is_finite() {
            volume.max(0.0)
        } else {
            1.0
        };
        self.volume_bits
            .store(sanitized.to_bits(), Ordering::Relaxed);
    }
}

fn reap_alsa_capture_thread(handle: thread::JoinHandle<()>) {
    if handle.is_finished() {
        if let Err(error) = handle.join() {
            tracing::warn!("ALSA capture thread join failed: {:?}", error);
        }
        return;
    }

    // ALSA and USB driver calls are outside Rust's control and can occasionally fail to return.
    // Never make the GUI or process shutdown wait on such a call.
    if let Err(error) = thread::Builder::new()
        .name("alsa-capture-reaper".to_string())
        .spawn(move || {
            if let Err(error) = handle.join() {
                tracing::warn!("ALSA capture thread join failed: {:?}", error);
            }
        })
    {
        tracing::warn!("Could not start ALSA capture reaper: {}", error);
    }
}

struct AudioCaptureProgress {
    last_sample_at: Instant,
}

struct AlsaCapture {
    thread: thread::JoinHandle<()>,
    failure_receiver: crossbeam_channel::Receiver<String>,
    consumer: Consumer<f32>,
    channels: u16,
    sample_rate: u32,
}

impl AudioCaptureProgress {
    fn new() -> Self {
        Self {
            last_sample_at: Instant::now(),
        }
    }

    fn mark_samples(&mut self) {
        self.last_sample_at = Instant::now();
    }

    fn stalled_for(&self, timeout: Duration) -> bool {
        self.last_sample_at.elapsed() >= timeout
    }
}

/// First-order high-precision IIR high-pass filter (~5 Hz cutoff) removing steady DC bias from
/// capture hardware so silence never carries a non-zero offset that could click across transitions.
struct DcBlocker {
    pole: f64,
    prev_x: Vec<f64>,
    prev_y: Vec<f64>,
    initialized: bool,
}

impl DcBlocker {
    fn new(sample_rate: u32, channels: u16) -> Self {
        let ch = (channels as usize).max(1);
        let sr = sample_rate.max(1) as f64;
        let pole = (-2.0 * std::f64::consts::PI * DC_BLOCKER_CUTOFF_HZ / sr).exp();
        Self {
            pole,
            prev_x: vec![0.0; ch],
            prev_y: vec![0.0; ch],
            initialized: false,
        }
    }

    fn reset(&mut self) {
        self.prev_x.fill(0.0);
        self.prev_y.fill(0.0);
        self.initialized = false;
    }

    fn process_interleaved(&mut self, samples: &mut [f32]) {
        let channels = self.prev_x.len();
        let frames = samples.len() / channels;
        if frames == 0 {
            return;
        }

        let mut start_frame = 0;
        if !self.initialized {
            for ch in 0..channels {
                let s = samples[ch];
                self.prev_x[ch] = if s.is_finite() { s as f64 } else { 0.0 };
                self.prev_y[ch] = 0.0;
                samples[ch] = 0.0;
            }
            self.initialized = true;
            start_frame = 1;
        }

        let pole = self.pole;
        for frame in start_frame..frames {
            let base = frame * channels;
            for ch in 0..channels {
                let raw = samples[base + ch];
                let x = if raw.is_finite() { raw as f64 } else { 0.0 };
                let y = x - self.prev_x[ch] + pole * self.prev_y[ch];
                self.prev_x[ch] = x;
                self.prev_y[ch] = y;
                samples[base + ch] = y as f32;
            }
        }
    }
}

struct LiveSource {
    statistics: Arc<PlaybackStatistics>,
    consumer: Consumer<f32>,
    channels: u16,
    sample_rate: u32,
    audio_latency_ms: Arc<AtomicU64>,
    capture_delay_ms: Arc<AtomicU64>,
    stop_playback: Arc<AtomicBool>,
    volume_bits: Arc<AtomicU32>,
    current_volume: f32,
    volume_slew_alpha: f32,
    staging_buf: Vec<f32>,
    local_buf: Vec<f32>,
    local_idx: usize,
    valid_len: usize,
    buffering: bool,
    has_started_playback: bool,
    prebuffer_frames: usize,
    target_frames: usize,
    max_drift_frames: usize,
    adaptive_resampling: bool,
    ema_queued_frames: f64,
    integral_trim: f64,
    resample_step: f64,
    resample_pos_fp: u64,
    resample_hist: Vec<f32>,
    resample_primed: bool,
    last_output_frame: Vec<f32>,
    declick_next_audio: bool,
}

impl Iterator for LiveSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let channels = self.channels as usize;
        let channel = self.local_idx % channels;

        if channel == 0 && self.stop_playback.load(Ordering::Relaxed) {
            return None;
        }

        if self.local_idx >= self.valid_len {
            self.refill_local_buf();
        }

        if channel == 0 {
            let target_vol = f32::from_bits(self.volume_bits.load(Ordering::Relaxed));
            let diff = target_vol - self.current_volume;
            if diff.abs() <= 1e-6 {
                self.current_volume = target_vol;
            } else {
                self.current_volume += diff * self.volume_slew_alpha;
            }
        }

        let raw_sample = self.local_buf[self.local_idx];
        self.last_output_frame[channel] = raw_sample;
        self.local_idx += 1;
        Some(raw_sample * self.current_volume)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.stop_playback.load(Ordering::Relaxed) {
            (0, Some(0))
        } else {
            (usize::MAX, None)
        }
    }
}

impl LiveSource {
    #[allow(clippy::too_many_arguments)]
    fn new(
        statistics: Arc<PlaybackStatistics>,
        consumer: Consumer<f32>,
        channels: u16,
        sample_rate: u32,
        buffer_size: u32,
        audio_latency_ms: Arc<AtomicU64>,
        capture_delay_ms: Arc<AtomicU64>,
        stop_playback: Arc<AtomicBool>,
        volume_bits: Arc<AtomicU32>,
    ) -> Self {
        let channels = channels.max(1);
        let ch = channels as usize;
        let sr = sample_rate.max(1) as usize;
        let ring_capacity_frames = (consumer.buffer().capacity() / ch).max(1);

        let declick_frames =
            ((sr as f64 * AUDIO_DECLICK_DURATION.as_secs_f64()).round() as usize).max(1);
        let out_chunk_frames = (sr / 200).max(declick_frames).max(256);

        let capture_period_frames = (buffer_size as usize).max(1);
        // CPAL's Linux ALSA backend uses a 25 ms output period by default (`sr / 40`), pulling an
        // entire 25 ms burst synchronously in one callback. Prebuffering `P + C + margin` ensures
        // the trough at the end of any output callback stays strictly positive across all beat
        // phases between capture periods and playback callbacks.
        let output_burst_frames = (sr / 40).max(1);
        let jitter_margin_frames = (sr * 15 / 1000).max(1);

        let max_safe_prebuffer = (ring_capacity_frames / 2).max(1);
        let prebuffer_frames = (capture_period_frames + output_burst_frames + jitter_margin_frames)
            .min(max_safe_prebuffer);
        let target_frames = (jitter_margin_frames
            + (capture_period_frames + output_burst_frames) / 2)
            .min(prebuffer_frames)
            .max(1);
        let desired_max_drift = (sr * 150 / 1000)
            .max(prebuffer_frames + capture_period_frames * 2 + output_burst_frames);
        let max_drift_frames = desired_max_drift
            .min(ring_capacity_frames.saturating_sub(capture_period_frames))
            .max(prebuffer_frames + 1);

        let adaptive_resampling = prebuffer_frames >= 8 && ring_capacity_frames >= 16;
        let initial_vol = f32::from_bits(volume_bits.load(Ordering::Relaxed));
        // ~4 ms exponential smoothing time constant for zipper-free volume changes
        let volume_slew_alpha = (1.0 / (0.004 * sr as f32)).clamp(0.0005, 1.0);

        Self {
            statistics,
            consumer,
            channels,
            sample_rate: sample_rate.max(1),
            audio_latency_ms,
            capture_delay_ms,
            stop_playback,
            volume_bits,
            current_volume: initial_vol,
            volume_slew_alpha,
            staging_buf: vec![0.0; (out_chunk_frames + 16) * ch],
            local_buf: vec![0.0; out_chunk_frames * ch],
            local_idx: 0,
            valid_len: 0,
            buffering: true,
            has_started_playback: false,
            prebuffer_frames,
            target_frames,
            max_drift_frames,
            adaptive_resampling,
            ema_queued_frames: target_frames as f64,
            integral_trim: 0.0,
            resample_step: 1.0,
            resample_pos_fp: 0,
            resample_hist: vec![0.0; 4 * ch],
            resample_primed: false,
            last_output_frame: vec![0.0; ch],
            declick_next_audio: true,
        }
    }

    fn current_step_fp(&self) -> u64 {
        if !self.adaptive_resampling {
            return RESAMPLE_FP_ONE;
        }
        ((self.resample_step * (RESAMPLE_FP_ONE as f64)).round() as u64).max(1)
    }

    fn update_adaptive_rate(&mut self, mid_queued_frames: f64, out_frames: usize) {
        let sr = self.sample_rate as f64;
        let dt = out_frames as f64 / sr;
        let alpha = dt / (0.5 + dt);
        self.ema_queued_frames += alpha * (mid_queued_frames - self.ema_queued_frames);

        let err_sec = (self.ema_queued_frames - self.target_frames as f64) / sr;
        // Overdamped PI loop (`zeta > 1.2`) tracking hardware crystal skew with zero overshoot
        // or audible pitch modulation.
        const KP: f64 = 0.12;
        const KI: f64 = 0.0025;

        self.integral_trim = (self.integral_trim + KI * err_sec * dt)
            .clamp(-MAX_ADAPTIVE_RATE_TRIM, MAX_ADAPTIVE_RATE_TRIM);
        let trim = (KP * err_sec + self.integral_trim)
            .clamp(-MAX_ADAPTIVE_RATE_TRIM, MAX_ADAPTIVE_RATE_TRIM);
        self.resample_step = 1.0 + trim;
    }

    fn refill_local_buf(&mut self) {
        // This runs directly in CPAL's real-time output callback. It must never block or allocate.
        let channels = self.channels as usize;
        let out_chunk_frames = (self.local_buf.len() / channels).max(1);

        let queued_samples = self.consumer.slots();
        let mut queued_frames = queued_samples / channels;

        // Publish once per refill, never per sample; no locks or allocations.
        self.statistics
            .queued_frames
            .store(queued_frames as u64, Ordering::Relaxed);

        let ring_buffer_ms = (queued_frames as f64 / self.sample_rate as f64 * 1000.0) as u64;
        let capture_ms = self.capture_delay_ms.load(Ordering::Relaxed);
        // Heuristic for playback latency: Rodio/CPAL usually buffers 2-3 periods (~20ms baseline).
        let playback_ms = 20;
        self.audio_latency_ms
            .store(capture_ms + ring_buffer_ms + playback_ms, Ordering::Relaxed);

        // Emergency latency bound if an extreme stall builds a backlog beyond normal resampler range.
        let mut skipped_audio = false;
        if queued_frames > self.max_drift_frames {
            let keep_frames = self.prebuffer_frames.min(queued_frames);
            let drop_frames = queued_frames.saturating_sub(keep_frames);
            if drop_frames > 0 {
                if let Ok(chunk) = self.consumer.read_chunk(drop_frames * channels) {
                    chunk.commit_all();
                    self.statistics
                        .drift_dropped_frames
                        .fetch_add(drop_frames as u64, Ordering::Relaxed);
                    queued_frames -= drop_frames;
                    self.resample_primed = false;
                    self.resample_pos_fp = 0;
                    self.ema_queued_frames = self.target_frames as f64;
                    skipped_audio = true;
                }
            }
        }

        // Hold in non-blocking silence until the jitter/period cushion is established so playback
        // does not scrape empty on every capture/playback beat cycle.
        if self.buffering {
            if queued_frames < self.prebuffer_frames {
                self.emit_silence_or_fade_out(self.has_started_playback);
                return;
            }
            self.buffering = false;
            self.ema_queued_frames = self.target_frames as f64;
        }

        let step_fp = self.current_step_fp();
        let prime_frames = if self.adaptive_resampling && !self.resample_primed {
            3
        } else {
            0
        };

        if queued_frames < prime_frames {
            self.enter_underrun();
            return;
        }

        let max_staging_frames = self.staging_buf.len() / channels;
        let avail_after_prime =
            (queued_frames - prime_frames).min(max_staging_frames.saturating_sub(prime_frames));

        let out_frames = if self.adaptive_resampling {
            let numer = ((avail_after_prime as u64 + 1) << 32)
                .saturating_sub(1)
                .saturating_sub(self.resample_pos_fp);
            ((numer / step_fp) as usize).min(out_chunk_frames)
        } else {
            avail_after_prime.min(out_chunk_frames)
        };

        if out_frames == 0 {
            self.enter_underrun();
            return;
        }

        let shift_frames = if self.adaptive_resampling {
            ((self.resample_pos_fp + out_frames as u64 * step_fp) >> 32) as usize
        } else {
            out_frames
        };
        let total_read_frames = prime_frames + shift_frames;
        let total_read_samples = total_read_frames * channels;

        if total_read_samples > 0 {
            let Ok(chunk) = self.consumer.read_chunk(total_read_samples) else {
                self.enter_underrun();
                return;
            };
            let (s1, s2) = chunk.as_slices();
            let l1 = s1.len();
            if self.adaptive_resampling {
                self.staging_buf[..l1].copy_from_slice(s1);
                if !s2.is_empty() {
                    self.staging_buf[l1..total_read_samples].copy_from_slice(s2);
                }
            } else {
                self.local_buf[..l1].copy_from_slice(s1);
                if !s2.is_empty() {
                    self.local_buf[l1..total_read_samples].copy_from_slice(s2);
                }
            }
            chunk.commit_all();
        }

        if self.adaptive_resampling {
            let mut staging_frame_idx = 0;
            if prime_frames > 0 {
                let (f0, rest) = self.staging_buf.split_at(channels);
                let (f1, rest2) = rest.split_at(channels);
                let f2 = &rest2[..channels];
                self.resample_hist[0..channels].copy_from_slice(f0);
                self.resample_hist[channels..2 * channels].copy_from_slice(f0);
                self.resample_hist[2 * channels..3 * channels].copy_from_slice(f1);
                self.resample_hist[3 * channels..4 * channels].copy_from_slice(f2);
                self.resample_pos_fp = 0;
                self.resample_primed = true;
                staging_frame_idx = 3;
            }

            for out_f in 0..out_frames {
                let frac = (self.resample_pos_fp as u32) as f32 * (1.0 / 4_294_967_296.0);
                let out_base = out_f * channels;
                if frac == 0.0 {
                    self.local_buf[out_base..out_base + channels]
                        .copy_from_slice(&self.resample_hist[channels..2 * channels]);
                } else {
                    for ch in 0..channels {
                        let ym1 = self.resample_hist[ch];
                        let y0 = self.resample_hist[channels + ch];
                        let y1 = self.resample_hist[2 * channels + ch];
                        let y2 = self.resample_hist[3 * channels + ch];
                        let a0 = -0.5 * ym1 + 1.5 * y0 - 1.5 * y1 + 0.5 * y2;
                        let a1 = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
                        let a2 = -0.5 * ym1 + 0.5 * y1;
                        self.local_buf[out_base + ch] = ((a0 * frac + a1) * frac + a2) * frac + y0;
                    }
                }

                self.resample_pos_fp += step_fp;
                let shifts = (self.resample_pos_fp >> 32) as usize;
                self.resample_pos_fp &= 0xFFFF_FFFF;
                for _ in 0..shifts {
                    self.resample_hist.copy_within(channels..4 * channels, 0);
                    let src_base = staging_frame_idx * channels;
                    self.resample_hist[3 * channels..4 * channels]
                        .copy_from_slice(&self.staging_buf[src_base..src_base + channels]);
                    staging_frame_idx += 1;
                }
            }
            debug_assert_eq!(staging_frame_idx, total_read_frames);

            let mid_queued = queued_frames as f64 - 0.5 * total_read_frames as f64;
            self.update_adaptive_rate(mid_queued, out_frames);
        }

        self.has_started_playback = true;
        self.valid_len = out_frames * channels;
        self.local_idx = 0;
        if self.declick_next_audio || skipped_audio {
            self.declick_audio_start();
            self.declick_next_audio = false;
        }
    }

    fn enter_underrun(&mut self) {
        self.buffering = true;
        self.resample_primed = false;
        self.resample_pos_fp = 0;
        self.declick_next_audio = true;
        self.emit_silence_or_fade_out(true);
    }

    fn emit_silence_or_fade_out(&mut self, count_as_underrun: bool) {
        let channels = self.channels as usize;
        let max_frames = (self.local_buf.len() / channels).max(1);
        let needs_fade = self.last_output_frame.iter().any(|&s| s.abs() > 1e-6);

        let frames = if needs_fade {
            self.declick_frame_count().min(max_frames)
        } else {
            let poll_frames =
                (self.sample_rate as f64 * AUDIO_UNDERRUN_SILENCE.as_secs_f64()).round() as usize;
            poll_frames.max(1).min(max_frames)
        };

        if count_as_underrun {
            self.statistics
                .silence_frames
                .fetch_add(frames as u64, Ordering::Relaxed);
        }

        self.valid_len = frames * channels;
        self.local_idx = 0;
        if needs_fade {
            self.fade_to_silence();
        } else {
            self.local_buf[..self.valid_len].fill(0.0);
        }
        self.declick_next_audio = true;
    }

    fn declick_frame_count(&self) -> usize {
        ((self.sample_rate as f64 * AUDIO_DECLICK_DURATION.as_secs_f64()).round() as usize).max(1)
    }

    /// Smooth a discontinuity with a C1 raised-cosine (half-Hann) crossfade operating purely in
    /// the preallocated local buffer.
    fn declick_audio_start(&mut self) {
        let channels = self.channels as usize;
        let available_frames = self.valid_len / channels;
        let fade_frames = self.declick_frame_count().min(available_frames);

        for frame in 0..fade_frames {
            let u = (frame + 1) as f32 / fade_frames as f32;
            let new_audio_weight = 0.5 * (1.0 - (std::f32::consts::PI * u).cos());
            let previous_audio_weight = 1.0 - new_audio_weight;
            for channel in 0..channels {
                let index = frame * channels + channel;
                self.local_buf[index] = self.last_output_frame[channel] * previous_audio_weight
                    + self.local_buf[index] * new_audio_weight;
            }
        }
    }

    fn fade_to_silence(&mut self) {
        let channels = self.channels as usize;
        let available_frames = self.valid_len / channels;
        let fade_frames = self.declick_frame_count().min(available_frames);

        for frame in 0..fade_frames {
            let u = (frame + 1) as f32 / fade_frames as f32;
            let weight = 0.5 * (1.0 + (std::f32::consts::PI * u).cos());
            for channel in 0..channels {
                self.local_buf[frame * channels + channel] =
                    self.last_output_frame[channel] * weight;
            }
        }

        self.local_buf[fade_frames * channels..self.valid_len].fill(0.0);
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

pub fn is_usable_input_device(name: &str, desc: &str, direction: Option<alsa::Direction>) -> bool {
    // If ALSA explicitly flagged it as Playback (output), skip it
    if direction == Some(alsa::Direction::Playback) {
        return false;
    }

    let name_lower = name.to_lowercase();
    let desc_lower = desc.to_lowercase();

    // Skip ALSA internal plugins and virtual devices that are not physical/server capture endpoints
    if name_lower == "null"
        || name_lower == "jack"
        || name_lower.starts_with("jack:")
        || name_lower == "oss"
        || name_lower.contains("oss")
        || name_lower == "samplerate"
        || name_lower == "speexrate"
        || name_lower == "lavrate"
        || name_lower == "speex"
        || name_lower == "upmix"
        || name_lower == "vdownmix"
        || name_lower == "dmix"
        || name_lower == "dsnoop"
    {
        return false;
    }

    // Skip surround and playback-only profiles
    if name_lower.starts_with("surround")
        || name_lower.starts_with("iec958")
        || name_lower.starts_with("spdif")
        || name_lower.starts_with("hdmi")
        || name_lower.starts_with("usbstream")
    {
        return false;
    }

    // Skip duplicate sysdefault (default:CARD=... already covers the card)
    if name_lower.starts_with("sysdefault") {
        return false;
    }

    // If description explicitly indicates an output-only profile and not input, skip
    if desc_lower.contains("output") && !desc_lower.contains("input") {
        return false;
    }

    true
}

pub fn filter_usable_audio_hints<I, F>(hints: I, mut can_open_capture: F) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (String, String, Option<alsa::Direction>)>,
    F: FnMut(&str) -> bool,
{
    let mut sources = Vec::new();
    let mut seen_card_keys = std::collections::HashSet::new();
    let mut seen_names = std::collections::HashSet::new();

    for (name, desc, direction) in hints {
        if !is_usable_input_device(&name, &desc, direction) {
            continue;
        }

        if seen_names.contains(&name) {
            continue;
        }

        if !can_open_capture(&name) {
            continue;
        }

        // Deduplicate multiple PCM interfaces for the same physical card/device (preferring default:CARD=...)
        if let Some(card_part) = name.split(':').nth(1) {
            let mut parts = card_part.split(',');
            let card = parts.next().unwrap_or("");
            if card.starts_with("CARD=") {
                let dev = parts.find(|p| p.starts_with("DEV=")).unwrap_or("DEV=0");
                let card_key = format!("{card}:{dev}");
                if !seen_card_keys.insert(card_key) {
                    continue;
                }
            }
        }

        seen_names.insert(name.clone());

        let display_name = if let Some((first, second)) = desc.split_once('\n') {
            if second.trim() == "Default Audio Device" {
                first.trim().to_string()
            } else {
                format!("{} - {}", first.trim(), second.trim())
            }
        } else {
            desc.replace('\n', " - ")
        };

        sources.push((display_name, name));
    }

    sources.sort_by(|(a, _), (b, _)| a.cmp(b));
    sources
}

pub fn find_audio_devices() -> Result<Vec<(String, String)>> {
    static INIT_ALSA_ERROR_HANDLER: std::sync::Once = std::sync::Once::new();
    INIT_ALSA_ERROR_HANDLER.call_once(|| unsafe {
        use libc::{c_char, c_int};
        type SndLibErrorHandler =
            unsafe extern "C" fn(*const c_char, c_int, *const c_char, c_int, *const c_char, ...);
        extern "C" {
            fn michadame_alsa_silent_error_handler(
                file: *const c_char,
                line: c_int,
                function: *const c_char,
                err: c_int,
                fmt: *const c_char,
                ...
            );
            fn snd_lib_error_set_handler(handler: SndLibErrorHandler) -> c_int;
        }
        snd_lib_error_set_handler(michadame_alsa_silent_error_handler);
    });

    use alsa::pcm::PCM;
    use alsa::Direction;

    let mut raw_hints = Vec::new();
    if let Ok(hints) = HintIter::new_str(None, "pcm") {
        for hint in hints {
            if let (Some(name), Some(desc)) = (hint.name, hint.desc) {
                raw_hints.push((name, desc, hint.direction));
            }
        }
    }

    Ok(filter_usable_audio_hints(
        raw_hints,
        |name| match PCM::new(name, Direction::Capture, true) {
            Ok(_) => true,
            Err(e) if e.to_string().contains("busy") => true,
            _ => false,
        },
    ))
}

#[allow(clippy::too_many_arguments)]
pub fn start_audio_stream(
    source_name: &str,
    peak_amplitude_shared: Arc<AtomicU64>,
    audio_latency_ms: Arc<AtomicU64>,
    buffer_size: u32,
    sample_rate: u32,
    sample_format: String,
    request_repaint: Arc<dyn Fn() + Send + Sync>,
    replay: Arc<crate::replay::AudioTap>,
) -> Result<AudioStreamHandle> {
    let capture_delay_ms = Arc::new(AtomicU64::new(0));
    let stop_capture = Arc::new(AtomicBool::new(false));
    let stop_playback = Arc::new(AtomicBool::new(false));
    let volume_bits = Arc::new(AtomicU32::new(1.0f32.to_bits()));

    let AlsaCapture {
        thread: alsa_thread,
        failure_receiver: capture_failure_receiver,
        consumer,
        channels: input_channels,
        sample_rate: input_sample_rate,
    } = start_alsa_capture(
        source_name,
        peak_amplitude_shared.clone(),
        capture_delay_ms.clone(),
        stop_capture.clone(),
        buffer_size,
        sample_rate,
        sample_format,
        request_repaint,
        replay,
    )?;
    let mut alsa_thread = Some(alsa_thread);

    // Playback via Rodio's lock-free DynamicMixer (`play_raw`) rather than `Sink` so `LiveSource`
    // avoids `SourcesQueueOutput`'s 512-sample artificial frame truncation and 5ms mutex polling.
    let (output_stream_guard, stream_handle) = match initialize_rodio_playback() {
        Ok(stream) => stream,
        Err(e) => {
            stop_capture.store(true, Ordering::Relaxed);
            stop_playback.store(true, Ordering::Relaxed);
            if let Some(handle) = alsa_thread.take() {
                reap_alsa_capture_thread(handle);
            }
            return Err(e);
        }
    };

    let statistics = Arc::new(PlaybackStatistics::default());
    let live_source = LiveSource::new(
        statistics.clone(),
        consumer,
        input_channels,
        input_sample_rate,
        buffer_size,
        audio_latency_ms,
        capture_delay_ms,
        stop_playback.clone(),
        volume_bits.clone(),
    );

    if let Err(e) = stream_handle
        .play_raw(live_source)
        .context("Failed to start live audio playback")
    {
        stop_capture.store(true, Ordering::Relaxed);
        stop_playback.store(true, Ordering::Relaxed);
        if let Some(handle) = alsa_thread.take() {
            reap_alsa_capture_thread(handle);
        }
        return Err(e);
    }

    Ok(AudioStreamHandle {
        statistics,
        sample_rate: input_sample_rate,
        channels: input_channels,
        alsa_capture_thread: alsa_thread,
        capture_failure_receiver,
        stop_capture,
        stop_playback,
        volume_bits,
        _output_stream_guard: output_stream_guard,
        _output_stream_handle: stream_handle,
    })
}

#[allow(clippy::too_many_arguments)]
fn start_alsa_capture(
    source_name: &str,
    peak_amplitude_shared: Arc<AtomicU64>,
    capture_delay_ms: Arc<AtomicU64>,
    stop_capture: Arc<AtomicBool>,
    buffer_size: u32,
    sample_rate: u32,
    sample_format: String,
    request_repaint: Arc<dyn Fn() + Send + Sync>,
    replay: Arc<crate::replay::AudioTap>,
) -> Result<AlsaCapture> {
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
        let c = (hwp_probe.set_channels_near(2)? as u16).max(1);
        (r, c)
    };

    // Size the SPSC bridge using the actual negotiated rate and channel count (~2s / >= 12 periods).
    let ring_frames = (rate.max(48_000) as usize * 2).max(buffer_size as usize * 12);
    let (mut producer, consumer) = RingBuffer::<f32>::new(ring_frames * channels as usize);

    let samples_captured = Arc::new(AtomicU64::new(0));
    let source_name_owned = source_name.to_string();
    let (capture_failure_sender, capture_failure_receiver) = crossbeam_channel::bounded(1);

    let handle = thread::spawn(move || {
        let mut capture_progress = AudioCaptureProgress::new();
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
                &mut capture_progress,
                &replay,
            );

            if stop_capture.load(Ordering::Relaxed) {
                break;
            }

            if let Err(error) = result {
                tracing::warn!("ALSA capture stopped ({}); reopening the device", error);
                if capture_progress.stalled_for(ALSA_RECOVERY_TIMEOUT) {
                    let message = format!(
                        "ALSA capture produced no samples for {:?}",
                        ALSA_RECOVERY_TIMEOUT
                    );
                    tracing::error!("{}; stopping the stream", message);
                    let _ = capture_failure_sender.try_send(message);
                    request_repaint();
                    break;
                }
            }
            sleep_until_reconnect(&stop_capture);
        }
        tracing::debug!("ALSA capture thread stopped.");
    });

    Ok(AlsaCapture {
        thread: handle,
        failure_receiver: capture_failure_receiver,
        consumer,
        channels,
        sample_rate: rate,
    })
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
    capture_progress: &mut AudioCaptureProgress,
    replay: &crate::replay::AudioTap,
) -> Result<()> {
    use alsa::pcm::{Access, HwParams, PCM};
    use alsa::Direction;

    let pcm = PCM::new(source_name, Direction::Capture, true)
        .with_context(|| format!("Failed to open ALSA device {source_name}"))?;
    let hwp = HwParams::any(&pcm)?;
    hwp.set_access(Access::RWInterleaved)?;
    hwp.set_format(pcm_format)?;
    let actual_rate = hwp.set_rate_near(rate, alsa::ValueOr::Nearest)?;
    let actual_channels = hwp.set_channels_near(channels as u32)? as u16;
    if actual_channels != channels {
        anyhow::bail!(
            "ALSA device {source_name} negotiated {actual_channels} channels (expected {channels})"
        );
    }
    let actual_period = hwp
        .set_period_size_near(buffer_size as alsa::pcm::Frames, alsa::ValueOr::Nearest)
        .unwrap_or(buffer_size as alsa::pcm::Frames)
        .max(1);
    // Request at least 4 periods (~200 ms floor) of kernel ring-buffer capacity so USB/OS
    // scheduling jitter cannot cause silent ALSA capture overruns (EPIPE). Wakeup cadence is
    // still driven by `actual_period`, so extra kernel capacity adds zero capture latency.
    let target_alsa_buffer =
        (actual_period.saturating_mul(4)).max((actual_rate / 5) as alsa::pcm::Frames);
    let _ = hwp.set_buffer_size_near(target_alsa_buffer);
    pcm.hw_params(&hwp)?;
    drop(hwp);

    let read_frames = (actual_period as usize).max(buffer_size as usize).max(1);

    match sample_format {
        "S32LE" => run_alsa_capture_loop(
            &pcm,
            pcm.io_i32()?,
            producer,
            peak_amplitude_shared,
            capture_delay_ms,
            stop_capture,
            read_frames,
            actual_rate,
            channels,
            samples_captured,
            capture_progress,
            replay,
            |sample| (sample as f64 / 2_147_483_648.0) as f32,
        ),
        "F32LE" => run_alsa_capture_loop(
            &pcm,
            pcm.io_f32()?,
            producer,
            peak_amplitude_shared,
            capture_delay_ms,
            stop_capture,
            read_frames,
            actual_rate,
            channels,
            samples_captured,
            capture_progress,
            replay,
            |sample| sample,
        ),
        _ => run_alsa_capture_loop(
            &pcm,
            pcm.io_i16()?,
            producer,
            peak_amplitude_shared,
            capture_delay_ms,
            stop_capture,
            read_frames,
            actual_rate,
            channels,
            samples_captured,
            capture_progress,
            replay,
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
    read_frames: usize,
    rate: u32,
    channels: u16,
    samples_captured: &Arc<AtomicU64>,
    capture_progress: &mut AudioCaptureProgress,
    replay: &crate::replay::AudioTap,
    convert: F,
) -> Result<()>
where
    S: Copy + Default,
    F: Fn(S) -> f32,
{
    replay.mark_discontinuity();
    let initial_filter = replay.filter.read().map(|c| c.clone()).unwrap_or_default();
    let mut dsp = crate::devices::dsp_filter::LiveAudioDsp::new(&initial_filter, rate);
    let mut dc_blocker = DcBlocker::new(rate, channels);

    let buffer_samples = read_frames * channels as usize;
    let mut input_buf = vec![S::default(); buffer_samples];
    let mut converted_buf = vec![0.0f32; buffer_samples];
    while !stop_capture.load(Ordering::Relaxed) {
        if let Ok(delay_frames) = pcm.delay() {
            let delay_ms = (delay_frames.max(0) as f64 / rate as f64 * 1000.0) as u64;
            capture_delay_ms.store(delay_ms, Ordering::Relaxed);
        }

        match io.readi(&mut input_buf) {
            Ok(frames) => {
                let sample_count = frames * channels as usize;
                if sample_count == 0 {
                    if capture_progress.stalled_for(ALSA_STALL_TIMEOUT) {
                        return Err(anyhow!(
                            "ALSA capture produced no samples for {:?}",
                            ALSA_STALL_TIMEOUT
                        ));
                    }
                    match pcm.wait(Some(ALSA_CAPTURE_POLL_TIMEOUT_MS)) {
                        Ok(_) => {}
                        Err(wait_error) => {
                            replay.mark_discontinuity();
                            dsp.reset();
                            dc_blocker.reset();
                            recover_alsa_capture(pcm, wait_error)?;
                        }
                    }
                    continue;
                }
                capture_progress.mark_samples();
                for (output, input) in converted_buf
                    .iter_mut()
                    .zip(input_buf.iter())
                    .take(sample_count)
                {
                    *output = convert(*input);
                }

                dc_blocker.process_interleaved(&mut converted_buf[..sample_count]);

                if let Ok(filter_config) = replay.filter.try_read() {
                    dsp.reconfigure(&filter_config, rate);
                }
                if dsp.enabled {
                    dsp.process_interleaved(&mut converted_buf[..sample_count], channels as usize);
                }

                let mut local_max = 0.0f32;
                for sample in &converted_buf[..sample_count] {
                    local_max = local_max.max(sample.abs());
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
                // Playback receives samples first; disabled replay performs no extra ALSA query.
                replay.observe_reactive(&converted_buf[..sample_count], rate, channels);
                if replay.is_enabled() {
                    let delay = pcm.delay().unwrap_or(0).max(0);
                    let at =
                        crate::replay::now_us() - (delay + frames as i64) * 1_000_000 / rate as i64;
                    replay.submit(&converted_buf[..sample_count], rate, channels, at);
                }
            }
            Err(error) if error.errno() == alsa::nix::errno::Errno::EAGAIN => {
                if capture_progress.stalled_for(ALSA_STALL_TIMEOUT) {
                    return Err(anyhow!(
                        "ALSA capture produced no samples for {:?}",
                        ALSA_STALL_TIMEOUT
                    ));
                }
                match pcm.wait(Some(ALSA_CAPTURE_POLL_TIMEOUT_MS)) {
                    Ok(_) => {}
                    Err(wait_error) => {
                        replay.mark_discontinuity();
                        dsp.reset();
                        dc_blocker.reset();
                        recover_alsa_capture(pcm, wait_error)?;
                    }
                }
            }
            Err(error) => {
                replay.mark_discontinuity();
                dsp.reset();
                dc_blocker.reset();
                recover_alsa_capture(pcm, error)?;
            }
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

const SHARED_PLAYBACK_IDLE_TIMEOUT: Duration = Duration::from_millis(250);

struct PlaybackCoordinatorState<H> {
    active_count: usize,
    handle: Option<H>,
    opening: bool,
    generation: u64,
    last_open_error: Option<String>,
}

struct PlaybackCoordinator<H> {
    mutex: std::sync::Mutex<PlaybackCoordinatorState<H>>,
    cv: std::sync::Condvar,
    idle_timeout: Duration,
}

/// RAII guard keeping the shared process-wide Rodio output stream alive.
///
/// While at least one `SharedPlaybackGuard` exists, all audio subsystems share
/// the same underlying `rodio::OutputStreamHandle` (and its internal software mixer)
/// so only a single application output stream appears in the desktop audio mixer.
/// When all guards are dropped, the output stream closes after a brief grace period.
pub struct SharedPlaybackGuard<H: Clone + Send + 'static = rodio::OutputStreamHandle> {
    coord: Arc<PlaybackCoordinator<H>>,
}

impl<H: Clone + Send + 'static> Drop for SharedPlaybackGuard<H> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.coord.mutex.lock() {
            state.active_count = state.active_count.saturating_sub(1);
            if state.active_count == 0 {
                self.coord.cv.notify_all();
            }
        }
    }
}

impl<H: Clone + Send + 'static> PlaybackCoordinator<H> {
    fn acquire(self: &Arc<Self>) -> Result<(SharedPlaybackGuard<H>, H)> {
        let mut state = self.mutex.lock().unwrap();

        if let Some(handle) = state.handle.clone() {
            state.active_count += 1;
            return Ok((
                SharedPlaybackGuard {
                    coord: Arc::clone(self),
                },
                handle,
            ));
        }

        state.active_count += 1;
        let wait_gen = state.generation;
        state.opening = true;
        self.cv.notify_all();

        while state.handle.is_none() && state.generation == wait_gen {
            state = self.cv.wait(state).unwrap();
        }

        if let Some(ref handle) = state.handle {
            Ok((
                SharedPlaybackGuard {
                    coord: Arc::clone(self),
                },
                handle.clone(),
            ))
        } else {
            state.active_count = state.active_count.saturating_sub(1);
            if state.active_count == 0 {
                self.cv.notify_all();
            }
            let err_msg = state
                .last_open_error
                .clone()
                .unwrap_or_else(|| "Failed to open audio output stream".to_string());
            Err(anyhow!(err_msg))
        }
    }
}

fn spawn_playback_coordinator<S, H, F>(
    idle_timeout: Duration,
    mut open_fn: F,
) -> Arc<PlaybackCoordinator<H>>
where
    H: Clone + Send + 'static,
    F: FnMut() -> Result<(S, H)> + Send + 'static,
{
    let coord = Arc::new(PlaybackCoordinator {
        mutex: std::sync::Mutex::new(PlaybackCoordinatorState {
            active_count: 0,
            handle: None,
            opening: false,
            generation: 0,
            last_open_error: None,
        }),
        cv: std::sync::Condvar::new(),
        idle_timeout,
    });

    let owner_coord = Arc::clone(&coord);
    let _ = thread::Builder::new()
        .name("michadame-audio-output".to_string())
        .spawn(move || {
            let mut current_stream: Option<S> = None;
            let mut state = owner_coord.mutex.lock().unwrap();

            loop {
                if state.opening && current_stream.is_none() {
                    state.opening = false;
                    drop(state);

                    let result = open_fn();

                    state = owner_coord.mutex.lock().unwrap();
                    state.generation = state.generation.wrapping_add(1);

                    match result {
                        Ok((stream, handle)) => {
                            if state.active_count > 0 {
                                current_stream = Some(stream);
                                state.handle = Some(handle);
                                state.last_open_error = None;
                            } else {
                                state.handle = None;
                                state.last_open_error = None;
                                drop(state);
                                drop(stream);
                                state = owner_coord.mutex.lock().unwrap();
                            }
                        }
                        Err(e) => {
                            state.handle = None;
                            state.last_open_error = Some(format!("{e:#}"));
                        }
                    }
                    owner_coord.cv.notify_all();
                    continue;
                }

                if state.active_count == 0 && current_stream.is_some() {
                    let (new_state, timeout_res) = owner_coord
                        .cv
                        .wait_timeout(state, owner_coord.idle_timeout)
                        .unwrap();
                    state = new_state;
                    if state.active_count == 0 && timeout_res.timed_out() {
                        state.handle = None;
                        let stream_to_drop = current_stream.take();
                        drop(state);
                        drop(stream_to_drop);
                        tracing::debug!("Closed idle shared Rodio output stream");
                        state = owner_coord.mutex.lock().unwrap();
                    }
                    continue;
                }

                if state.active_count == 0
                    && !state.opening
                    && current_stream.is_none()
                    && Arc::strong_count(&owner_coord) == 1
                {
                    break;
                }

                state = owner_coord.cv.wait(state).unwrap();
            }
        });

    coord
}

fn select_preferred_48khz_config(
    configs: impl IntoIterator<Item = cpal::SupportedStreamConfigRange>,
) -> Option<cpal::SupportedStreamConfig> {
    configs
        .into_iter()
        .filter(|range| {
            range.min_sample_rate() <= PREFERRED_OUTPUT_SAMPLE_RATE
                && PREFERRED_OUTPUT_SAMPLE_RATE <= range.max_sample_rate()
        })
        .max_by(|a, b| a.cmp_default_heuristics(b))
        .and_then(|best| best.try_with_sample_rate(PREFERRED_OUTPUT_SAMPLE_RATE))
}

fn prioritized_output_device_rank(name: &str) -> Option<u8> {
    let n = name.trim().to_lowercase();
    if n == "pipewire" {
        Some(4)
    } else if n == "pulse" {
        Some(3)
    } else if n.contains("pipewire") {
        Some(2)
    } else if n.contains("pulse") {
        Some(1)
    } else {
        None
    }
}

fn open_device_preferring_48khz(
    device: &cpal::Device,
) -> Result<(OutputStream, rodio::OutputStreamHandle)> {
    // CPAL's Linux ALSA backend hardcodes 44,100 Hz in `default_output_config()` whenever
    // 44.1 kHz lies in [min_rate, max_rate], forcing 48 kHz capture audio through Rodio's linear
    // resampler. Explicitly request 48,000 Hz stereo F32 when supported, falling back cleanly.
    if let Ok(supported_configs) = device.supported_output_configs() {
        if let Some(config_48k) = select_preferred_48khz_config(supported_configs) {
            if let Ok(stream) = OutputStream::try_from_device_config(device, config_48k) {
                return Ok(stream);
            }
        }
    }

    OutputStream::try_from_device(device).context("Failed to open output stream on device")
}

fn open_hardware_output_stream() -> Result<(OutputStream, rodio::OutputStreamHandle)> {
    // On Linux, the "default" host is ALSA, but for output we want to hit the sound server
    // (PipeWire / PulseAudio) at 48 kHz rather than opening raw hardware at 44.1 kHz.
    let host = cpal::default_host();
    let mut prioritized = Vec::new();

    if let Ok(devices) = host.output_devices() {
        for device in devices {
            if let Ok(name) = device.name() {
                if let Some(rank) = prioritized_output_device_rank(&name) {
                    prioritized.push((rank, name, device));
                }
            }
        }
    }

    prioritized.sort_by(|(rank_a, name_a, _), (rank_b, name_b, _)| {
        rank_b.cmp(rank_a).then_with(|| name_a.cmp(name_b))
    });

    for (_, name, device) in prioritized {
        match open_device_preferring_48khz(&device) {
            Ok(stream) => {
                tracing::info!("Opened prioritized output device: {}", name);
                return Ok(stream);
            }
            Err(err) => {
                tracing::debug!(
                    "Could not open prioritized output device {}: {:#}",
                    name,
                    err
                );
            }
        }
    }

    if let Some(default_device) = host.default_output_device() {
        if let Ok(stream) = open_device_preferring_48khz(&default_device) {
            tracing::info!("Opened default output device at preferred rate");
            return Ok(stream);
        }
    }

    tracing::info!("Using system default output stream fallback");
    OutputStream::try_default().context("Failed to open default output stream")
}

pub(crate) fn initialize_rodio_playback() -> Result<(SharedPlaybackGuard, rodio::OutputStreamHandle)>
{
    if cfg!(test) {
        return Err(anyhow!(
            "Hardware audio playback is disabled during unit tests"
        ));
    }

    static SHARED_PLAYBACK: std::sync::OnceLock<
        Arc<PlaybackCoordinator<rodio::OutputStreamHandle>>,
    > = std::sync::OnceLock::new();

    let coord = SHARED_PLAYBACK.get_or_init(|| {
        spawn_playback_coordinator(SHARED_PLAYBACK_IDLE_TIMEOUT, open_hardware_output_stream)
    });

    coord.acquire()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_live_source(
        consumer: Consumer<f32>,
        channels: u16,
        sample_rate: u32,
        buffer_size: u32,
    ) -> LiveSource {
        LiveSource::new(
            Arc::new(PlaybackStatistics::default()),
            consumer,
            channels,
            sample_rate,
            buffer_size,
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU32::new(1.0f32.to_bits())),
        )
    }

    fn push_frames(producer: &mut Producer<f32>, frames: usize, left: f32, right: f32) {
        for _ in 0..frames {
            producer.push(left).unwrap();
            producer.push(right).unwrap();
        }
    }

    #[test]
    fn audio_capture_progress_expires_and_recovers_on_real_samples() {
        let mut progress = AudioCaptureProgress {
            last_sample_at: Instant::now() - ALSA_RECOVERY_TIMEOUT,
        };
        assert!(progress.stalled_for(ALSA_RECOVERY_TIMEOUT));

        progress.mark_samples();
        assert!(!progress.stalled_for(ALSA_RECOVERY_TIMEOUT));
    }

    #[test]
    fn reaping_a_stuck_audio_worker_does_not_block_the_caller() {
        let handle = thread::spawn(|| thread::sleep(Duration::from_secs(1)));
        let started = Instant::now();

        reap_alsa_capture_thread(handle);

        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn startup_prebuffers_cushion_and_survives_21ms_capture_vs_25ms_playback_bursts() {
        // Real-world capture card cadence: 48 kHz stereo, 1024-frame (21.33 ms) capture periods
        // feeding CPAL's 1200-frame (25.0 ms) synchronous output callbacks.
        let sample_rate = 48_000u32;
        let buffer_size = 1024u32;
        let (mut producer, consumer) = RingBuffer::<f32>::new(48_000 * 2 * 2);
        let mut source = make_test_live_source(consumer, 2, sample_rate, buffer_size);

        let capture_period = buffer_size as usize; // 1024 frames (21.333 ms)
        let playback_burst = 1200usize; // 25.000 ms
        let mut next_capture_frame_due = capture_period;
        let mut total_output_frames = 0usize;

        // Simulate 10 full seconds (480,000 stereo frames) of asynchronous bursty streaming.
        let total_callbacks = (sample_rate as usize * 10) / playback_burst;
        for _ in 0..total_callbacks {
            while next_capture_frame_due <= total_output_frames {
                for i in 0..capture_period {
                    let phase =
                        2.0 * std::f32::consts::PI * 440.0 * (i as f32) / sample_rate as f32;
                    let s = 0.25 * phase.sin();
                    producer.push(s).unwrap();
                    producer.push(-s).unwrap();
                }
                next_capture_frame_due += capture_period;
            }

            for _ in 0..(playback_burst * 2) {
                assert!(source.next().is_some());
            }
            total_output_frames += playback_burst;
        }

        assert!(source.has_started_playback);
        assert_eq!(
            source.statistics.silence_frames.load(Ordering::Relaxed),
            0,
            "steady-state capture/playback bursts must never underrun once prebuffered"
        );
        assert_eq!(
            source
                .statistics
                .drift_dropped_frames
                .load(Ordering::Relaxed),
            0,
            "matched clocks must never drop frames"
        );
    }

    #[test]
    fn adaptive_resampler_tracks_positive_and_negative_clock_skew_without_underruns_or_drops() {
        // Verify both -1000 ppm (capture slower than playback, which previously caused ~1 Hz
        // underrun ticks) and +1000 ppm (capture faster than playback, which previously caused
        // periodic hard frame drops) over 60 simulated seconds each.
        for skew_ppm in [-1000.0f64, 1000.0f64] {
            let sample_rate = 48_000u32;
            let buffer_size = 1024u32;
            let (mut producer, consumer) = RingBuffer::<f32>::new(48_000 * 2 * 2);
            let mut source = make_test_live_source(consumer, 2, sample_rate, buffer_size);

            let capture_period = buffer_size as usize;
            let playback_burst = 1200usize;
            let capture_rate = sample_rate as f64 * (1.0 + skew_ppm * 1e-6);

            let mut captured_frames_total = 0usize;
            let total_callbacks = (sample_rate as usize * 60) / playback_burst; // 60 seconds

            for cb in 0..total_callbacks {
                let elapsed_sec = (cb * playback_burst) as f64 / sample_rate as f64;
                let produced_by_now = (elapsed_sec * capture_rate) as usize;
                while captured_frames_total + capture_period <= produced_by_now {
                    push_frames(&mut producer, capture_period, 0.1, -0.1);
                    captured_frames_total += capture_period;
                }

                for _ in 0..(playback_burst * 2) {
                    let s = source.next().unwrap();
                    assert!(s.is_finite());
                }
            }

            assert!(source.has_started_playback);
            assert_eq!(
                source.statistics.silence_frames.load(Ordering::Relaxed),
                0,
                "skew {skew_ppm} ppm caused underruns"
            );
            assert_eq!(
                source
                    .statistics
                    .drift_dropped_frames
                    .load(Ordering::Relaxed),
                0,
                "skew {skew_ppm} ppm caused hard frame drops"
            );
        }
    }

    #[test]
    fn dc_offset_silence_produces_zero_impulse_across_capture_and_playback() {
        // Typical USB HDMI capture card DC bias during silent game screen (~+64 LSB in S16LE).
        let dc_bias = 64.0f32 / 32768.0;
        let mut dc_blocker = DcBlocker::new(48_000, 2);
        let (mut producer, consumer) = RingBuffer::<f32>::new(48_000 * 2 * 2);
        let mut source = make_test_live_source(consumer, 2, 48_000, 1024);

        let mut period_buf = vec![dc_bias; 1024 * 2];
        let mut max_abs_output = 0.0f32;

        // Run through startup, steady state, an intentional 100ms capture stall (underrun), and
        // recovery; output must remain completely silent (0.0) with zero "tic" impulse.
        for step in 0..120 {
            if !(50..55).contains(&step) {
                period_buf.fill(dc_bias);
                dc_blocker.process_interleaved(&mut period_buf);
                for &sample in &period_buf {
                    producer.push(sample).unwrap();
                }
            }
            for _ in 0..(1200 * 2) {
                let out = source.next().unwrap();
                max_abs_output = max_abs_output.max(out.abs());
            }
        }

        assert_eq!(
            max_abs_output, 0.0,
            "constant DC bias during silence must never produce a click or impulse"
        );
    }

    #[test]
    fn underrun_fades_smoothly_and_rebuilds_prebuffer_before_resuming() {
        let (mut producer, consumer) = RingBuffer::<f32>::new(16_384);
        let mut source = make_test_live_source(consumer, 2, 48_000, 1024);
        let prebuffer = source.prebuffer_frames;

        // Startup prebuffer holds non-blocking 1ms silence chunks until `prebuffer_frames` arrive.
        assert_eq!(source.next(), Some(0.0));
        assert_eq!(source.valid_len, 96, "startup poll chunk should cover 1 ms");
        assert_eq!(
            source.statistics.silence_frames.load(Ordering::Relaxed),
            0,
            "initial startup prebuffering should not report mid-stream underruns"
        );
        for _ in 1..source.valid_len {
            assert_eq!(source.next(), Some(0.0));
        }

        // Fill prebuffer and drain all queued audio until genuine starvation occurs.
        push_frames(&mut producer, prebuffer, 0.8, -0.8);
        let mut last_left = 0.0f32;
        while source.consumer.slots() > 0 || source.local_idx < source.valid_len {
            last_left = source.next().unwrap();
            let _right = source.next().unwrap();
        }
        assert!((last_left - 0.8).abs() < 1e-4);

        // Starvation enters a smooth 5ms (240-frame) C1 raised-cosine fade-out rather than
        // snapping or V-notching over 1ms.
        let fade_first_left = source.next().unwrap();
        let fade_first_right = source.next().unwrap();
        assert!(fade_first_left > 0.79 && fade_first_left < 0.8);
        assert!(fade_first_right < -0.79 && fade_first_right > -0.8);
        let fade_frames = source.declick_frame_count();
        assert_eq!(fade_frames, 240);
        for _ in 1..fade_frames {
            source.next();
            source.next();
        }
        assert!(source.last_output_frame[0].abs() < 1e-6);
        assert_eq!(
            source.statistics.silence_frames.load(Ordering::Relaxed),
            fade_frames as u64
        );

        // Partial refill below `prebuffer_frames` stays in 1ms silence hold instead of prematurely
        // draining and re-underrunning on the next callback.
        push_frames(&mut producer, prebuffer / 2, 0.5, -0.5);
        assert_eq!(source.next(), Some(0.0));
        for _ in 1..96 {
            assert_eq!(source.next(), Some(0.0));
        }
        assert_eq!(source.consumer.slots(), (prebuffer / 2) * 2);

        // Completing the prebuffer cushion resumes playback with a smooth raised-cosine fade-in.
        push_frames(&mut producer, prebuffer, 0.5, -0.5);
        let resume_first_left = source.next().unwrap();
        let resume_first_right = source.next().unwrap();
        assert!(resume_first_left > 0.0 && resume_first_left < 0.01);
        assert!(resume_first_right < 0.0 && resume_first_right > -0.01);
    }

    #[test]
    fn emergency_clock_drift_skip_is_declicked_and_restores_cushion() {
        let (mut producer, consumer) = RingBuffer::<f32>::new(48_000 * 2);
        let mut source = make_test_live_source(consumer, 2, 48_000, 1024);
        source.buffering = false;
        source.has_started_playback = true;
        source.declick_next_audio = false;
        source.last_output_frame = vec![1.0, -1.0];

        let excessive_frames = source.max_drift_frames + 2_000;
        push_frames(&mut producer, excessive_frames, 0.25, -0.25);

        let first_left = source.next().unwrap();
        let first_right = source.next().unwrap();

        assert!(first_left < 1.0 && first_left > 0.25);
        assert!(first_right > -1.0 && first_right < -0.25);
        assert_eq!(
            source.statistics.queued_frames.load(Ordering::Relaxed),
            excessive_frames as u64
        );
        assert_eq!(
            source
                .statistics
                .drift_dropped_frames
                .load(Ordering::Relaxed),
            (excessive_frames - source.prebuffer_frames) as u64
        );
        assert_eq!(source.statistics.silence_frames.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn live_source_stops_on_frame_boundary_and_reports_unbounded_size_hint() {
        let (mut producer, consumer) = RingBuffer::<f32>::new(16_384);
        let mut source = make_test_live_source(consumer, 2, 48_000, 256);
        let stop_flag = Arc::clone(&source.stop_playback);
        let vol_bits = Arc::clone(&source.volume_bits);

        assert_eq!(source.size_hint(), (usize::MAX, None));

        push_frames(&mut producer, source.prebuffer_frames + 64, 0.5, -0.5);
        vol_bits.store(0.5f32.to_bits(), Ordering::Relaxed);

        let left = source.next().unwrap();
        // Setting stop mid-frame must still yield the right channel before terminating cleanly.
        stop_flag.store(true, Ordering::Relaxed);
        let right = source.next();
        assert!(
            right.is_some(),
            "must complete stereo frame before stopping"
        );
        assert!(left.is_finite());
        assert_eq!(source.next(), None);
        assert_eq!(source.size_hint(), (0, Some(0)));
    }

    #[test]
    fn preferred_output_config_selects_48khz_stereo_f32_over_44_1khz_default() {
        let ranges = vec![
            cpal::SupportedStreamConfigRange::new(
                2,
                cpal::SampleRate(44_100),
                cpal::SampleRate(192_000),
                cpal::SupportedBufferSize::Unknown,
                cpal::SampleFormat::I16,
            ),
            cpal::SupportedStreamConfigRange::new(
                2,
                cpal::SampleRate(8_000),
                cpal::SampleRate(192_000),
                cpal::SupportedBufferSize::Unknown,
                cpal::SampleFormat::F32,
            ),
        ];

        let chosen = select_preferred_48khz_config(ranges).expect("48 kHz config should be chosen");
        assert_eq!(chosen.channels(), 2);
        assert_eq!(chosen.sample_rate(), cpal::SampleRate(48_000));
        assert_eq!(chosen.sample_format(), cpal::SampleFormat::F32);

        assert_eq!(prioritized_output_device_rank("pipewire"), Some(4));
        assert_eq!(prioritized_output_device_rank("pulse"), Some(3));
        assert!(prioritized_output_device_rank("hw:CARD=PCH,DEV=0").is_none());
    }

    #[test]
    fn test_is_usable_input_device_filtering() {
        use alsa::Direction;

        // Valid input devices
        assert!(is_usable_input_device(
            "pulse",
            "PulseAudio Sound Server",
            None
        ));
        assert!(is_usable_input_device(
            "pipewire",
            "PipeWire Sound Server",
            None
        ));
        assert!(is_usable_input_device(
            "default",
            "Default Audio Device",
            None
        ));
        assert!(is_usable_input_device(
            "default:CARD=PCH",
            "HDA Intel PCH, ALC1220 Analog\nDefault Audio Device",
            None
        ));
        assert!(is_usable_input_device(
            "default:CARD=U25773",
            "UGREEN 25773, USB Audio\nDefault Audio Device",
            Some(Direction::Capture)
        ));
        assert!(is_usable_input_device(
            "front:CARD=PCH,DEV=0",
            "HDA Intel PCH, ALC1220 Analog\nFront output / input",
            None
        ));

        // Output-only direction
        assert!(!is_usable_input_device(
            "anything",
            "Any device",
            Some(Direction::Playback)
        ));

        // Output descriptions
        assert!(!is_usable_input_device(
            "hdmi:CARD=HDMI,DEV=0",
            "HDA ATI HDMI, LG ULTRAFINE\nHDMI Audio Output",
            None
        ));
        assert!(!is_usable_input_device(
            "iec958:CARD=PCH,DEV=0",
            "HDA Intel PCH\nIEC958 (S/PDIF) Digital Audio Output",
            None
        ));
        assert!(!is_usable_input_device(
            "surround51:CARD=PCH,DEV=0",
            "5.1 Surround output to Front, Center, Rear and Subwoofer speakers",
            None
        ));
        assert!(!is_usable_input_device(
            "usbstream:CARD=PCH",
            "USB Stream Output",
            None
        ));

        // ALSA plugins & internal endpoints
        assert!(!is_usable_input_device("null", "Discard all samples", None));
        assert!(!is_usable_input_device(
            "samplerate",
            "Rate Converter Plugin",
            None
        ));
        assert!(!is_usable_input_device(
            "speexrate",
            "Rate Converter Plugin",
            None
        ));
        assert!(!is_usable_input_device(
            "lavrate",
            "Rate Converter Plugin",
            None
        ));
        assert!(!is_usable_input_device(
            "upmix",
            "Plugin for channel upmix",
            None
        ));
        assert!(!is_usable_input_device(
            "vdownmix",
            "Plugin for channel downmix",
            None
        ));
        assert!(!is_usable_input_device(
            "jack",
            "JACK Audio Connection Kit",
            None
        ));
        assert!(!is_usable_input_device("oss", "Open Sound System", None));

        // Duplicates
        assert!(!is_usable_input_device(
            "sysdefault:CARD=PCH",
            "Default Audio Device",
            None
        ));
    }

    #[test]
    fn test_find_audio_devices_only_returns_usable_inputs() {
        use alsa::Direction;
        let synthetic_hints = vec![
            (
                "pulse".to_string(),
                "PulseAudio Sound Server".to_string(),
                None,
            ),
            (
                "pipewire".to_string(),
                "PipeWire Sound Server".to_string(),
                None,
            ),
            (
                "default:CARD=PCH".to_string(),
                "HDA Intel PCH, ALC1220 Analog\nDefault Audio Device".to_string(),
                Some(Direction::Capture),
            ),
            (
                "front:CARD=PCH,DEV=0".to_string(),
                "HDA Intel PCH, ALC1220 Analog\nFront output / input".to_string(),
                None,
            ),
            (
                "sysdefault:CARD=PCH".to_string(),
                "HDA Intel PCH\nDefault Audio Device".to_string(),
                None,
            ),
            (
                "surround51:CARD=PCH,DEV=0".to_string(),
                "5.1 Surround output to Front, Center, Rear and Subwoofer speakers".to_string(),
                None,
            ),
            (
                "iec958:CARD=PCH,DEV=0".to_string(),
                "HDA Intel PCH\nIEC958 (S/PDIF) Digital Audio Output".to_string(),
                None,
            ),
            (
                "hdmi:CARD=HDMI,DEV=0".to_string(),
                "HDA ATI HDMI\nHDMI Audio Output".to_string(),
                None,
            ),
            (
                "usbstream:CARD=PCH".to_string(),
                "USB Stream Output".to_string(),
                None,
            ),
            ("null".to_string(), "Discard all samples".to_string(), None),
            (
                "samplerate".to_string(),
                "Rate Converter Plugin".to_string(),
                None,
            ),
        ];

        let devices = filter_usable_audio_hints(synthetic_hints, |_| true);
        assert_eq!(devices.len(), 3);

        for (desc, name) in &devices {
            let n = name.to_lowercase();
            let d = desc.to_lowercase();

            assert!(
                !n.starts_with("surround"),
                "Surround output device listed: {}",
                name
            );
            assert!(
                !n.starts_with("iec958"),
                "IEC958 output device listed: {}",
                name
            );
            assert!(
                !n.starts_with("hdmi"),
                "HDMI output device listed: {}",
                name
            );
            assert!(
                !n.starts_with("usbstream"),
                "USBStream output device listed: {}",
                name
            );
            assert!(
                !n.starts_with("sysdefault"),
                "Duplicate sysdefault listed: {}",
                name
            );
            assert!(
                n != "null"
                    && n != "samplerate"
                    && n != "speexrate"
                    && n != "upmix"
                    && n != "vdownmix",
                "Plugin listed: {}",
                name
            );
            assert!(
                !d.contains("output to")
                    && !d.contains("digital audio output")
                    && !d.contains("hdmi audio output"),
                "Output-only description listed: {}",
                desc
            );
        }
    }

    #[test]
    fn shared_playback_coordinator_shares_single_stream_and_closes_when_idle() {
        let open_count = Arc::new(AtomicU64::new(0));
        let drop_count = Arc::new(AtomicU64::new(0));

        struct MockStream {
            drop_count: Arc<AtomicU64>,
        }
        impl Drop for MockStream {
            fn drop(&mut self) {
                self.drop_count.fetch_add(1, Ordering::SeqCst);
            }
        }

        let open_count_clone = Arc::clone(&open_count);
        let drop_count_clone = Arc::clone(&drop_count);
        let coord = spawn_playback_coordinator(Duration::from_millis(30), move || {
            let id = open_count_clone.fetch_add(1, Ordering::SeqCst) + 1;
            Ok((
                MockStream {
                    drop_count: Arc::clone(&drop_count_clone),
                },
                id,
            ))
        });

        // No stream is opened until the first caller acquires a guard
        assert_eq!(open_count.load(Ordering::SeqCst), 0);
        assert_eq!(drop_count.load(Ordering::SeqCst), 0);

        // First caller opens stream #1
        let (guard1, handle1) = coord.acquire().unwrap();
        assert_eq!(handle1, 1);
        assert_eq!(open_count.load(Ordering::SeqCst), 1);

        // Concurrent callers share stream #1 without opening a second stream
        let (guard2, handle2) = coord.acquire().unwrap();
        let (guard3, handle3) = coord.acquire().unwrap();
        assert_eq!(handle2, 1);
        assert_eq!(handle3, 1);
        assert_eq!(open_count.load(Ordering::SeqCst), 1);
        assert_eq!(drop_count.load(Ordering::SeqCst), 0);

        // Dropping two of the three guards leaves stream #1 open
        drop(guard2);
        drop(guard3);
        thread::sleep(Duration::from_millis(50));
        assert_eq!(drop_count.load(Ordering::SeqCst), 0);

        // Dropping the last guard and re-acquiring within the idle grace window reuses stream #1
        drop(guard1);
        let (guard_reused, handle_reused) = coord.acquire().unwrap();
        assert_eq!(handle_reused, 1);
        assert_eq!(open_count.load(Ordering::SeqCst), 1);
        assert_eq!(drop_count.load(Ordering::SeqCst), 0);

        // Dropping the last guard and waiting past the idle timeout closes stream #1
        drop(guard_reused);
        let deadline = Instant::now() + Duration::from_millis(500);
        while drop_count.load(Ordering::SeqCst) < 1 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(drop_count.load(Ordering::SeqCst), 1);

        // Acquiring again after idle close opens stream #2
        let (guard_new, handle_new) = coord.acquire().unwrap();
        assert_eq!(handle_new, 2);
        assert_eq!(open_count.load(Ordering::SeqCst), 2);
        drop(guard_new);
    }

    #[test]
    fn shared_playback_coordinator_propagates_open_failure_and_recovers() {
        let attempts = Arc::new(AtomicU64::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let coord = spawn_playback_coordinator(Duration::from_millis(20), move || {
            let n = attempts_clone.fetch_add(1, Ordering::SeqCst) + 1;
            if n == 1 {
                Err(anyhow!("Simulated ALSA device busy"))
            } else {
                Ok(((), n))
            }
        });

        let err = coord.acquire().err().expect("First attempt should fail");
        assert!(err.to_string().contains("Simulated ALSA device busy"));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);

        // Subsequent acquisition succeeds cleanly
        let (_guard, handle) = coord.acquire().unwrap();
        assert_eq!(handle, 2);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
