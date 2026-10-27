use crate::video::decoder::{copy_frame_tightly, renderable_pixel_format};
use crate::video::types::{RawFrame, ScalerFilter};
use anyhow::{Context, Result};
use rodio::{Sink, Source};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const AUDIO_OUTPUT_RATE: u32 = 48000;

pub(crate) fn create_audio_resampler(
    format: ffmpeg_next::format::Sample,
    layout: ffmpeg_next::ChannelLayout,
    rate: u32,
) -> Option<ffmpeg_next::software::resampling::context::Context> {
    ffmpeg_next::software::resampling::context::Context::get(
        format,
        layout,
        rate,
        ffmpeg_next::format::Sample::F32(ffmpeg_next::format::sample::Type::Packed),
        ffmpeg_next::ChannelLayout::STEREO,
        AUDIO_OUTPUT_RATE,
    )
    .ok()
}

/// Resamples one decoded frame to 48 kHz packed stereo f32.
///
/// `Context::run` with an empty output frame allocates only `input.samples()` of
/// capacity. When upsampling (e.g. 44.1 kHz -> 48 kHz) swr then can't emit everything
/// it produced and silently buffers the rest, so the backlog grows by ~81 ms for every
/// second of 44.1 kHz audio. That delays all audio relative to its PTS and carries stale
/// audio across seeks. Allocating room for the pending delay plus the converted input
/// lets swr drain completely on every call.
pub(crate) fn resample_audio_frame(
    rsm: &mut ffmpeg_next::software::resampling::context::Context,
    input: &ffmpeg_next::frame::Audio,
) -> Option<ffmpeg_next::frame::Audio> {
    let in_rate = i64::from(if input.rate() > 0 {
        input.rate()
    } else {
        rsm.input().rate
    })
    .max(1);
    let pending = rsm.delay().map(|d| d.output).unwrap_or(0).max(0);
    let converted = (input.samples() as i64 * i64::from(AUDIO_OUTPUT_RATE) + in_rate - 1) / in_rate;
    let capacity = (pending + converted + 256) as usize;
    let mut output = ffmpeg_next::frame::Audio::new(
        ffmpeg_next::format::Sample::F32(ffmpeg_next::format::sample::Type::Packed),
        capacity,
        ffmpeg_next::ChannelLayout::STEREO,
    );
    rsm.run(input, &mut output).ok()?;
    Some(output)
}

/// Flushes any remaining delayed samples buffered inside the `SwrContext` at EOF.
pub(crate) fn flush_audio_resampler(
    rsm: &mut ffmpeg_next::software::resampling::context::Context,
) -> Option<ffmpeg_next::frame::Audio> {
    let pending = rsm.delay().map(|d| d.output).unwrap_or(0).max(0);
    let capacity = (pending + 256) as usize;
    let mut output = ffmpeg_next::frame::Audio::new(
        ffmpeg_next::format::Sample::F32(ffmpeg_next::format::sample::Type::Packed),
        capacity,
        ffmpeg_next::ChannelLayout::STEREO,
    );
    rsm.flush(&mut output).ok()?;
    if output.samples() > 0 {
        Some(output)
    } else {
        None
    }
}

const AUDIO_OUTPUT_CHANNELS: u16 = 2;
const MAX_VIDEO_BUFFER_FRAMES: usize = 32;
const MAX_CONSECUTIVE_PACKET_READ_ERRORS: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PacketReadErrorAction {
    DrainAndEof,
    RetryTransient,
    AbortDemux,
}

pub(crate) fn classify_packet_read_error(
    err: &ffmpeg_next::Error,
    consecutive_errors: &mut u32,
) -> PacketReadErrorAction {
    match err {
        ffmpeg_next::Error::Eof => PacketReadErrorAction::DrainAndEof,
        ffmpeg_next::Error::Other { errno } if *errno == libc::EAGAIN => {
            PacketReadErrorAction::RetryTransient
        }
        _ => {
            *consecutive_errors = consecutive_errors.saturating_add(1);
            if *consecutive_errors >= MAX_CONSECUTIVE_PACKET_READ_ERRORS {
                PacketReadErrorAction::AbortDemux
            } else {
                PacketReadErrorAction::RetryTransient
            }
        }
    }
}

pub(crate) fn apply_seek_if_supported<F>(
    target_us: i64,
    video_stream_index: i32,
    video_time_base: ffmpeg_next::Rational,
    mut seek_fn: F,
) -> bool
where
    F: FnMut(i32, i64) -> i32,
{
    if seek_fn(-1, target_us) >= 0 {
        return true;
    }
    let target_ts = unsafe {
        ffmpeg_next::sys::av_rescale_q(
            target_us,
            ffmpeg_next::Rational(1, 1_000_000).into(),
            video_time_base.into(),
        )
    };
    seek_fn(video_stream_index, target_ts) >= 0
}

/// Formats a time in seconds to "MM:SS" or "HH:MM:SS".
pub fn format_time(seconds: f64) -> String {
    if seconds.is_nan() || seconds <= 0.0 {
        return "00:00".to_string();
    }
    let total_secs = seconds.round() as u64;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, minutes, secs)
    } else {
        format!("{:02}:{:02}", minutes, secs)
    }
}

/// Formats a time in seconds with context of total duration (includes HH if duration >= 3600).
pub fn format_time_with_duration(seconds: f64, duration_seconds: f64) -> String {
    let show_hours = duration_seconds >= 3600.0;
    if seconds.is_nan() || seconds <= 0.0 {
        return if show_hours {
            "00:00:00".to_string()
        } else {
            "00:00".to_string()
        };
    }
    let total_secs = seconds.round() as u64;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if show_hours || hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, minutes, secs)
    } else {
        format!("{:02}:{:02}", minutes, secs)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayerCommand {
    Play,
    Pause,
    TogglePause,
    Seek(f64),
    Stop,
    SetVolume(f32),
}

#[derive(Debug, Clone)]
enum DemuxCommand {
    Seek {
        target_us: i64,
        ack: crossbeam_channel::Sender<bool>,
    },
    Stop,
}

#[derive(Debug, Clone)]
pub enum PlayerEvent {
    Started {
        width: u32,
        height: u32,
        duration_secs: f64,
        framerate: f64,
    },
    Eof,
    Failed(String),
    Stopped,
}

pub struct VideoFileConfig {
    pub file_path: PathBuf,
    pub scaler_filter: Arc<AtomicU8>,
    pub color_range: Arc<AtomicU8>,
    pub stop_requested: Arc<AtomicBool>,
    pub request_repaint: Arc<dyn Fn() + Send + Sync>,
    pub peak_amplitude: Arc<AtomicU64>,
    pub audio_latency_ms: Arc<AtomicU64>,
    pub replay_audio: Arc<crate::replay::AudioTap>,
    pub master_volume: f32,
}

pub(crate) struct SharedAudioQueue {
    samples: VecDeque<f32>,
    /// PTS in microseconds of the sample currently at the front of `samples`.
    head_pts_us: i64,
    /// Total samples played since last seek / PTS sync.
    samples_consumed: u64,
    /// Generation token for the queue to discard stale samples across seeks.
    generation: u64,
}

impl SharedAudioQueue {
    fn new() -> Self {
        Self {
            samples: VecDeque::with_capacity(AUDIO_OUTPUT_RATE as usize * 2),
            head_pts_us: 0,
            samples_consumed: 0,
            generation: 0,
        }
    }

    fn clear(&mut self, new_pts_us: i64, new_gen: u64) {
        self.samples.clear();
        self.head_pts_us = new_pts_us;
        self.samples_consumed = 0;
        self.generation = new_gen;
    }

    pub(crate) fn end_pts_us(&self) -> i64 {
        let channels = AUDIO_OUTPUT_CHANNELS as u64;
        let total_samples = self.samples_consumed + self.samples.len() as u64;
        let total_frames = total_samples / channels;
        let elapsed_us = (total_frames as f64 / AUDIO_OUTPUT_RATE as f64 * 1_000_000.0) as i64;
        self.head_pts_us + elapsed_us
    }

    pub(crate) fn has_unplayed_audio(
        &mut self,
        current_gen: u64,
        sink_active: bool,
        fallback_clock_us: i64,
    ) -> bool {
        if self.generation < current_gen {
            self.samples.clear();
            self.generation = current_gen;
            return false;
        }
        if self.samples.is_empty() {
            return false;
        }
        if sink_active || fallback_clock_us < self.end_pts_us() {
            true
        } else {
            self.samples.clear();
            false
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn pack_decoded_video_frame(
    frame: &ffmpeg_next::frame::Video,
    normalized: &mut ffmpeg_next::frame::Video,
    scaler: &mut ffmpeg_next::software::scaling::context::Context,
    output_format: ffmpeg_next::format::Pixel,
    dec_width: u32,
    dec_height: u32,
    color_range: &AtomicU8,
    replay_rate: crate::replay::config::Rate,
) -> Option<Arc<RawFrame>> {
    let packed = if frame.format() == output_format
        && frame.width() == dec_width
        && frame.height() == dec_height
    {
        copy_frame_tightly(frame)
            .ok()
            .map(|data| (frame.width(), frame.height(), frame.format(), data))
    } else if scaler.run(frame, normalized).is_ok() {
        copy_frame_tightly(normalized).ok().map(|data| {
            (
                normalized.width(),
                normalized.height(),
                normalized.format(),
                data,
            )
        })
    } else {
        None
    };

    packed.map(|(w, h, fmt, data)| {
        let range_val = color_range.load(Ordering::Relaxed);
        Arc::new(RawFrame {
            captured_at: crate::replay::now_us(),
            rate: replay_rate,
            width: w,
            height: h,
            data,
            format: fmt,
            color_range: crate::video::types::ColorRange::from_u8(range_val),
        })
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn receive_and_enqueue_video_frames(
    video_decoder: &mut ffmpeg_next::decoder::Video,
    scaler: &mut ffmpeg_next::software::scaling::context::Context,
    decoded: &mut ffmpeg_next::frame::Video,
    normalized: &mut ffmpeg_next::frame::Video,
    output_format: ffmpeg_next::format::Pixel,
    video_time_base: ffmpeg_next::Rational,
    video_frame_interval_us: i64,
    video_fallback_pts: &mut i64,
    pending_video_seek_target_us: &mut Option<i64>,
    pending_audio_seek_target_us: &mut Option<i64>,
    last_video_frame: &mut Option<(i64, ffmpeg_next::frame::Video)>,
    color_range: &AtomicU8,
    replay_rate: crate::replay::config::Rate,
    video_queue: &Mutex<VecDeque<(i64, Arc<RawFrame>, u64)>>,
    gen: u64,
    flush_eof: bool,
) {
    if flush_eof {
        let _ = video_decoder.send_eof();
    }
    let dec_w = video_decoder.width();
    let dec_h = video_decoder.height();
    while video_decoder.receive_frame(decoded).is_ok() {
        let frame_pts = decoded
            .pts()
            .map(|pts| unsafe {
                ffmpeg_next::sys::av_rescale_q(
                    pts,
                    video_time_base.into(),
                    ffmpeg_next::Rational(1, 1_000_000).into(),
                )
            })
            .unwrap_or_else(|| {
                *video_fallback_pts += video_frame_interval_us;
                *video_fallback_pts
            });
        *video_fallback_pts = frame_pts;

        if let Some(target) = *pending_video_seek_target_us {
            if frame_pts < target {
                // Preroll: skip scaling & memory allocation, keep candidate ref in case EOF
                *last_video_frame = Some((frame_pts, decoded.clone()));
                continue;
            } else {
                // Reached seek target!
                *pending_video_seek_target_us = None;
                *last_video_frame = None;
            }
        }

        if let Some(raw_frame) = pack_decoded_video_frame(
            decoded,
            normalized,
            scaler,
            output_format,
            dec_w,
            dec_h,
            color_range,
            replay_rate,
        ) {
            if let Ok(mut vq) = video_queue.lock() {
                vq.push_back((frame_pts, raw_frame, gen));
            }
        }
    }

    if flush_eof {
        if let Some((pts, frame)) = last_video_frame.take() {
            *pending_video_seek_target_us = None;
            *pending_audio_seek_target_us = None;
            if let Some(raw_frame) = pack_decoded_video_frame(
                &frame,
                normalized,
                scaler,
                output_format,
                dec_w,
                dec_h,
                color_range,
                replay_rate,
            ) {
                if let Ok(mut vq) = video_queue.lock() {
                    vq.push_back((pts, raw_frame, gen));
                }
            }
        }
    }
}

fn enqueue_resampled_audio(
    resampled_frame: &ffmpeg_next::frame::Audio,
    pts_us: Option<i64>,
    audio_fallback_pts: &mut i64,
    pending_audio_seek_target_us: &mut Option<i64>,
    audio_queue: &Mutex<SharedAudioQueue>,
    gen: u64,
) {
    let samples_count = resampled_frame.samples();
    if samples_count == 0 {
        return;
    }
    let slice = resampled_frame.plane::<f32>(0);
    let pts_us = pts_us.unwrap_or(*audio_fallback_pts);
    let duration_us = (samples_count as f64 / AUDIO_OUTPUT_RATE as f64 * 1_000_000.0) as i64;
    let frame_end_us = pts_us + duration_us;
    *audio_fallback_pts = frame_end_us;

    if let Some(target) = *pending_audio_seek_target_us {
        if frame_end_us <= target {
            // Entire audio frame is before target; discard
            return;
        }
        if pts_us < target {
            // Audio frame spans across target; trim leading samples
            let skip_us = target - pts_us;
            let skip_frames =
                (skip_us as f64 / 1_000_000.0 * AUDIO_OUTPUT_RATE as f64).round() as usize;
            let skip_samples = (skip_frames * AUDIO_OUTPUT_CHANNELS as usize).min(slice.len());
            let trimmed_slice = &slice[skip_samples..];
            if !trimmed_slice.is_empty() {
                if let Ok(mut q) = audio_queue.lock() {
                    if q.generation == gen {
                        if q.samples.is_empty() {
                            q.head_pts_us = target;
                            q.samples_consumed = 0;
                        }
                        q.samples.extend(trimmed_slice);
                    }
                }
            }
            *pending_audio_seek_target_us = None;
            return;
        }
        *pending_audio_seek_target_us = None;
    }

    if let Ok(mut q) = audio_queue.lock() {
        if q.generation == gen {
            if q.samples.is_empty() {
                q.head_pts_us = pts_us;
                q.samples_consumed = 0;
            }
            q.samples.extend(slice);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn receive_and_enqueue_audio_frames(
    dec: &mut ffmpeg_next::decoder::Audio,
    rsm: &mut ffmpeg_next::software::resampling::context::Context,
    audio_time_base: ffmpeg_next::Rational,
    resampler_needs_reset: &mut bool,
    audio_fallback_pts: &mut i64,
    pending_audio_seek_target_us: &mut Option<i64>,
    audio_queue: &Mutex<SharedAudioQueue>,
    gen: u64,
    flush_eof: bool,
) {
    if flush_eof {
        let _ = dec.send_eof();
    }
    let mut audio_frame = ffmpeg_next::frame::Audio::empty();
    while dec.receive_frame(&mut audio_frame).is_ok() {
        if *resampler_needs_reset {
            *resampler_needs_reset = false;
            let layout = if audio_frame.channel_layout().is_empty() {
                ffmpeg_next::ChannelLayout::default(audio_frame.channels().max(1) as i32)
            } else {
                audio_frame.channel_layout()
            };
            let rate = if audio_frame.rate() > 0 {
                audio_frame.rate()
            } else {
                rsm.input().rate
            };
            if let Some(fresh) = create_audio_resampler(audio_frame.format(), layout, rate) {
                *rsm = fresh;
            }
        }
        if let Some(resampled_frame) = resample_audio_frame(rsm, &audio_frame) {
            let pts_us = audio_frame.pts().map(|pts| unsafe {
                ffmpeg_next::sys::av_rescale_q(
                    pts,
                    audio_time_base.into(),
                    ffmpeg_next::Rational(1, 1_000_000).into(),
                )
            });
            enqueue_resampled_audio(
                &resampled_frame,
                pts_us,
                audio_fallback_pts,
                pending_audio_seek_target_us,
                audio_queue,
                gen,
            );
        }
    }
    if flush_eof {
        while let Some(flushed_frame) = flush_audio_resampler(rsm) {
            enqueue_resampled_audio(
                &flushed_frame,
                None,
                audio_fallback_pts,
                pending_audio_seek_target_us,
                audio_queue,
                gen,
            );
        }
    }
}

struct VideoFileAudioSource {
    queue: Arc<Mutex<SharedAudioQueue>>,
    current_pts_shared: Arc<AtomicI64>,
    seek_generation: Arc<AtomicU64>,
    peak_amplitude: Arc<AtomicU64>,
    audio_latency_ms: Arc<AtomicU64>,
    replay_audio: Arc<crate::replay::AudioTap>,
    local_batch: Vec<f32>,
    read_buf: VecDeque<f32>,
    local_gen: u64,
}

impl Iterator for VideoFileAudioSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let mut report_batch = false;
        let current_gen = self.seek_generation.load(Ordering::SeqCst);
        if self.local_gen != current_gen {
            self.read_buf.clear();
            self.local_gen = current_gen;
        }

        if self.read_buf.is_empty() {
            let channels = AUDIO_OUTPUT_CHANNELS as usize;
            if let Ok(mut q) = self.queue.try_lock() {
                if q.generation < current_gen {
                    q.samples.clear();
                    q.generation = current_gen;
                } else {
                    let count = (q.samples.len() / channels).min(16) * channels;
                    if count > 0 {
                        self.read_buf.extend(q.samples.drain(..count));
                        q.samples_consumed += count as u64;
                        let played_frames = q.samples_consumed / channels as u64;
                        let elapsed_us =
                            (played_frames as f64 / AUDIO_OUTPUT_RATE as f64 * 1_000_000.0) as i64;
                        let current_pts = q.head_pts_us + elapsed_us;
                        self.current_pts_shared
                            .store(current_pts, Ordering::Relaxed);
                    }
                }
            }

            if self.read_buf.is_empty() {
                for _ in 0..channels {
                    self.read_buf.push_back(0.0);
                }
            }
        }

        let sample = self.read_buf.pop_front().unwrap_or(0.0);
        self.local_batch.push(sample);

        // Batch report to audio meter and replay tap
        if self.local_batch.len() >= 512 {
            report_batch = true;
        }

        if report_batch {
            let mut max_abs = 0.0f32;
            for &s in &self.local_batch {
                let abs = s.abs();
                if abs > max_abs {
                    max_abs = abs;
                }
            }
            let peak_bits = (max_abs * 1000.0) as u64;
            self.peak_amplitude.fetch_max(peak_bits, Ordering::Relaxed);
            self.audio_latency_ms.store(25, Ordering::Relaxed);

            self.replay_audio.observe_reactive(
                &self.local_batch,
                AUDIO_OUTPUT_RATE,
                AUDIO_OUTPUT_CHANNELS,
            );

            if self.replay_audio.is_enabled() {
                self.replay_audio.submit(
                    &self.local_batch,
                    AUDIO_OUTPUT_RATE,
                    AUDIO_OUTPUT_CHANNELS,
                    crate::replay::now_us(),
                );
            }
            self.local_batch.clear();
        }

        Some(sample)
    }
}

impl Source for VideoFileAudioSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        AUDIO_OUTPUT_CHANNELS
    }

    fn sample_rate(&self) -> u32 {
        AUDIO_OUTPUT_RATE
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

struct MonotonicClock {
    base_pts_us: i64,
    start_instant: Option<Instant>,
}

impl MonotonicClock {
    fn new(base_pts_us: i64) -> Self {
        Self {
            base_pts_us,
            start_instant: Some(Instant::now()),
        }
    }

    fn now_pts_us(&self) -> i64 {
        if let Some(start) = self.start_instant {
            self.base_pts_us + start.elapsed().as_micros() as i64
        } else {
            self.base_pts_us
        }
    }

    fn pause(&mut self) {
        self.base_pts_us = self.now_pts_us();
        self.start_instant = None;
    }

    fn resume(&mut self) {
        if self.start_instant.is_none() {
            self.start_instant = Some(Instant::now());
        }
    }

    fn seek(&mut self, target_us: i64) {
        self.base_pts_us = target_us;
        if self.start_instant.is_some() {
            self.start_instant = Some(Instant::now());
        }
    }
}

/// Metadata extracted from a video file without starting full playback.
#[derive(Debug, Clone)]
pub struct VideoFileInfo {
    pub width: u32,
    pub height: u32,
    pub duration_secs: f64,
    pub framerate: f64,
}

/// Probes a video file container and decoder parameters to determine native dimensions and duration.
pub fn probe_video_file(path: &Path) -> Result<VideoFileInfo> {
    ffmpeg_next::init().context("Failed to initialize FFmpeg")?;
    let ictx = ffmpeg_next::format::input(path)
        .with_context(|| format!("Failed to open video file at {:?}", path))?;
    let video_stream = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .context("Could not find video stream in file")?;
    let video_time_base = video_stream.time_base();
    let video_duration = video_stream.duration();
    let raw_rate = video_stream.avg_frame_rate();
    let framerate = if raw_rate.numerator() > 0 && raw_rate.denominator() > 0 {
        raw_rate.numerator() as f64 / raw_rate.denominator() as f64
    } else {
        30.0
    };

    let container_duration = ictx.duration();
    let file_duration_us = if container_duration > 0 {
        container_duration
    } else if video_duration > 0 {
        unsafe {
            ffmpeg_next::sys::av_rescale_q(
                video_duration,
                video_time_base.into(),
                ffmpeg_next::Rational(1, 1_000_000).into(),
            )
        }
    } else {
        0
    };
    let duration_secs = file_duration_us as f64 / 1_000_000.0;

    let video_decoder =
        ffmpeg_next::codec::context::Context::from_parameters(video_stream.parameters())?
            .decoder()
            .video()
            .context("Failed to create video decoder")?;
    let width = video_decoder.width();
    let height = video_decoder.height();

    Ok(VideoFileInfo {
        width,
        height,
        duration_secs,
        framerate,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn video_file_thread_main(
    frame_sender: crossbeam_channel::Sender<Arc<RawFrame>>,
    status_sender: crossbeam_channel::Sender<PlayerEvent>,
    command_receiver: crossbeam_channel::Receiver<PlayerCommand>,
    position_us: Arc<AtomicI64>,
    is_paused_shared: Arc<AtomicBool>,
    is_eof_shared: Arc<AtomicBool>,
    config: VideoFileConfig,
) -> Result<()> {
    ffmpeg_next::init().context("Failed to initialize FFmpeg")?;

    let VideoFileConfig {
        file_path,
        scaler_filter,
        color_range,
        stop_requested,
        request_repaint,
        peak_amplitude,
        audio_latency_ms,
        replay_audio,
        master_volume,
    } = config;

    let mut ictx = ffmpeg_next::format::input(&file_path)
        .with_context(|| format!("Failed to open video file at {:?}", file_path))?;

    let video_stream = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .context("Could not find video stream in file")?;
    let video_stream_index = video_stream.index();
    let video_time_base = video_stream.time_base();
    let video_duration = video_stream.duration();
    let raw_rate = video_stream.avg_frame_rate();
    let framerate = if raw_rate.numerator() > 0 && raw_rate.denominator() > 0 {
        raw_rate.numerator() as f64 / raw_rate.denominator() as f64
    } else {
        30.0
    };
    let replay_rate = crate::replay::config::Rate::new((framerate.round() as u32).max(1), 1);

    // Compute duration in microseconds
    let container_duration = ictx.duration();
    let file_duration_us = if container_duration > 0 {
        container_duration
    } else if video_duration > 0 {
        unsafe {
            ffmpeg_next::sys::av_rescale_q(
                video_duration,
                video_time_base.into(),
                ffmpeg_next::Rational(1, 1_000_000).into(),
            )
        }
    } else {
        0
    };
    let duration_secs = file_duration_us as f64 / 1_000_000.0;

    let mut video_decoder =
        ffmpeg_next::codec::context::Context::from_parameters(video_stream.parameters())?
            .decoder()
            .video()
            .context("Failed to create video decoder")?;
    video_decoder.set_threading(ffmpeg_next::codec::threading::Config::default());

    let width = video_decoder.width();
    let height = video_decoder.height();

    // Check for audio stream
    let audio_info = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Audio)
        .map(|s| (s.index(), s.time_base(), s.parameters()));

    let (mut audio_decoder, audio_stream_index, audio_time_base, mut resampler) =
        if let Some((idx, time_base, params)) = audio_info {
            match ffmpeg_next::codec::context::Context::from_parameters(params)
                .and_then(|c| c.decoder().audio())
            {
                Ok(decoder) => {
                    let ch_layout = if decoder.channel_layout().is_empty() {
                        ffmpeg_next::ChannelLayout::default(decoder.channels().max(1) as i32)
                    } else {
                        decoder.channel_layout()
                    };
                    let resampler =
                        create_audio_resampler(decoder.format(), ch_layout, decoder.rate());
                    (Some(decoder), Some(idx), Some(time_base), resampler)
                }
                Err(e) => {
                    tracing::warn!("Failed to open audio stream: {}", e);
                    (None, None, None, None)
                }
            }
        } else {
            (None, None, None, None)
        };

    let has_audio = audio_decoder.is_some() && resampler.is_some();
    let audio_queue = Arc::new(Mutex::new(SharedAudioQueue::new()));
    let audio_clock_pts_us = Arc::new(AtomicI64::new(0));
    let seek_generation = Arc::new(AtomicU64::new(0));

    // Initialize Rodio playback for audio if audio exists
    let mut _audio_guard = None;
    let mut audio_sink = None;
    if has_audio {
        match crate::devices::audio::initialize_rodio_playback() {
            Ok((guard, stream_handle)) => match Sink::try_new(&stream_handle) {
                Ok(sink) => {
                    let audio_source = VideoFileAudioSource {
                        queue: audio_queue.clone(),
                        current_pts_shared: audio_clock_pts_us.clone(),
                        seek_generation: seek_generation.clone(),
                        peak_amplitude: peak_amplitude.clone(),
                        audio_latency_ms: audio_latency_ms.clone(),
                        replay_audio: replay_audio.clone(),
                        local_batch: Vec::with_capacity(512),
                        read_buf: VecDeque::with_capacity(512),
                        local_gen: 0,
                    };
                    sink.append(audio_source);
                    sink.set_volume(master_volume);
                    sink.play();
                    audio_sink = Some(sink);
                    _audio_guard = Some(guard);
                }
                Err(e) => tracing::warn!("Failed to create audio sink: {}", e),
            },
            Err(e) => tracing::warn!("Failed to initialize audio playback: {}", e),
        }
    }

    let _ = status_sender.send(PlayerEvent::Started {
        width,
        height,
        duration_secs,
        framerate,
    });
    request_repaint();

    let (demux_cmd_tx, demux_cmd_rx) = crossbeam_channel::unbounded::<DemuxCommand>();
    let video_queue = Arc::new(Mutex::new(
        VecDeque::<(i64, Arc<RawFrame>, u64)>::with_capacity(32),
    ));
    let is_demux_eof = Arc::new(AtomicBool::new(false));
    let demux_stop_flag = Arc::new(AtomicBool::new(false));
    let video_frame_interval_us = (1_000_000.0 / framerate.max(1.0)) as i64;

    // Spawn independent background Demuxer thread (Producer)
    let demux_handle = {
        let video_queue = Arc::clone(&video_queue);
        let audio_queue = Arc::clone(&audio_queue);
        let audio_clock_pts_us = Arc::clone(&audio_clock_pts_us);
        let is_demux_eof = Arc::clone(&is_demux_eof);
        let demux_stop_flag = Arc::clone(&demux_stop_flag);
        let seek_generation = Arc::clone(&seek_generation);
        let scaler_filter = Arc::clone(&scaler_filter);
        let color_range = Arc::clone(&color_range);

        thread::spawn(move || {
            let mut current_scaler_val = scaler_filter.load(Ordering::Relaxed);
            let output_format = renderable_pixel_format(video_decoder.format());
            let mut scaler = match ffmpeg_next::software::scaling::context::Context::get(
                video_decoder.format(),
                video_decoder.width(),
                video_decoder.height(),
                output_format,
                video_decoder.width(),
                video_decoder.height(),
                ScalerFilter::from_u8(current_scaler_val).into_ffmpeg_flag(),
            ) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Failed to create software scaler: {}", e);
                    return;
                }
            };
            let mut demux_eof = false;
            let mut consecutive_read_errors = 0u32;
            let mut audio_fallback_pts = 0i64;
            let mut video_fallback_pts = 0i64;
            let mut pending_video_seek_target_us: Option<i64> = None;
            let mut pending_audio_seek_target_us: Option<i64> = None;
            let mut resampler_needs_reset = false;
            let mut last_video_frame: Option<(i64, ffmpeg_next::frame::Video)> = None;
            let mut decoded = ffmpeg_next::frame::Video::empty();
            let mut normalized = ffmpeg_next::frame::Video::empty();

            loop {
                if demux_stop_flag.load(Ordering::Relaxed) {
                    break;
                }

                // Handle demuxer commands
                while let Ok(cmd) = demux_cmd_rx.try_recv() {
                    match cmd {
                        DemuxCommand::Seek {
                            mut target_us,
                            mut ack,
                        } => {
                            // Coalesce consecutive Seek commands
                            while let Ok(next) = demux_cmd_rx.try_recv() {
                                match next {
                                    DemuxCommand::Seek {
                                        target_us: newer_us,
                                        ack: newer_ack,
                                    } => {
                                        let _ = ack.send(false);
                                        target_us = newer_us;
                                        ack = newer_ack;
                                    }
                                    DemuxCommand::Stop => {
                                        let _ = ack.send(false);
                                        return;
                                    }
                                }
                            }

                            // Seek container streams with -1; fallback to video stream index if needed
                            let seek_succeeded = apply_seek_if_supported(
                                target_us,
                                video_stream_index as i32,
                                video_time_base,
                                |stream_idx, ts| unsafe {
                                    ffmpeg_next::sys::av_seek_frame(
                                        ictx.as_mut_ptr(),
                                        stream_idx,
                                        ts,
                                        ffmpeg_next::sys::AVSEEK_FLAG_BACKWARD,
                                    )
                                },
                            );

                            if !seek_succeeded {
                                tracing::warn!(
                                    "Failed to seek video container to {}us; preserving current playback state",
                                    target_us
                                );
                                let _ = ack.send(false);
                                continue;
                            }

                            let gen = seek_generation.fetch_add(1, Ordering::SeqCst) + 1;

                            video_decoder.flush();
                            if let Some(ref mut ad) = audio_decoder {
                                ad.flush();
                            }
                            // Discard any audio still buffered inside swr from the old position:
                            // the resampler is rebuilt from the first decoded frame after the seek.
                            resampler_needs_reset = true;

                            if let Ok(mut q) = audio_queue.lock() {
                                q.clear(target_us, gen);
                            }
                            audio_clock_pts_us.store(target_us, Ordering::Relaxed);
                            audio_fallback_pts = target_us;
                            video_fallback_pts = target_us;

                            if let Ok(mut vq) = video_queue.lock() {
                                vq.clear();
                            }

                            pending_video_seek_target_us = Some(target_us);
                            pending_audio_seek_target_us = Some(target_us);
                            last_video_frame = None;
                            consecutive_read_errors = 0;
                            demux_eof = false;
                            is_demux_eof.store(false, Ordering::Release);
                            let _ = ack.send(true);
                        }
                        DemuxCommand::Stop => {
                            return;
                        }
                    }
                }

                if demux_stop_flag.load(Ordering::Relaxed) {
                    break;
                }

                // Check scaler changes
                let new_scaler_val = scaler_filter.load(Ordering::Relaxed);
                if new_scaler_val != current_scaler_val {
                    current_scaler_val = new_scaler_val;
                    if let Ok(s) = ffmpeg_next::software::scaling::context::Context::get(
                        video_decoder.format(),
                        video_decoder.width(),
                        video_decoder.height(),
                        output_format,
                        video_decoder.width(),
                        video_decoder.height(),
                        ScalerFilter::from_u8(current_scaler_val).into_ffmpeg_flag(),
                    ) {
                        scaler = s;
                    }
                }

                // Throttle check: sleep only when both queues are adequately filled
                let (v_len, a_len) = {
                    let v = video_queue.lock().map(|q| q.len()).unwrap_or(0);
                    let a = audio_queue.lock().map(|q| q.samples.len()).unwrap_or(0);
                    (v, a)
                };

                let video_ample = v_len >= 24;
                let audio_ample = !has_audio || a_len >= AUDIO_OUTPUT_RATE as usize;

                if (video_ample && audio_ample) || v_len >= MAX_VIDEO_BUFFER_FRAMES {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }

                if demux_eof {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }

                let mut packet = ffmpeg_next::Packet::empty();
                match packet.read(&mut ictx) {
                    Ok(()) => {
                        consecutive_read_errors = 0;
                    }
                    Err(err) => {
                        match classify_packet_read_error(&err, &mut consecutive_read_errors) {
                            PacketReadErrorAction::RetryTransient => {
                                thread::sleep(Duration::from_millis(2));
                                continue;
                            }
                            PacketReadErrorAction::AbortDemux => {
                                tracing::warn!(
                                    "Stopping demux after {} consecutive packet read errors (last: {})",
                                    consecutive_read_errors,
                                    err
                                );
                                let gen = seek_generation.load(Ordering::SeqCst);
                                receive_and_enqueue_video_frames(
                                    &mut video_decoder,
                                    &mut scaler,
                                    &mut decoded,
                                    &mut normalized,
                                    output_format,
                                    video_time_base,
                                    video_frame_interval_us,
                                    &mut video_fallback_pts,
                                    &mut pending_video_seek_target_us,
                                    &mut pending_audio_seek_target_us,
                                    &mut last_video_frame,
                                    &color_range,
                                    replay_rate,
                                    &video_queue,
                                    gen,
                                    true,
                                );
                                if let (Some(ref mut dec), Some(ref mut rsm), Some(tb)) =
                                    (&mut audio_decoder, &mut resampler, audio_time_base)
                                {
                                    receive_and_enqueue_audio_frames(
                                        dec,
                                        rsm,
                                        tb,
                                        &mut resampler_needs_reset,
                                        &mut audio_fallback_pts,
                                        &mut pending_audio_seek_target_us,
                                        &audio_queue,
                                        gen,
                                        true,
                                    );
                                }
                                demux_eof = true;
                                is_demux_eof.store(true, Ordering::Release);
                                continue;
                            }
                            PacketReadErrorAction::DrainAndEof => {
                                let gen = seek_generation.load(Ordering::SeqCst);
                                receive_and_enqueue_video_frames(
                                    &mut video_decoder,
                                    &mut scaler,
                                    &mut decoded,
                                    &mut normalized,
                                    output_format,
                                    video_time_base,
                                    video_frame_interval_us,
                                    &mut video_fallback_pts,
                                    &mut pending_video_seek_target_us,
                                    &mut pending_audio_seek_target_us,
                                    &mut last_video_frame,
                                    &color_range,
                                    replay_rate,
                                    &video_queue,
                                    gen,
                                    true,
                                );
                                if let (Some(ref mut dec), Some(ref mut rsm), Some(tb)) =
                                    (&mut audio_decoder, &mut resampler, audio_time_base)
                                {
                                    receive_and_enqueue_audio_frames(
                                        dec,
                                        rsm,
                                        tb,
                                        &mut resampler_needs_reset,
                                        &mut audio_fallback_pts,
                                        &mut pending_audio_seek_target_us,
                                        &audio_queue,
                                        gen,
                                        true,
                                    );
                                }
                                demux_eof = true;
                                is_demux_eof.store(true, Ordering::Release);
                                continue;
                            }
                        }
                    }
                }

                let stream_idx = packet.stream();
                let gen = seek_generation.load(Ordering::SeqCst);

                if stream_idx == video_stream_index {
                    if video_decoder.send_packet(&packet).is_ok() {
                        receive_and_enqueue_video_frames(
                            &mut video_decoder,
                            &mut scaler,
                            &mut decoded,
                            &mut normalized,
                            output_format,
                            video_time_base,
                            video_frame_interval_us,
                            &mut video_fallback_pts,
                            &mut pending_video_seek_target_us,
                            &mut pending_audio_seek_target_us,
                            &mut last_video_frame,
                            &color_range,
                            replay_rate,
                            &video_queue,
                            gen,
                            false,
                        );
                    }
                } else if Some(stream_idx) == audio_stream_index {
                    if let (Some(ref mut dec), Some(ref mut rsm), Some(tb)) =
                        (&mut audio_decoder, &mut resampler, audio_time_base)
                    {
                        if dec.send_packet(&packet).is_ok() {
                            receive_and_enqueue_audio_frames(
                                dec,
                                rsm,
                                tb,
                                &mut resampler_needs_reset,
                                &mut audio_fallback_pts,
                                &mut pending_audio_seek_target_us,
                                &audio_queue,
                                gen,
                                false,
                            );
                        }
                    }
                }
            }
        })
    };

    // Presenter loop (Consumer & Synchronizer)
    let mut mono_clock = MonotonicClock::new(0);
    let mut last_audio_clock = 0i64;
    let mut last_audio_adv_time = Instant::now();
    let mut first_frame_after_seek = true;
    let mut seek_stabilize_until = Instant::now();

    let request_demux_seek = |target_us: i64| -> bool {
        let (ack_tx, ack_rx) = crossbeam_channel::bounded(1);
        if demux_cmd_tx
            .send(DemuxCommand::Seek {
                target_us,
                ack: ack_tx,
            })
            .is_err()
        {
            return false;
        }
        ack_rx.recv_timeout(Duration::from_secs(2)).unwrap_or(false)
    };

    while !stop_requested.load(Ordering::Relaxed) {
        // Handle incoming UI commands
        let mut next_cmd = command_receiver.try_recv().ok();
        while let Some(cmd) = next_cmd.take() {
            match cmd {
                PlayerCommand::Play => {
                    if is_eof_shared.load(Ordering::Relaxed) && request_demux_seek(0) {
                        first_frame_after_seek = true;
                        mono_clock.seek(0);
                        position_us.store(0, Ordering::Relaxed);
                        last_audio_clock = 0;
                        seek_stabilize_until = Instant::now() + Duration::from_millis(300);
                        is_eof_shared.store(false, Ordering::Relaxed);
                    }
                    is_paused_shared.store(false, Ordering::Relaxed);
                    if let Some(ref sink) = audio_sink {
                        sink.play();
                    }
                    mono_clock.resume();
                    last_audio_adv_time = Instant::now();
                    request_repaint();
                }
                PlayerCommand::Pause => {
                    is_paused_shared.store(true, Ordering::Relaxed);
                    if let Some(ref sink) = audio_sink {
                        sink.pause();
                    }
                    mono_clock.pause();
                    request_repaint();
                }
                PlayerCommand::TogglePause => {
                    let paused = is_paused_shared.load(Ordering::Relaxed);
                    if paused {
                        if is_eof_shared.load(Ordering::Relaxed) && request_demux_seek(0) {
                            first_frame_after_seek = true;
                            mono_clock.seek(0);
                            position_us.store(0, Ordering::Relaxed);
                            last_audio_clock = 0;
                            seek_stabilize_until = Instant::now() + Duration::from_millis(300);
                            is_eof_shared.store(false, Ordering::Relaxed);
                        }
                        is_paused_shared.store(false, Ordering::Relaxed);
                        if let Some(ref sink) = audio_sink {
                            sink.play();
                        }
                        mono_clock.resume();
                        last_audio_adv_time = Instant::now();
                    } else {
                        is_paused_shared.store(true, Ordering::Relaxed);
                        if let Some(ref sink) = audio_sink {
                            sink.pause();
                        }
                        mono_clock.pause();
                    }
                    request_repaint();
                }
                PlayerCommand::Stop => {
                    if request_demux_seek(0) {
                        first_frame_after_seek = true;
                        mono_clock.seek(0);
                        position_us.store(0, Ordering::Relaxed);
                        last_audio_clock = 0;
                        seek_stabilize_until = Instant::now() + Duration::from_millis(300);
                        is_eof_shared.store(false, Ordering::Relaxed);
                    }
                    is_paused_shared.store(true, Ordering::Relaxed);
                    if let Some(ref sink) = audio_sink {
                        sink.pause();
                    }
                    mono_clock.pause();
                    request_repaint();
                }
                PlayerCommand::SetVolume(vol) => {
                    if let Some(ref sink) = audio_sink {
                        sink.set_volume(vol);
                    }
                }
                PlayerCommand::Seek(mut target_secs) => {
                    // Coalesce consecutive Seek commands
                    while let Ok(peeked) = command_receiver.try_recv() {
                        match peeked {
                            PlayerCommand::Seek(newer_secs) => {
                                target_secs = newer_secs;
                            }
                            other => {
                                next_cmd = Some(other);
                                break;
                            }
                        }
                    }
                    let target_us = (target_secs * 1_000_000.0).max(0.0) as i64;
                    if request_demux_seek(target_us) {
                        mono_clock.seek(target_us);
                        position_us.store(target_us, Ordering::Relaxed);
                        first_frame_after_seek = true;
                        last_audio_clock = target_us;
                        last_audio_adv_time = Instant::now();
                        seek_stabilize_until = Instant::now() + Duration::from_millis(300);
                        is_eof_shared.store(false, Ordering::Relaxed);
                        request_repaint();
                    }
                }
            }
            if next_cmd.is_none() {
                next_cmd = command_receiver.try_recv().ok();
            }
        }

        if stop_requested.load(Ordering::Relaxed) {
            break;
        }

        // Peek next frame PTS & generation
        let current_gen = seek_generation.load(Ordering::SeqCst);
        let peek_result = {
            let mut guard = video_queue.lock().ok();
            if let Some(ref mut q) = guard {
                // Drop any frames from prior seek generations
                while let Some((_, _, frame_gen)) = q.front() {
                    if *frame_gen < current_gen {
                        q.pop_front();
                    } else {
                        break;
                    }
                }
                q.front().map(|(pts, _, _)| *pts)
            } else {
                None
            }
        };

        // If this is the first frame after a seek, present immediately (even if paused)!
        if first_frame_after_seek {
            if peek_result.is_some() {
                first_frame_after_seek = false;
                let popped = video_queue.lock().ok().and_then(|mut q| q.pop_front());
                if let Some((pts, frame, _)) = popped {
                    position_us.store(pts, Ordering::Relaxed);
                    mono_clock.seek(pts);
                    let _ = frame_sender.try_send(frame);
                    request_repaint();
                }
            }
        }

        let is_paused = is_paused_shared.load(Ordering::Relaxed);
        if is_paused {
            thread::sleep(Duration::from_millis(10));
            continue;
        }

        // Determine master clock
        let audio_clock_val = audio_clock_pts_us.load(Ordering::Relaxed);
        if audio_clock_val != last_audio_clock {
            last_audio_clock = audio_clock_val;
            last_audio_adv_time = Instant::now();
        }

        let audio_stalled = has_audio && last_audio_adv_time.elapsed() > Duration::from_millis(200);
        let current_clock = if has_audio && !audio_stalled {
            audio_clock_val
        } else {
            mono_clock.now_pts_us()
        };

        // Check EOF condition
        let demux_is_eof = is_demux_eof.load(Ordering::Acquire);
        let queue_is_empty =
            peek_result.is_none() && video_queue.lock().map(|q| q.is_empty()).unwrap_or(true);
        if demux_is_eof && queue_is_empty {
            let sink_active = audio_sink.is_some() && !audio_stalled;
            let audio_remaining = has_audio
                && audio_queue
                    .lock()
                    .map(|mut q| q.has_unplayed_audio(current_gen, sink_active, current_clock))
                    .unwrap_or(false);
            if audio_remaining {
                position_us.store(current_clock, Ordering::Relaxed);
                request_repaint();
                thread::sleep(Duration::from_millis(2));
                continue;
            }

            if !is_eof_shared.load(Ordering::Relaxed) {
                if has_audio {
                    if let Ok(q) = audio_queue.lock() {
                        let end_pts = q.end_pts_us();
                        if end_pts > position_us.load(Ordering::Relaxed) {
                            position_us.store(end_pts, Ordering::Relaxed);
                        }
                    }
                }
                is_eof_shared.store(true, Ordering::Relaxed);
                is_paused_shared.store(true, Ordering::Relaxed);
                if let Some(ref sink) = audio_sink {
                    sink.pause();
                }
                mono_clock.pause();
                let _ = status_sender.send(PlayerEvent::Eof);
                request_repaint();
            }
            thread::sleep(Duration::from_millis(15));
            continue;
        }

        let Some(frame_pts) = peek_result else {
            thread::sleep(Duration::from_millis(2));
            continue;
        };

        let diff_us = frame_pts - current_clock;
        if diff_us > 1_500 {
            let sleep_us = (diff_us - 1_000).min(5_000);
            thread::sleep(Duration::from_micros(sleep_us as u64));
            continue;
        }

        // Present frame
        let popped = video_queue.lock().ok().and_then(|mut q| q.pop_front());
        if let Some((pts, frame, _)) = popped {
            // Drop severely late frames (>100ms behind clock) if queue has newer frames,
            // but suppress drops during the first 300ms after a seek while pipeline stabilizes.
            if Instant::now() > seek_stabilize_until && current_clock - pts > 100_000 {
                let has_more = video_queue.lock().map(|q| !q.is_empty()).unwrap_or(false);
                if has_more {
                    continue;
                }
            }

            position_us.store(pts, Ordering::Relaxed);
            let _ = frame_sender.try_send(frame);
            request_repaint();
        }
    }

    // Cleanup demuxer thread
    demux_stop_flag.store(true, Ordering::Relaxed);
    let _ = demux_cmd_tx.send(DemuxCommand::Stop);
    let _ = demux_handle.join();

    let _ = status_sender.send(PlayerEvent::Stopped);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_resampler_44k() -> ffmpeg_next::software::resampling::context::Context {
        ffmpeg_next::init().unwrap();
        ffmpeg_next::software::resampling::context::Context::get(
            ffmpeg_next::format::Sample::F32(ffmpeg_next::format::sample::Type::Planar),
            ffmpeg_next::ChannelLayout::STEREO,
            44100,
            ffmpeg_next::format::Sample::F32(ffmpeg_next::format::sample::Type::Packed),
            ffmpeg_next::ChannelLayout::STEREO,
            AUDIO_OUTPUT_RATE,
        )
        .unwrap()
    }

    fn make_test_aac_like_frame(index: usize) -> ffmpeg_next::frame::Audio {
        let mut f = ffmpeg_next::frame::Audio::new(
            ffmpeg_next::format::Sample::F32(ffmpeg_next::format::sample::Type::Planar),
            1024,
            ffmpeg_next::ChannelLayout::STEREO,
        );
        f.set_rate(44100);
        for ch in 0..2 {
            for (i, s) in f.plane_mut::<f32>(ch).iter_mut().enumerate() {
                *s = (((index * 1024 + i) as f32) * 0.01).sin() * 0.5;
            }
        }
        f
    }

    #[test]
    fn test_resampler_44k_to_48k_does_not_accumulate_backlog() {
        let mut rsm = make_test_resampler_44k();
        let mut total = 0usize;
        // 10 seconds of 44.1 kHz AAC-sized (1024-sample) frames
        let frames = 431usize;
        for i in 0..frames {
            let out = resample_audio_frame(&mut rsm, &make_test_aac_like_frame(i)).unwrap();
            total += out.samples();
        }
        let expected = frames * 1024 * 48000 / 44100;
        // Output must track the input duration to within a few ms (filter delay only)
        assert!(
            expected - total < 480,
            "resampler fell behind: got {} frames, expected ~{}",
            total,
            expected
        );
        let backlog = rsm.delay().map(|d| d.output).unwrap_or(0);
        assert!(
            backlog < 480,
            "swr is holding {} frames of stale audio",
            backlog
        );
    }

    #[test]
    fn test_fresh_resampler_after_seek_has_no_stale_audio() {
        let mut rsm = make_test_resampler_44k();
        for i in 0..200 {
            let _ = resample_audio_frame(&mut rsm, &make_test_aac_like_frame(i));
        }
        rsm = create_audio_resampler(
            ffmpeg_next::format::Sample::F32(ffmpeg_next::format::sample::Type::Planar),
            ffmpeg_next::ChannelLayout::STEREO,
            44100,
        )
        .unwrap();
        assert_eq!(rsm.delay().map(|d| d.output).unwrap_or(0), 0);
    }
    #[test]
    fn test_format_time_values() {
        assert_eq!(format_time(0.0), "00:00");
        assert_eq!(format_time(-10.0), "00:00");
        assert_eq!(format_time(f64::NAN), "00:00");
        assert_eq!(format_time(5.0), "00:05");
        assert_eq!(format_time(59.0), "00:59");
        assert_eq!(format_time(60.0), "01:00");
        assert_eq!(format_time(65.0), "01:05");
        assert_eq!(format_time(3599.0), "59:59");
        assert_eq!(format_time(3600.0), "01:00:00");
        assert_eq!(format_time(3665.0), "01:01:05");
        assert_eq!(format_time(7325.0), "02:02:05");
        assert_eq!(format_time(37304.0), "10:21:44");
    }

    #[test]
    fn test_format_time_with_duration() {
        assert_eq!(format_time_with_duration(0.0, 37304.0), "00:00:00");
        assert_eq!(format_time_with_duration(15.0, 37304.0), "00:00:15");
        assert_eq!(format_time_with_duration(3665.0, 37304.0), "01:01:05");
        assert_eq!(format_time_with_duration(15.0, 600.0), "00:15");
        assert_eq!(format_time_with_duration(599.0, 600.0), "09:59");
    }

    #[test]
    fn test_monotonic_clock_pause_resume_seek() {
        let mut clock = MonotonicClock::new(1_000_000);
        thread::sleep(Duration::from_millis(10));
        let pts1 = clock.now_pts_us();
        assert!(pts1 >= 1_000_000);

        clock.pause();
        let pts_paused = clock.now_pts_us();
        thread::sleep(Duration::from_millis(10));
        assert_eq!(clock.now_pts_us(), pts_paused);

        clock.resume();
        thread::sleep(Duration::from_millis(10));
        assert!(clock.now_pts_us() >= pts_paused);

        clock.seek(5_000_000);
        let pts_seeked = clock.now_pts_us();
        assert!((pts_seeked - 5_000_000).abs() < 50_000);
    }

    #[test]
    fn test_shared_audio_queue_silence_does_not_corrupt_consumed_count() {
        let queue = Arc::new(Mutex::new(SharedAudioQueue::new()));
        let pts_shared = Arc::new(AtomicI64::new(10_000_000));
        let seek_generation = Arc::new(AtomicU64::new(0));
        let peak_amplitude = Arc::new(AtomicU64::new(0));
        let audio_latency_ms = Arc::new(AtomicU64::new(0));
        let replay_audio = Arc::new(crate::replay::AudioTap::default());

        let mut source = VideoFileAudioSource {
            queue: Arc::clone(&queue),
            current_pts_shared: Arc::clone(&pts_shared),
            seek_generation,
            peak_amplitude,
            audio_latency_ms,
            replay_audio,
            local_batch: Vec::with_capacity(512),
            read_buf: VecDeque::with_capacity(512),
            local_gen: 0,
        };

        // Initialize queue to 10.0s at generation 0 (as on a seek)
        queue.lock().unwrap().clear(10_000_000, 0);

        // Pull 100 samples while queue is empty (underrun silence)
        for _ in 0..100 {
            let sample = source.next().unwrap();
            assert_eq!(sample, 0.0);
        }

        // Verify that silence did NOT corrupt samples_consumed or drift pts_shared
        {
            let q = queue.lock().unwrap();
            assert_eq!(q.samples_consumed, 0);
            assert_eq!(q.head_pts_us, 10_000_000);
        }
        assert_eq!(pts_shared.load(Ordering::Relaxed), 10_000_000);

        // Now push real samples (e.g. 480 stereo samples = 960 floats = 10ms of audio)
        {
            let mut q = queue.lock().unwrap();
            q.samples.extend((0..960).map(|i| (i as f32) / 1000.0));
        }

        // Pull 480 samples (240 stereo frames = 5ms)
        for _ in 0..480 {
            let _ = source.next().unwrap();
        }

        // PTS should have advanced by exactly 5000us (5ms) from 10_000_000 to 10_005_000
        let current_pts = pts_shared.load(Ordering::Relaxed);
        assert_eq!(current_pts, 10_005_000);
    }

    #[test]
    fn test_video_file_audio_source_discards_stale_generation_samples() {
        let queue = Arc::new(Mutex::new(SharedAudioQueue::new()));
        let pts_shared = Arc::new(AtomicI64::new(5_000_000));
        let seek_generation = Arc::new(AtomicU64::new(0));
        let peak_amplitude = Arc::new(AtomicU64::new(0));
        let audio_latency_ms = Arc::new(AtomicU64::new(0));
        let replay_audio = Arc::new(crate::replay::AudioTap::default());

        let mut source = VideoFileAudioSource {
            queue: Arc::clone(&queue),
            current_pts_shared: Arc::clone(&pts_shared),
            seek_generation: Arc::clone(&seek_generation),
            peak_amplitude,
            audio_latency_ms,
            replay_audio,
            local_batch: Vec::with_capacity(512),
            read_buf: VecDeque::with_capacity(512),
            local_gen: 0,
        };

        // Queue contains samples from generation 0
        {
            let mut q = queue.lock().unwrap();
            q.clear(5_000_000, 0);
            q.samples.extend((0..480).map(|_| 0.5f32));
        }

        // Bump seek_generation to simulate a seek/stop
        seek_generation.store(1, Ordering::SeqCst);

        // First sample read should detect generation mismatch, discard stale samples, and return silence
        let sample = source.next().unwrap();
        assert_eq!(sample, 0.0);

        // Verify remaining stale samples were cleared
        assert!(queue.lock().unwrap().samples.is_empty());
    }

    #[test]
    fn test_audio_trim_at_seek_boundary() {
        let target_us = 10_000_000i64; // 10.0s
        let pts_us = 9_990_000i64; // 9.99s (10ms before target)
        let samples_count = 960usize; // 20ms of audio (48kHz stereo = 1920 floats)
        let slice: Vec<f32> = (0..(samples_count * 2)).map(|i| i as f32).collect();

        let duration_us = (samples_count as f64 / AUDIO_OUTPUT_RATE as f64 * 1_000_000.0) as i64;
        let frame_end_us = pts_us + duration_us; // 10.01s (spans across target)

        assert!(pts_us < target_us && frame_end_us > target_us);

        let skip_us = target_us - pts_us;
        let skip_frames =
            (skip_us as f64 / 1_000_000.0 * AUDIO_OUTPUT_RATE as f64).round() as usize;
        let skip_samples = (skip_frames * 2).min(slice.len());
        let trimmed_slice = &slice[skip_samples..];

        // 10ms at 48kHz = 480 frames = 960 samples skipped
        assert_eq!(trimmed_slice.len(), 960);
    }

    #[test]
    fn test_flush_audio_resampler_drains_buffered_tail_samples() {
        let mut rsm = make_test_resampler_44k();
        let mut produced_before_flush = 0usize;
        for i in 0..10 {
            let out = resample_audio_frame(&mut rsm, &make_test_aac_like_frame(i)).unwrap();
            produced_before_flush += out.samples();
        }
        let pending_before = rsm.delay().map(|d| d.output).unwrap_or(0);
        assert!(pending_before > 0, "expected swr filter delay before flush");

        let mut flushed_total = 0usize;
        while let Some(flushed) = flush_audio_resampler(&mut rsm) {
            flushed_total += flushed.samples();
        }
        assert!(
            flushed_total > 0,
            "flush_audio_resampler should recover delayed samples from swr"
        );
        let expected_total = (10 * 1024 * AUDIO_OUTPUT_RATE as usize) / 44100;
        assert!(
            (produced_before_flush + flushed_total).abs_diff(expected_total) <= 2,
            "total samples after flush ({}) should match expected ({})",
            produced_before_flush + flushed_total,
            expected_total
        );
    }

    #[test]
    fn test_video_eof_drain_recovers_b_frame_reordered_tail() {
        ffmpeg_next::init().unwrap();
        let codec = ffmpeg_next::encoder::find(ffmpeg_next::codec::Id::MPEG4)
            .or_else(|| ffmpeg_next::encoder::find(ffmpeg_next::codec::Id::H264))
            .expect("MPEG4 or H264 encoder must be available");
        let time_base = ffmpeg_next::Rational(1, 30);

        let mut enc_ctx = ffmpeg_next::codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .unwrap();
        enc_ctx.set_width(64);
        enc_ctx.set_height(64);
        enc_ctx.set_format(ffmpeg_next::format::Pixel::YUV420P);
        enc_ctx.set_time_base(time_base);
        enc_ctx.set_max_b_frames(2);
        enc_ctx.set_bit_rate(200_000);
        let mut encoder = enc_ctx.open_as(codec).unwrap();

        let total_input_frames = 6i64;
        let mut packets = Vec::new();
        for i in 0..total_input_frames {
            let mut frame =
                ffmpeg_next::frame::Video::new(ffmpeg_next::format::Pixel::YUV420P, 64, 64);
            frame.set_pts(Some(i));
            frame.data_mut(0).fill((i as u8) * 30 + 16);
            frame.data_mut(1).fill(128);
            frame.data_mut(2).fill(128);
            encoder.send_frame(&frame).unwrap();
            let mut pkt = ffmpeg_next::Packet::empty();
            while encoder.receive_packet(&mut pkt).is_ok() {
                packets.push(pkt.clone());
            }
        }
        encoder.send_eof().unwrap();
        let mut pkt = ffmpeg_next::Packet::empty();
        while encoder.receive_packet(&mut pkt).is_ok() {
            packets.push(pkt.clone());
        }

        let params = ffmpeg_next::codec::Parameters::from(&encoder);
        let mut decoder = ffmpeg_next::codec::context::Context::from_parameters(params)
            .unwrap()
            .decoder()
            .video()
            .unwrap();
        let output_format = renderable_pixel_format(decoder.format());
        let mut scaler = ffmpeg_next::software::scaling::context::Context::get(
            decoder.format(),
            64,
            64,
            output_format,
            64,
            64,
            ScalerFilter::Bilinear.into_ffmpeg_flag(),
        )
        .unwrap();

        let mut decoded = ffmpeg_next::frame::Video::empty();
        let mut normalized = ffmpeg_next::frame::Video::empty();
        let mut video_fallback_pts = 0i64;
        let mut pending_video_seek = None;
        let mut pending_audio_seek = None;
        let mut last_video_frame = None;
        let color_range = AtomicU8::new(0);
        let replay_rate = crate::replay::config::Rate::new(30, 1);
        let video_queue = Mutex::new(VecDeque::new());

        for packet in &packets {
            decoder.send_packet(packet).unwrap();
            receive_and_enqueue_video_frames(
                &mut decoder,
                &mut scaler,
                &mut decoded,
                &mut normalized,
                output_format,
                time_base,
                33_333,
                &mut video_fallback_pts,
                &mut pending_video_seek,
                &mut pending_audio_seek,
                &mut last_video_frame,
                &color_range,
                replay_rate,
                &video_queue,
                0,
                false,
            );
        }

        let count_before_eof = video_queue.lock().unwrap().len();
        assert!(
            count_before_eof < total_input_frames as usize,
            "B-frame decoder should hold at least one delayed frame before send_eof (got {})",
            count_before_eof
        );

        receive_and_enqueue_video_frames(
            &mut decoder,
            &mut scaler,
            &mut decoded,
            &mut normalized,
            output_format,
            time_base,
            33_333,
            &mut video_fallback_pts,
            &mut pending_video_seek,
            &mut pending_audio_seek,
            &mut last_video_frame,
            &color_range,
            replay_rate,
            &video_queue,
            0,
            true,
        );

        assert_eq!(
            video_queue.lock().unwrap().len(),
            total_input_frames as usize,
            "EOF drain must recover all delayed B-frames"
        );
    }

    #[test]
    fn test_longer_audio_tail_delays_eof_until_drained() {
        let mut q = SharedAudioQueue::new();
        q.clear(0, 1);
        // Push 1.0s of stereo 48kHz audio (48_000 frames * 2 channels = 96_000 samples)
        q.samples.resize(96_000, 0.25);
        assert_eq!(q.end_pts_us(), 1_000_000);

        // Video ended at 800ms; with active audio sink, audio is still unplayed while samples remain
        assert!(q.has_unplayed_audio(1, true, 800_000));

        // Even without an active sink (headless fallback), audio remains unplayed while clock < 1.0s
        assert!(q.has_unplayed_audio(1, false, 850_000));
        assert!(!q.samples.is_empty());

        // Once fallback clock reaches 1.0s (or sink drains all samples), EOF can proceed
        assert!(!q.has_unplayed_audio(1, false, 1_000_000));
        assert!(q.samples.is_empty());
    }

    #[test]
    fn test_apply_seek_if_supported_checks_fallback_and_reports_failure() {
        let tb = ffmpeg_next::Rational(1, 1000);
        // Both seeks fail -> returns false so caller preserves queues/clocks
        assert!(!apply_seek_if_supported(
            5_000_000,
            2,
            tb,
            |stream_idx, ts| {
                if stream_idx == -1 {
                    assert_eq!(ts, 5_000_000);
                    -1
                } else {
                    assert_eq!(stream_idx, 2);
                    assert_eq!(ts, 5_000);
                    -22
                }
            }
        ));

        // Default seek fails, stream seek succeeds -> returns true
        assert!(apply_seek_if_supported(
            5_000_000,
            2,
            tb,
            |stream_idx, _| {
                if stream_idx == -1 {
                    -1
                } else {
                    0
                }
            }
        ));

        // Default seek succeeds -> stream seek not even needed
        let mut fallback_called = false;
        assert!(apply_seek_if_supported(
            5_000_000,
            2,
            tb,
            |stream_idx, _| {
                if stream_idx == -1 {
                    0
                } else {
                    fallback_called = true;
                    -1
                }
            }
        ));
        assert!(!fallback_called);
    }

    #[test]
    fn test_classify_packet_read_error_distinguishes_eagain_and_bounds_permanent_errors() {
        let mut consecutive = 0;
        let eagain = ffmpeg_next::Error::Other {
            errno: libc::EAGAIN,
        };
        for _ in 0..32 {
            assert_eq!(
                classify_packet_read_error(&eagain, &mut consecutive),
                PacketReadErrorAction::RetryTransient
            );
        }
        assert_eq!(consecutive, 0, "EAGAIN must not count as a permanent error");

        assert_eq!(
            classify_packet_read_error(&ffmpeg_next::Error::Eof, &mut consecutive),
            PacketReadErrorAction::DrainAndEof
        );

        let perm_err = ffmpeg_next::Error::InvalidData;
        for i in 1..MAX_CONSECUTIVE_PACKET_READ_ERRORS {
            assert_eq!(
                classify_packet_read_error(&perm_err, &mut consecutive),
                PacketReadErrorAction::RetryTransient,
                "iteration {i} should retry briefly before threshold"
            );
        }
        assert_eq!(
            classify_packet_read_error(&perm_err, &mut consecutive),
            PacketReadErrorAction::AbortDemux
        );
    }
}
