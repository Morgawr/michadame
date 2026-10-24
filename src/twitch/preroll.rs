//! Background pre-roll video player for Twitch streaming.
//!
//! Plays a user-selected video file before going live on Twitch:
//! - Decodes video frames to `RawFrame` timed to presentation clock
//! - Decodes audio to 48kHz stereo and submits to the broadcast audio route
//! - Mutes streamer's mic and game capture to the livestream during pre-roll
//! - Provides fade out / fade in transition parameters to seamlessly transition to the live feed

use crate::replay::AudioTap;
use crate::video::decoder::{copy_frame_tightly, renderable_pixel_format};
use crate::video::player::{create_audio_resampler, probe_video_file, resample_audio_frame};
use crate::video::types::{ColorRange, RawFrame};
use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender};
use rodio::{buffer::SamplesBuffer, Sink};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_TRANSITION_SECS: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollMode {
    Preroll,
    Outro,
}

pub struct PrerollPlayer {
    pub mode: RollMode,
    pub duration_secs: f64,
    pub position_us: Arc<AtomicI64>,
    pub is_finished: Arc<AtomicBool>,
    pub stop_requested: Arc<AtomicBool>,
    pub frame_rx: Receiver<Arc<RawFrame>>,
    pub latest_frame: Option<Arc<RawFrame>>,
    pub thread_handle: Option<thread::JoinHandle<()>>,
    pub audio_tap: Arc<AudioTap>,
    pub transition_duration_secs: f64,
}

impl PrerollPlayer {
    pub fn start(
        file_path: PathBuf,
        audio_tap: Arc<AudioTap>,
        repaint: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        Self::start_with_mode(file_path, audio_tap, repaint, RollMode::Preroll)
    }

    pub fn start_outro(
        file_path: PathBuf,
        audio_tap: Arc<AudioTap>,
        repaint: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        Self::start_with_mode(file_path, audio_tap, repaint, RollMode::Outro)
    }

    pub fn start_with_mode(
        file_path: PathBuf,
        audio_tap: Arc<AudioTap>,
        repaint: Arc<dyn Fn() + Send + Sync>,
        mode: RollMode,
    ) -> Result<Self> {
        let label = match mode {
            RollMode::Preroll => "pre-roll",
            RollMode::Outro => "outro",
        };
        let probe = probe_video_file(&file_path)
            .with_context(|| format!("Failed to probe {label} video file {:?}", file_path))?;
        let duration_secs = probe.duration_secs.max(0.5);
        let transition_duration_secs = if duration_secs < 2.0 {
            (duration_secs * 0.4).max(0.2)
        } else {
            DEFAULT_TRANSITION_SECS
        };

        let position_us = Arc::new(AtomicI64::new(0));
        let is_finished = Arc::new(AtomicBool::new(false));
        let stop_requested = Arc::new(AtomicBool::new(false));
        let (frame_tx, frame_rx) = crossbeam_channel::bounded::<Arc<RawFrame>>(4);

        audio_tap.set_broadcast_preroll_active(true);

        let thread_path = file_path.clone();
        let thread_audio_tap = audio_tap.clone();
        let thread_stop = stop_requested.clone();
        let thread_pos = position_us.clone();
        let thread_finished = is_finished.clone();
        let thread_dur_secs = duration_secs;
        let thread_trans_secs = transition_duration_secs;
        let thread_repaint = repaint.clone();
        let thread_name = match mode {
            RollMode::Preroll => "preroll-player",
            RollMode::Outro => "outro-player",
        };

        let handle = thread::Builder::new()
            .name(thread_name.into())
            .spawn(move || {
                let res = run_preroll_worker(
                    thread_path,
                    thread_audio_tap.clone(),
                    frame_tx,
                    thread_pos.clone(),
                    thread_finished.clone(),
                    thread_stop.clone(),
                    thread_dur_secs,
                    thread_trans_secs,
                    mode,
                    thread_repaint.clone(),
                );
                if let Err(e) = res {
                    tracing::warn!("Roll video player ({mode:?}) stopped with error: {e:#}");
                }
                if mode == RollMode::Preroll || thread_stop.load(Ordering::Relaxed) {
                    thread_audio_tap.set_broadcast_preroll_active(false);
                }
                thread_finished.store(true, Ordering::Release);
                thread_repaint();
            })?;

        Ok(Self {
            mode,
            duration_secs,
            position_us,
            is_finished,
            stop_requested,
            frame_rx,
            latest_frame: None,
            thread_handle: Some(handle),
            audio_tap,
            transition_duration_secs,
        })
    }

    pub fn elapsed_secs(&self) -> f64 {
        self.position_us.load(Ordering::Relaxed) as f64 / 1_000_000.0
    }

    pub fn remaining_secs(&self) -> f64 {
        (self.duration_secs - self.elapsed_secs()).max(0.0)
    }

    /// Computes the black fade overlay alpha (0.0 = completely transparent, 1.0 = completely black).
    ///
    /// For `RollMode::Preroll` (at the end of the pre-roll video):
    /// - First half: pre-roll fades to black (0.0 -> 1.0)
    /// - Midpoint: completely black (1.0), live feed takes over
    /// - Second half: live feed fades in from black (1.0 -> 0.0)
    ///
    /// For `RollMode::Outro` (at the beginning of the outro video):
    /// - First half: live feed fades to black (0.0 -> 1.0)
    /// - Midpoint: completely black (1.0), outro video takes over
    /// - Second half: outro video fades in from black (1.0 -> 0.0)
    pub fn fade_alpha(&self) -> f32 {
        let d = self.duration_secs;
        let t = self.elapsed_secs();
        let trans = self.transition_duration_secs;
        let half = (trans / 2.0).max(0.001);

        match self.mode {
            RollMode::Preroll => {
                let t_start = (d - trans).max(0.0);
                let t_mid = t_start + trans / 2.0;

                if t < t_start {
                    0.0
                } else if t < t_mid {
                    ((t - t_start) / half).clamp(0.0, 1.0) as f32
                } else if t < d {
                    ((d - t) / half).clamp(0.0, 1.0) as f32
                } else {
                    0.0
                }
            }
            RollMode::Outro => {
                let t_mid = trans / 2.0;
                let t_end = trans.min(d);

                if t <= 0.0 {
                    0.0
                } else if t < t_mid {
                    (t / half).clamp(0.0, 1.0) as f32
                } else if t < t_end {
                    ((t_end - t) / half).clamp(0.0, 1.0) as f32
                } else {
                    0.0
                }
            }
        }
    }

    /// True if the pre-roll transition has reached or passed the midpoint where the live capture
    /// feed should replace the pre-roll video under the black overlay.
    pub fn is_in_fade_in_phase(&self) -> bool {
        let d = self.duration_secs;
        let t = self.elapsed_secs();
        let trans = self.transition_duration_secs;
        let t_mid = (d - trans).max(0.0) + trans / 2.0;
        if t >= t_mid {
            if self.mode == RollMode::Preroll {
                self.audio_tap.set_broadcast_preroll_active(false);
            }
            true
        } else {
            false
        }
    }

    /// For `RollMode::Outro`, returns true during the first half of the entry transition
    /// while the live capture feed is fading out to black before the outro video takes over.
    pub fn is_in_fade_out_phase(&self) -> bool {
        let t = self.elapsed_secs();
        let t_mid = self.transition_duration_secs / 2.0;
        t < t_mid
    }

    /// Polls new decoded frames from the worker thread.
    pub fn poll_frame(&mut self) -> Option<Arc<RawFrame>> {
        while let Ok(frame) = self.frame_rx.try_recv() {
            self.latest_frame = Some(frame);
        }
        self.latest_frame.clone()
    }

    pub fn is_completed(&self) -> bool {
        self.is_finished.load(Ordering::Relaxed) || self.elapsed_secs() >= self.duration_secs
    }

    pub fn stop(&mut self) {
        self.stop_requested.store(true, Ordering::Relaxed);
        self.audio_tap.set_broadcast_preroll_active(false);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for PrerollPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[allow(clippy::too_many_arguments)]
fn run_preroll_worker(
    path: PathBuf,
    audio_tap: Arc<AudioTap>,
    frame_tx: Sender<Arc<RawFrame>>,
    position_us: Arc<AtomicI64>,
    is_finished: Arc<AtomicBool>,
    stop_requested: Arc<AtomicBool>,
    duration_secs: f64,
    transition_secs: f64,
    mode: RollMode,
    repaint: Arc<dyn Fn() + Send + Sync>,
) -> Result<()> {
    ffmpeg_next::init().context("Failed to init FFmpeg")?;
    let mut ictx = ffmpeg_next::format::input(&path)
        .with_context(|| format!("Failed to open roll video file {:?}", path))?;

    let video_info = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .map(|s| (s.index(), s.time_base(), s.parameters()))
        .context("No video stream found in roll file")?;
    let (video_idx, video_time_base, video_params) = video_info;

    let mut video_decoder = ffmpeg_next::codec::context::Context::from_parameters(video_params)?
        .decoder()
        .video()
        .context("Failed to create video decoder")?;
    video_decoder.set_threading(ffmpeg_next::codec::threading::Config::default());

    let output_format = renderable_pixel_format(video_decoder.format());
    let mut scaler = ffmpeg_next::software::scaling::context::Context::get(
        video_decoder.format(),
        video_decoder.width(),
        video_decoder.height(),
        output_format,
        video_decoder.width(),
        video_decoder.height(),
        ffmpeg_next::software::scaling::flag::Flags::BILINEAR,
    )?;

    // Check for optional audio stream
    let audio_info = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Audio)
        .map(|s| (s.index(), s.time_base(), s.parameters()));

    let (mut audio_decoder, audio_idx, audio_time_base, mut resampler) =
        if let Some((idx, tb, params)) = audio_info {
            match ffmpeg_next::codec::context::Context::from_parameters(params)
                .and_then(|c| c.decoder().audio())
            {
                Ok(dec) => {
                    let layout = if dec.channel_layout().is_empty() {
                        ffmpeg_next::ChannelLayout::default(dec.channels().max(1) as i32)
                    } else {
                        dec.channel_layout()
                    };
                    let rsm = create_audio_resampler(dec.format(), layout, dec.rate());
                    (Some(dec), Some(idx), Some(tb), rsm)
                }
                Err(e) => {
                    tracing::warn!("Could not open roll audio stream: {e}");
                    (None, None, None, None)
                }
            }
        } else {
            (None, None, None, None)
        };

    // Initialize local rodio output so streamer can hear the pre-roll / outro audio
    let (_rodio_guard, local_sink) = match crate::devices::audio::initialize_rodio_playback() {
        Ok((guard, handle)) => (Some(guard), Sink::try_new(&handle).ok()),
        Err(_) => (None, None),
    };

    let start_time = Instant::now();
    let start_epoch_us = crate::replay::now_us();
    let total_us = (duration_secs * 1_000_000.0) as i64;
    let t_start = (duration_secs - transition_secs).max(0.0);
    let t_mid = t_start + transition_secs / 2.0;
    let t_mid_us = (t_mid * 1_000_000.0) as i64;
    let audio_cutoff_us = match mode {
        RollMode::Preroll => t_mid_us,
        RollMode::Outro => total_us,
    };

    let mut preroll_audio_active = true;
    let mut audio_frames_emitted: u64 = 0;
    let mut silent_audio_emitted_us: i64 = 0;
    const SILENT_CHUNK_US: i64 = 20_000;
    const SILENT_CHUNK_SAMPLES: usize = 960 * 2; // 20ms at 48kHz stereo
    let silent_chunk = [0.0f32; SILENT_CHUNK_SAMPLES];

    let mut decoded_video = ffmpeg_next::frame::Video::empty();
    let mut normalized_video = ffmpeg_next::frame::Video::empty();
    let mut decoded_audio = ffmpeg_next::frame::Audio::empty();

    for (stream, packet) in ictx.packets() {
        if stop_requested.load(Ordering::Relaxed) {
            break;
        }

        let stream_idx = stream.index();
        if stream_idx == video_idx {
            if video_decoder.send_packet(&packet).is_ok() {
                while video_decoder.receive_frame(&mut decoded_video).is_ok() {
                    if stop_requested.load(Ordering::Relaxed) {
                        break;
                    }

                    let frame_pts_us = decoded_video
                        .pts()
                        .map(|pts| unsafe {
                            ffmpeg_next::sys::av_rescale_q(
                                pts,
                                video_time_base.into(),
                                ffmpeg_next::Rational(1, 1_000_000).into(),
                            )
                        })
                        .unwrap_or_else(|| start_time.elapsed().as_micros() as i64);

                    // Sync presentation to wall clock
                    let target_time =
                        start_time + Duration::from_micros(frame_pts_us.max(0) as u64);
                    while !stop_requested.load(Ordering::Relaxed) {
                        let now = Instant::now();
                        if target_time <= now {
                            break;
                        }
                        let sleep_dur = target_time - now;
                        if sleep_dur <= Duration::from_millis(1) {
                            break;
                        }
                        thread::sleep(sleep_dur.min(Duration::from_millis(50)));
                    }

                    if stop_requested.load(Ordering::Relaxed) {
                        break;
                    }

                    if audio_idx.is_none() && preroll_audio_active {
                        let target_silent_us = frame_pts_us.max(0).min(audio_cutoff_us);
                        while silent_audio_emitted_us + SILENT_CHUNK_US <= target_silent_us {
                            audio_tap.submit_preroll_audio(
                                &silent_chunk,
                                48000,
                                2,
                                start_epoch_us + silent_audio_emitted_us,
                            );
                            silent_audio_emitted_us += SILENT_CHUNK_US;
                        }
                    }

                    if mode == RollMode::Preroll && preroll_audio_active && frame_pts_us >= t_mid_us {
                        preroll_audio_active = false;
                        audio_tap.set_broadcast_preroll_active(false);
                    }

                    if scaler.run(&decoded_video, &mut normalized_video).is_ok() {
                        if let Ok(data) = copy_frame_tightly(&normalized_video) {
                            let raw_frame = Arc::new(RawFrame {
                                captured_at: crate::replay::now_us(),
                                rate: crate::replay::config::Rate::new(60, 1),
                                width: normalized_video.width(),
                                height: normalized_video.height(),
                                data,
                                format: normalized_video.format(),
                                color_range: ColorRange::Limited,
                            });
                            let _ = frame_tx.try_send(raw_frame);
                            repaint();
                        }
                    }

                    position_us.store(frame_pts_us.min(total_us), Ordering::Relaxed);
                }
            }
        } else if Some(stream_idx) == audio_idx {
            if let (Some(ref mut dec), Some(ref mut rsm)) = (&mut audio_decoder, &mut resampler) {
                if dec.send_packet(&packet).is_ok() {
                    while dec.receive_frame(&mut decoded_audio).is_ok() {
                        if stop_requested.load(Ordering::Relaxed) {
                            break;
                        }
                        if let Some(resampled) = resample_audio_frame(rsm, &decoded_audio) {
                            let frames = resampled.samples();
                            let samples_count = frames * 2; // stereo
                            if samples_count > 0 {
                                let fallback_pts_us =
                                    ((audio_frames_emitted * 1_000_000) / 48_000) as i64;
                                let audio_pts_us = decoded_audio
                                    .pts()
                                    .and_then(|pts| {
                                        audio_time_base.map(|tb| unsafe {
                                            ffmpeg_next::sys::av_rescale_q(
                                                pts,
                                                tb.into(),
                                                ffmpeg_next::Rational(1, 1_000_000).into(),
                                            )
                                        })
                                    })
                                    .unwrap_or(fallback_pts_us)
                                    .max(0);
                                audio_frames_emitted += frames as u64;

                                let pts_sec = audio_pts_us as f64 / 1_000_000.0;
                                while !stop_requested.load(Ordering::Relaxed) {
                                    let elapsed_sec = start_time.elapsed().as_secs_f64();
                                    if pts_sec <= elapsed_sec + 0.05 {
                                        break;
                                    }
                                    let sleep_secs = (pts_sec - (elapsed_sec + 0.05)).min(0.05);
                                    thread::sleep(Duration::from_secs_f64(sleep_secs));
                                }

                                if stop_requested.load(Ordering::Relaxed) {
                                    break;
                                }

                                if mode == RollMode::Preroll && pts_sec >= t_mid {
                                    if preroll_audio_active {
                                        preroll_audio_active = false;
                                        audio_tap.set_broadcast_preroll_active(false);
                                    }
                                    continue;
                                }

                                let plane = resampled.plane::<f32>(0);
                                let mut samples = plane[..samples_count.min(plane.len())].to_vec();

                                let vol = match mode {
                                    RollMode::Preroll => {
                                        // Fade pre-roll audio volume out during transition first half
                                        if pts_sec > t_start {
                                            ((t_mid - pts_sec) / (t_mid - t_start).max(0.001))
                                                .clamp(0.0, 1.0) as f32
                                        } else {
                                            1.0f32
                                        }
                                    }
                                    RollMode::Outro => {
                                        // Fade outro audio volume in during entry transition
                                        let t_end = transition_secs.min(duration_secs);
                                        if pts_sec < t_end {
                                            (pts_sec / t_end.max(0.001)).clamp(0.0, 1.0) as f32
                                        } else {
                                            1.0f32
                                        }
                                    }
                                };

                                if vol < 0.999 {
                                    for s in &mut samples {
                                        *s *= vol;
                                    }
                                }

                                if preroll_audio_active {
                                    audio_tap.submit_preroll_audio(
                                        &samples,
                                        48000,
                                        2,
                                        start_epoch_us + audio_pts_us,
                                    );
                                }

                                // Play locally through streamer's audio device
                                if let Some(ref sink) = local_sink {
                                    if vol > 0.001 {
                                        sink.append(SamplesBuffer::new(2, 48000, samples));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // After demux EOF, wait until full duration / transition has elapsed
    while !stop_requested.load(Ordering::Relaxed) {
        let elapsed = start_time.elapsed().as_micros() as i64;
        if audio_idx.is_none() && preroll_audio_active {
            let target_silent_us = elapsed.max(0).min(audio_cutoff_us);
            while silent_audio_emitted_us + SILENT_CHUNK_US <= target_silent_us {
                audio_tap.submit_preroll_audio(
                    &silent_chunk,
                    48000,
                    2,
                    start_epoch_us + silent_audio_emitted_us,
                );
                silent_audio_emitted_us += SILENT_CHUNK_US;
            }
        }
        if mode == RollMode::Preroll && preroll_audio_active && elapsed >= t_mid_us {
            preroll_audio_active = false;
            audio_tap.set_broadcast_preroll_active(false);
        }
        position_us.store(elapsed.min(total_us), Ordering::Relaxed);
        repaint();
        if elapsed >= total_us {
            break;
        }
        thread::sleep(Duration::from_millis(16));
    }

    position_us.store(total_us, Ordering::Relaxed);
    is_finished.store(true, Ordering::Release);
    repaint();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fade_alpha_calculations() {
        let player = PrerollPlayer {
            mode: RollMode::Preroll,
            duration_secs: 10.0,
            position_us: Arc::new(AtomicI64::new(0)),
            is_finished: Arc::new(AtomicBool::new(false)),
            stop_requested: Arc::new(AtomicBool::new(false)),
            frame_rx: crossbeam_channel::unbounded().1,
            latest_frame: None,
            thread_handle: None,
            audio_tap: Arc::new(AudioTap::default()),
            transition_duration_secs: 1.0,
        };

        // At t = 0 (start), alpha is 0.0
        assert_eq!(player.fade_alpha(), 0.0);
        assert!(!player.is_in_fade_in_phase());

        // At t = 8.0 (before transition start at 9.0), alpha is 0.0
        player.position_us.store(8_000_000, Ordering::Relaxed);
        assert_eq!(player.fade_alpha(), 0.0);
        assert!(!player.is_in_fade_in_phase());

        // At t = 9.0 (transition start), alpha is 0.0
        player.position_us.store(9_000_000, Ordering::Relaxed);
        assert_eq!(player.fade_alpha(), 0.0);

        // At t = 9.25 (halfway through fade-out), alpha is ~0.5
        player.position_us.store(9_250_000, Ordering::Relaxed);
        let alpha = player.fade_alpha();
        assert!((alpha - 0.5).abs() < 0.01);
        assert!(!player.is_in_fade_in_phase());

        // At t = 9.5 (midpoint, completely black), alpha is 1.0
        player.position_us.store(9_500_000, Ordering::Relaxed);
        assert_eq!(player.fade_alpha(), 1.0);
        assert!(player.is_in_fade_in_phase());

        // At t = 9.75 (halfway through fade-in), alpha is ~0.5
        player.position_us.store(9_750_000, Ordering::Relaxed);
        let alpha = player.fade_alpha();
        assert!((alpha - 0.5).abs() < 0.01);
        assert!(player.is_in_fade_in_phase());

        // At t = 10.0 (end of transition), alpha is 0.0
        player.position_us.store(10_000_000, Ordering::Relaxed);
        assert_eq!(player.fade_alpha(), 0.0);
        assert!(player.is_in_fade_in_phase());
        assert!(player.is_completed());
    }

    #[test]
    fn test_outro_fade_alpha_calculations() {
        let player = PrerollPlayer {
            mode: RollMode::Outro,
            duration_secs: 10.0,
            position_us: Arc::new(AtomicI64::new(0)),
            is_finished: Arc::new(AtomicBool::new(false)),
            stop_requested: Arc::new(AtomicBool::new(false)),
            frame_rx: crossbeam_channel::unbounded().1,
            latest_frame: None,
            thread_handle: None,
            audio_tap: Arc::new(AudioTap::default()),
            transition_duration_secs: 1.0,
        };

        // At t = 0 (start of outro transition), alpha is 0.0, in fade-out phase (live stream visible)
        assert_eq!(player.fade_alpha(), 0.0);
        assert!(player.is_in_fade_out_phase());

        // At t = 0.25 (halfway through fade-out of live stream), alpha is ~0.5
        player.position_us.store(250_000, Ordering::Relaxed);
        let alpha = player.fade_alpha();
        assert!((alpha - 0.5).abs() < 0.01);
        assert!(player.is_in_fade_out_phase());

        // At t = 0.5 (midpoint, completely black, outro video takes over), alpha is 1.0
        player.position_us.store(500_000, Ordering::Relaxed);
        assert_eq!(player.fade_alpha(), 1.0);
        assert!(!player.is_in_fade_out_phase());

        // At t = 0.75 (halfway through fade-in of outro video), alpha is ~0.5
        player.position_us.store(750_000, Ordering::Relaxed);
        let alpha = player.fade_alpha();
        assert!((alpha - 0.5).abs() < 0.01);
        assert!(!player.is_in_fade_out_phase());

        // At t = 1.0 (end of entry transition), alpha is 0.0
        player.position_us.store(1_000_000, Ordering::Relaxed);
        assert_eq!(player.fade_alpha(), 0.0);
        assert!(!player.is_in_fade_out_phase());
        assert!(!player.is_completed());

        // At t = 5.0 (middle of outro playback), alpha is 0.0
        player.position_us.store(5_000_000, Ordering::Relaxed);
        assert_eq!(player.fade_alpha(), 0.0);
        assert!(!player.is_in_fade_out_phase());

        // At t = 10.0 (end of outro video), completed is true
        player.position_us.store(10_000_000, Ordering::Relaxed);
        assert_eq!(player.fade_alpha(), 0.0);
        assert!(player.is_completed());
    }

    #[test]
    fn test_remaining_secs() {
        let player = PrerollPlayer {
            mode: RollMode::Preroll,
            duration_secs: 15.0,
            position_us: Arc::new(AtomicI64::new(5_000_000)),
            is_finished: Arc::new(AtomicBool::new(false)),
            stop_requested: Arc::new(AtomicBool::new(false)),
            frame_rx: crossbeam_channel::unbounded().1,
            latest_frame: None,
            thread_handle: None,
            audio_tap: Arc::new(AudioTap::default()),
            transition_duration_secs: 1.0,
        };

        assert!((player.remaining_secs() - 10.0).abs() < 0.001);
    }
}
