//! Standalone "Replay Buffer Scrub Player" window (opened with F5):
//! previews a snapshot of the replay buffer at the moment F5 was pressed,
//! allows scrubbing and placing Start/End markers, and saves the trimmed clip to disk.

use crate::app::AppState;
use crate::replay::ScrubSnapshot;
use crate::video::player::{PlayerCommand, PlayerEvent, VideoFileConfig};
use crate::video::types::{checkout_frame_buffer, ColorRange, RawFrame, ScalerFilter};
use crossbeam_channel::Receiver;
use eframe::egui::{self, Color32, RichText};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

/// Designed size (points) of the Replay Buffer Scrub Player window.
const SCRUB_WINDOW_SIZE: egui::Vec2 = egui::vec2(960.0, 680.0);

/// Skip step in seconds for the Skip Back / Skip Forward buttons and Left/Right arrow keys.
const SKIP_STEP_SECS: f64 = 5.0;

/// Maximum preview texture dimensions uploaded for the scrub player.
const MAX_PREVIEW_WIDTH: u32 = 1280;
const MAX_PREVIEW_HEIGHT: u32 = 720;

/// Extra trailing buffer bytes reserved so `libswscale` SIMD horizontal filter routines
/// never read or write past allocation bounds on tight scanlines.
const SWS_SIMD_PADDING_BYTES: usize = 64;

pub struct ScrubPlayerHandle {
    pub command_sender: crossbeam_channel::Sender<PlayerCommand>,
    pub rgba_rx: Receiver<egui::ColorImage>,
    pub event_rx: Receiver<PlayerEvent>,
    pub position_us: Arc<AtomicI64>,
    pub is_paused_shared: Arc<AtomicBool>,
    pub is_eof_shared: Arc<AtomicBool>,
    pub stop_requested: Arc<AtomicBool>,
    pub player_thread: Option<JoinHandle<()>>,
    pub converter_thread: Option<JoinHandle<()>>,
}

impl Drop for ScrubPlayerHandle {
    fn drop(&mut self) {
        self.stop_requested.store(true, Ordering::Release);
        let _ = self.command_sender.send(PlayerCommand::Stop);
        let player_handle = self.player_thread.take();
        let converter_handle = self.converter_thread.take();
        if player_handle.is_some() || converter_handle.is_some() {
            let _ = std::thread::Builder::new()
                .name("scrub-player-cleanup".into())
                .spawn(move || {
                    if let Some(handle) = player_handle {
                        let _ = handle.join();
                    }
                    if let Some(handle) = converter_handle {
                        let _ = handle.join();
                    }
                });
        }
    }
}

#[derive(Default)]
pub struct ReplayScrubState {
    pub window_open: bool,
    pub loading: bool,
    pub snapshot: Option<ScrubSnapshot>,
    pub player: Option<ScrubPlayerHandle>,
    pub current_position_secs: f64,
    pub duration_secs: f64,
    pub is_paused: bool,
    pub is_eof: bool,
    pub is_seeking: bool,
    pub was_playing_before_scrub: bool,
    pub start_marker_secs: Option<f64>,
    pub end_marker_secs: Option<f64>,
    pub last_seek_sent_secs: Option<f64>,
    pub pending_loop_seek_until: Option<std::time::Instant>,
    pub texture: Option<egui::TextureHandle>,
    pub video_size: (u32, u32),
}

impl ReplayScrubState {
    pub fn is_open(&self) -> bool {
        self.window_open
    }

    pub fn open_pending(&mut self) {
        self.player = None;
        self.snapshot = None;
        self.texture = None;
        self.window_open = false;
        self.loading = true;
        self.current_position_secs = 0.0;
        self.duration_secs = 0.0;
        self.is_paused = false;
        self.is_eof = false;
        self.is_seeking = false;
        self.was_playing_before_scrub = false;
        self.start_marker_secs = None;
        self.end_marker_secs = None;
        self.last_seek_sent_secs = None;
        self.pending_loop_seek_until = None;
    }

    pub fn close(&mut self) {
        self.player = None;
        self.snapshot = None;
        self.texture = None;
        self.window_open = false;
        self.loading = false;
        self.is_seeking = false;
        self.was_playing_before_scrub = false;
        self.start_marker_secs = None;
        self.end_marker_secs = None;
        self.last_seek_sent_secs = None;
        self.pending_loop_seek_until = None;
    }

    pub fn start_with_snapshot(
        &mut self,
        snapshot: ScrubSnapshot,
        master_volume: f32,
        scaler_filter: Arc<AtomicU8>,
        color_range: Arc<AtomicU8>,
        ctx: &egui::Context,
    ) {
        self.player = None;
        self.texture = None;
        self.duration_secs = snapshot.duration_secs.max(0.1);
        self.current_position_secs = 0.0;
        self.video_size = (snapshot.width.max(1), snapshot.height.max(1));
        self.is_paused = false;
        self.is_eof = false;
        self.is_seeking = false;
        self.was_playing_before_scrub = false;
        self.start_marker_secs = None;
        self.end_marker_secs = None;
        self.last_seek_sent_secs = None;
        self.pending_loop_seek_until = None;
        self.loading = false;
        self.window_open = true;

        let preview_path = snapshot.preview_file.path().to_path_buf();
        self.snapshot = Some(snapshot);

        let (raw_frame_tx, raw_frame_rx) = crossbeam_channel::bounded::<Arc<RawFrame>>(16);
        let (rgba_tx, rgba_rx) = crossbeam_channel::bounded::<egui::ColorImage>(2);
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<PlayerEvent>();
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<PlayerCommand>();

        let position_us = Arc::new(AtomicI64::new(0));
        let is_paused = Arc::new(AtomicBool::new(false));
        let is_eof = Arc::new(AtomicBool::new(false));
        let stop_requested = Arc::new(AtomicBool::new(false));

        let repaint_ctx = ctx.clone();
        let request_repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            // Wake the root viewport so `AppState::update` runs `draw_replay_scrub_window`
            // and repaints the immediate scrub viewport. Never call `request_repaint_of`
            // with an unscaled `ViewportId::from_hash_of("replay_scrub_window")` here:
            // `egui::Context::request_repaint_of` inserts a default `ViewportState` entry
            // with `parent = ROOT` into `ctx.viewports`, which survives the immediate
            // viewport's `end_frame` pass and causes `eframe` to spawn and immediately drop
            // a phantom OS window (and Wayland clipboard device) on every decoded frame.
            repaint_ctx.request_repaint_of(egui::ViewportId::ROOT);
        });
        let notify_event = Arc::clone(&request_repaint);
        let convert_repaint = Arc::clone(&request_repaint);
        let convert_stop = Arc::clone(&stop_requested);
        let convert_scaler_filter = Arc::clone(&scaler_filter);
        let rgba_rx_evict = rgba_rx.clone();

        let converter_thread = thread::Builder::new()
            .name("replay-scrub-rgba".into())
            .spawn(move || {
                let mut converter = ScrubFrameConverter::default();
                while let Ok(mut frame) = raw_frame_rx.recv() {
                    if convert_stop.load(Ordering::Acquire) {
                        break;
                    }
                    // Coalesce burst decode/seek backlog so only the freshest frame is scaled.
                    while let Ok(newer) = raw_frame_rx.try_recv() {
                        frame = newer;
                    }
                    if convert_stop.load(Ordering::Acquire) {
                        break;
                    }
                    let filter =
                        ScalerFilter::from_u8(convert_scaler_filter.load(Ordering::Relaxed));
                    if let Some(color_image) =
                        raw_frame_to_color_image(&frame, &mut converter, filter)
                    {
                        if rgba_tx.is_full() {
                            let _ = rgba_rx_evict.try_recv();
                        }
                        if rgba_tx.try_send(color_image).is_ok() {
                            convert_repaint();
                        }
                    }
                }
            })
            .ok();

        // Use an isolated, disabled AudioTap so the scrub player's audio plays only to the
        // local speakers and never leaks into the background replay buffer or Twitch broadcast.
        let isolated_audio_tap = Arc::new(crate::replay::AudioTap::default());

        let config = VideoFileConfig {
            file_path: preview_path,
            scaler_filter,
            color_range,
            stop_requested: Arc::clone(&stop_requested),
            request_repaint,
            peak_amplitude: Arc::new(AtomicU64::new(0)),
            audio_latency_ms: Arc::new(AtomicU64::new(0)),
            replay_audio: isolated_audio_tap,
            master_volume: master_volume.clamp(0.0, 1.0),
        };

        let pos_clone = Arc::clone(&position_us);
        let paused_clone = Arc::clone(&is_paused);
        let eof_clone = Arc::clone(&is_eof);
        let event_tx_clone = event_tx.clone();

        let handle = thread::Builder::new()
            .name("replay-scrub-player".into())
            .spawn(move || {
                let result = crate::video::player::video_file_thread_main(
                    raw_frame_tx,
                    event_tx_clone.clone(),
                    cmd_rx,
                    pos_clone,
                    paused_clone,
                    eof_clone,
                    config,
                );
                match result {
                    Ok(()) => {
                        let _ = event_tx_clone.send(PlayerEvent::Stopped);
                    }
                    Err(e) => {
                        let _ = event_tx_clone.send(PlayerEvent::Failed(format!("{e:#}")));
                    }
                }
                notify_event();
            })
            .ok();

        self.player = Some(ScrubPlayerHandle {
            command_sender: cmd_tx,
            rgba_rx,
            event_rx,
            position_us,
            is_paused_shared: is_paused,
            is_eof_shared: is_eof,
            stop_requested,
            player_thread: handle,
            converter_thread,
        });
    }

    /// Returns `(effective_start_secs, effective_end_secs)` using `0.0` and `duration_secs`
    /// when no start or end markers have been placed by the user.
    pub fn effective_markers(&self) -> (f64, f64) {
        let dur = self.duration_secs.max(0.0);
        let start = self.start_marker_secs.unwrap_or(0.0).clamp(0.0, dur);
        let end = self.end_marker_secs.unwrap_or(dur).clamp(0.0, dur);
        if start <= end {
            (start, end)
        } else {
            (end, start)
        }
    }

    pub fn place_start_here(&mut self) {
        let pos = self
            .current_position_secs
            .clamp(0.0, self.duration_secs.max(0.0));
        self.start_marker_secs = Some(pos);
        if let Some(end) = self.end_marker_secs {
            if end <= pos + 0.05 {
                self.end_marker_secs = None;
            }
        }
    }

    pub fn place_end_here(&mut self) {
        let pos = self
            .current_position_secs
            .clamp(0.0, self.duration_secs.max(0.0));
        self.end_marker_secs = Some(pos);
        if let Some(start) = self.start_marker_secs {
            if start >= pos - 0.05 {
                self.start_marker_secs = None;
            }
        }
    }

    pub fn clear_markers(&mut self) {
        self.start_marker_secs = None;
        self.end_marker_secs = None;
    }

    pub fn seek_to(&mut self, target_secs: f64) {
        let clamped = target_secs.clamp(0.0, self.duration_secs.max(0.0));
        self.current_position_secs = clamped;
        if self
            .last_seek_sent_secs
            .is_some_and(|prev| (clamped - prev).abs() < 0.002)
        {
            return;
        }
        self.last_seek_sent_secs = Some(clamped);
        if let Some(ref player) = self.player {
            player
                .position_us
                .store((clamped * 1_000_000.0) as i64, Ordering::Relaxed);
            let _ = player.command_sender.send(PlayerCommand::Seek(clamped));
        }
    }

    pub fn loop_to_start(&mut self) {
        let (eff_start, _) = self.effective_markers();
        let clamped = eff_start.clamp(0.0, self.duration_secs.max(0.0));
        self.current_position_secs = clamped;
        self.last_seek_sent_secs = Some(clamped);
        self.is_eof = false;
        self.is_paused = false;
        self.pending_loop_seek_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(250));
        if let Some(ref player) = self.player {
            player
                .position_us
                .store((clamped * 1_000_000.0) as i64, Ordering::Relaxed);
            player.is_eof_shared.store(false, Ordering::Relaxed);
            player.is_paused_shared.store(false, Ordering::Relaxed);
            let _ = player.command_sender.send(PlayerCommand::Seek(clamped));
            let _ = player.command_sender.send(PlayerCommand::Play);
        }
    }

    pub fn toggle_pause(&mut self) {
        if self.is_paused || self.is_eof {
            let (_, eff_end) = self.effective_markers();
            if self.is_eof || self.current_position_secs >= eff_end - 0.02 {
                self.loop_to_start();
                return;
            }
            self.is_paused = false;
            self.is_eof = false;
            if let Some(ref player) = self.player {
                player.is_paused_shared.store(false, Ordering::Relaxed);
                player.is_eof_shared.store(false, Ordering::Relaxed);
                let _ = player.command_sender.send(PlayerCommand::Play);
            }
        } else {
            self.is_paused = true;
            if let Some(ref player) = self.player {
                player.is_paused_shared.store(true, Ordering::Relaxed);
                let _ = player.command_sender.send(PlayerCommand::Pause);
            }
        }
    }
}

pub fn format_time_tenths(seconds: f64) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return "00:00.0".to_string();
    }
    let total_tenths = (seconds * 10.0).round() as u64;
    let tenths = total_tenths % 10;
    let total_secs = total_tenths / 10;
    let minutes = total_secs / 60;
    let secs = total_secs % 60;
    format!("{:02}:{:02}.{}", minutes, secs, tenths)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScrubScalerKey {
    src_format: ffmpeg_next::format::Pixel,
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    color_range: ColorRange,
    flags_bits: libc::c_int,
}

/// Persistent YUV/NV12/RGBA -> preview RGBA converter that caches `swscale::Context`
/// across frames and writes directly into pooled `RawFrame` byte buffers.
#[derive(Default)]
pub struct ScrubFrameConverter {
    scaler: Option<ffmpeg_next::software::scaling::context::Context>,
    key: Option<ScrubScalerKey>,
    scratch_src: Vec<u8>,
    #[cfg(test)]
    reinit_count: usize,
}

#[inline]
fn preview_scaled_dimensions(src_w: u32, src_h: u32) -> (u32, u32) {
    if src_w > MAX_PREVIEW_WIDTH || src_h > MAX_PREVIEW_HEIGHT {
        let scale =
            (MAX_PREVIEW_WIDTH as f32 / src_w as f32).min(MAX_PREVIEW_HEIGHT as f32 / src_h as f32);
        let w = ((src_w as f32 * scale).round() as u32).max(2) & !1;
        let h = ((src_h as f32 * scale).round() as u32).max(2) & !1;
        (w, h)
    } else {
        (src_w, src_h)
    }
}

/// Computes tight `(plane_offsets, plane_strides, total_bytes)` for supported `RawFrame` formats.
fn raw_frame_plane_layout(
    format: ffmpeg_next::format::Pixel,
    width: u32,
    height: u32,
) -> Option<([usize; 3], [libc::c_int; 4], usize)> {
    use ffmpeg_next::format::Pixel;

    let w = width as usize;
    let h = height as usize;
    match format {
        Pixel::RGBA => {
            let row0 = w.checked_mul(4)?;
            let total = row0.checked_mul(h)?;
            Some(([0, 0, 0], [row0 as libc::c_int, 0, 0, 0], total))
        }
        Pixel::YUYV422 => {
            let row0 = w.checked_mul(2)?;
            let total = row0.checked_mul(h)?;
            Some(([0, 0, 0], [row0 as libc::c_int, 0, 0, 0], total))
        }
        Pixel::NV12 => {
            let y_bytes = w.checked_mul(h)?;
            let cw = w.div_ceil(2);
            let ch = h.div_ceil(2);
            let uv_row = cw.checked_mul(2)?;
            let uv_bytes = uv_row.checked_mul(ch)?;
            let total = y_bytes.checked_add(uv_bytes)?;
            Some((
                [0, y_bytes, 0],
                [w as libc::c_int, uv_row as libc::c_int, 0, 0],
                total,
            ))
        }
        Pixel::YUV420P | Pixel::YUVJ420P => {
            let y_bytes = w.checked_mul(h)?;
            let cw = w.div_ceil(2);
            let ch = h.div_ceil(2);
            let c_bytes = cw.checked_mul(ch)?;
            let u_end = y_bytes.checked_add(c_bytes)?;
            let total = u_end.checked_add(c_bytes)?;
            Some((
                [0, y_bytes, u_end],
                [w as libc::c_int, cw as libc::c_int, cw as libc::c_int, 0],
                total,
            ))
        }
        Pixel::YUV422P | Pixel::YUVJ422P => {
            let y_bytes = w.checked_mul(h)?;
            let cw = w.div_ceil(2);
            let c_bytes = cw.checked_mul(h)?;
            let u_end = y_bytes.checked_add(c_bytes)?;
            let total = u_end.checked_add(c_bytes)?;
            Some((
                [0, y_bytes, u_end],
                [w as libc::c_int, cw as libc::c_int, cw as libc::c_int, 0],
                total,
            ))
        }
        _ => None,
    }
}

impl ScrubFrameConverter {
    #[cfg(test)]
    pub fn reinit_count(&self) -> usize {
        self.reinit_count
    }

    /// Converts `frame` into a pooled `Pixel::RGBA` `RawFrame` (scaled to at most `1280x720`),
    /// reusing the cached `swscale::Context` whenever format, dimensions, range, and filter match.
    pub fn convert_to_rgba_frame(
        &mut self,
        frame: &RawFrame,
        scaler_filter: ScalerFilter,
    ) -> Option<RawFrame> {
        use ffmpeg_next::format::Pixel;

        if frame.width == 0 || frame.height == 0 {
            return None;
        }

        let (dst_w, dst_h) = preview_scaled_dimensions(frame.width, frame.height);

        if frame.format == Pixel::RGBA && frame.width == dst_w && frame.height == dst_h {
            let needed = (dst_w as usize)
                .checked_mul(dst_h as usize)?
                .checked_mul(4)?;
            if frame.data.len() < needed {
                return None;
            }
            let mut out = checkout_frame_buffer(needed);
            out.copy_from_slice(&frame.data[..needed]);
            return Some(RawFrame {
                captured_at: frame.captured_at,
                rate: frame.rate,
                width: dst_w,
                height: dst_h,
                data: out,
                format: Pixel::RGBA,
                color_range: ColorRange::Full,
            });
        }

        let (offsets, src_strides, total_src_bytes) =
            raw_frame_plane_layout(frame.format, frame.width, frame.height)?;
        if frame.data.len() < total_src_bytes {
            return None;
        }

        let (sws_src_format, effective_color_range) = match frame.format {
            Pixel::YUVJ420P => (Pixel::YUV420P, ColorRange::Full),
            Pixel::YUVJ422P => (Pixel::YUV422P, ColorRange::Full),
            other => (other, frame.color_range),
        };
        let flags = scaler_filter.into_ffmpeg_flag();
        let desired_key = ScrubScalerKey {
            src_format: sws_src_format,
            src_w: frame.width,
            src_h: frame.height,
            dst_w,
            dst_h,
            color_range: effective_color_range,
            flags_bits: flags.bits(),
        };

        if self.key != Some(desired_key) || self.scaler.is_none() {
            let mut new_scaler = ffmpeg_next::software::scaling::context::Context::get(
                sws_src_format,
                frame.width,
                frame.height,
                Pixel::RGBA,
                dst_w,
                dst_h,
                flags,
            )
            .ok()?;

            if sws_src_format != Pixel::RGBA {
                unsafe {
                    let cs = if frame.width >= 1280 || frame.height >= 720 {
                        ffmpeg_next::sys::SWS_CS_ITU709
                    } else {
                        ffmpeg_next::sys::SWS_CS_ITU601
                    };
                    let coeff = ffmpeg_next::sys::sws_getCoefficients(cs);
                    let src_range = match effective_color_range {
                        ColorRange::Full => 1,
                        ColorRange::Limited => 0,
                    };
                    let _ = ffmpeg_next::sys::sws_setColorspaceDetails(
                        new_scaler.as_mut_ptr(),
                        coeff,
                        src_range,
                        coeff,
                        1,
                        0,
                        1 << 16,
                        1 << 16,
                    );
                }
            }

            self.scaler = Some(new_scaler);
            self.key = Some(desired_key);
            #[cfg(test)]
            {
                self.reinit_count += 1;
            }
        }

        let min_padded_src = total_src_bytes.checked_add(SWS_SIMD_PADDING_BYTES)?;
        let base_src_ptr = if frame.data.capacity() >= min_padded_src {
            frame.data.as_ptr()
        } else {
            if self.scratch_src.len() < min_padded_src {
                self.scratch_src.resize(min_padded_src, 0);
            }
            self.scratch_src[..total_src_bytes].copy_from_slice(&frame.data[..total_src_bytes]);
            self.scratch_src.as_ptr()
        };

        let mut src_ptrs = [std::ptr::null::<u8>(); 4];
        for i in 0..3 {
            if src_strides[i] > 0 {
                src_ptrs[i] = unsafe { base_src_ptr.add(offsets[i]) };
            }
        }

        let dst_row_bytes = (dst_w as usize).checked_mul(4)?;
        let dst_bytes = dst_row_bytes.checked_mul(dst_h as usize)?;
        let mut rgba = checkout_frame_buffer(dst_bytes.checked_add(SWS_SIMD_PADDING_BYTES)?);
        let dst_ptrs: [*mut u8; 4] = [
            rgba.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ];
        let dst_strides: [libc::c_int; 4] = [dst_row_bytes as libc::c_int, 0, 0, 0];

        let scaler = self.scaler.as_mut()?;
        let scaled_rows = unsafe {
            ffmpeg_next::sys::sws_scale(
                scaler.as_mut_ptr(),
                src_ptrs.as_ptr(),
                src_strides.as_ptr(),
                0,
                frame.height as libc::c_int,
                dst_ptrs.as_ptr(),
                dst_strides.as_ptr(),
            )
        };
        if scaled_rows <= 0 {
            return None;
        }
        rgba.truncate(dst_bytes);

        Some(RawFrame {
            captured_at: frame.captured_at,
            rate: frame.rate,
            width: dst_w,
            height: dst_h,
            data: rgba,
            format: Pixel::RGBA,
            color_range: ColorRange::Full,
        })
    }

    /// Converts `frame` into an `egui::ColorImage` ready for GPU texture upload,
    /// reusing the cached `swscale::Context` and recycling intermediate pooled buffers.
    pub fn convert_to_color_image(
        &mut self,
        frame: &RawFrame,
        scaler_filter: ScalerFilter,
    ) -> Option<egui::ColorImage> {
        use ffmpeg_next::format::Pixel;

        if frame.width == 0 || frame.height == 0 {
            return None;
        }

        if frame.format == Pixel::RGBA
            && frame.width <= MAX_PREVIEW_WIDTH
            && frame.height <= MAX_PREVIEW_HEIGHT
        {
            let needed = (frame.width as usize)
                .checked_mul(frame.height as usize)?
                .checked_mul(4)?;
            if frame.data.len() < needed {
                return None;
            }
            return Some(egui::ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.data[..needed],
            ));
        }

        let rgba_frame = self.convert_to_rgba_frame(frame, scaler_filter)?;
        Some(egui::ColorImage::from_rgba_unmultiplied(
            [rgba_frame.width as usize, rgba_frame.height as usize],
            &rgba_frame.data,
        ))
    }
}

pub(crate) fn raw_frame_to_color_image(
    frame: &RawFrame,
    converter: &mut ScrubFrameConverter,
    scaler_filter: ScalerFilter,
) -> Option<egui::ColorImage> {
    converter.convert_to_color_image(frame, scaler_filter)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScrubAction {
    None,
    Cancel,
    Save,
}

pub fn draw_replay_scrub_window(state: &mut AppState, ctx: &egui::Context) -> bool {
    if state.steam_deck.is_some() {
        state.replay.scrub_open_requested = false;
        if state.replay_scrub.is_open() || state.replay_scrub.loading {
            state.replay_scrub.close();
            state.sync_local_audio_volume();
        }
        return false;
    }

    let mut repaint = false;

    if state.replay.scrub_open_requested {
        state.replay.scrub_open_requested = false;
        if !state.replay_scrub.is_open() && !state.replay_scrub.loading {
            if state.replay.request_scrub_preview() {
                state.replay_scrub.open_pending();
                repaint = true;
            }
        }
    }

    if let Some(snapshot_res) = state.replay.take_scrub_snapshot() {
        match snapshot_res {
            Ok(snapshot) if snapshot.duration_secs >= crate::replay::MIN_SCRUB_DURATION_SECS => {
                if state.replay_scrub.loading || state.replay_scrub.is_open() {
                    state.replay_scrub.start_with_snapshot(
                        snapshot,
                        state.hardware.master_volume,
                        state.scaler_filter.clone(),
                        state.color_range.clone(),
                        ctx,
                    );
                    state.sync_local_audio_volume();
                    repaint = true;
                }
            }
            _ => {
                // Fewer than 5 seconds or snapshot unavailable: silently ignore without opening view or notifying user
                state.replay_scrub.close();
                state.sync_local_audio_volume();
            }
        }
    }

    if state.replay_scrub.loading && !state.replay_scrub.is_open() {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
        return repaint;
    }

    if !state.replay_scrub.is_open() || !state.ui.scale.settled_once {
        return repaint;
    }

    // Poll pre-converted RGBA scrub frames and player events
    let mut latest_image = None;
    let mut failed_error = None;
    let mut hit_eof = false;
    let mut raw_pos_secs = state.replay_scrub.current_position_secs;
    if let Some(ref mut player) = state.replay_scrub.player {
        while let Ok(image) = player.rgba_rx.try_recv() {
            latest_image = Some(image);
        }
        while let Ok(event) = player.event_rx.try_recv() {
            match event {
                PlayerEvent::Started {
                    width,
                    height,
                    duration_secs,
                    ..
                } => {
                    if width > 0 && height > 0 {
                        state.replay_scrub.video_size = (width, height);
                    }
                    if duration_secs > 0.0 {
                        state.replay_scrub.duration_secs = duration_secs;
                    }
                    repaint = true;
                }
                PlayerEvent::Eof => {
                    hit_eof = true;
                    repaint = true;
                }
                PlayerEvent::Failed(err) => {
                    failed_error = Some(err);
                }
                PlayerEvent::Stopped => {}
            }
        }
        raw_pos_secs = (player.position_us.load(Ordering::Relaxed) as f64 / 1_000_000.0)
            .clamp(0.0, state.replay_scrub.duration_secs.max(0.0));
        state.replay_scrub.is_paused = player.is_paused_shared.load(Ordering::Relaxed);
        state.replay_scrub.is_eof = player.is_eof_shared.load(Ordering::Relaxed);
    }

    let (eff_start, eff_end) = state.replay_scrub.effective_markers();
    if hit_eof && !state.replay_scrub.is_seeking {
        state.replay_scrub.loop_to_start();
        repaint = true;
    } else if !state.replay_scrub.is_seeking {
        let waiting_for_loop_seek = state
            .replay_scrub
            .pending_loop_seek_until
            .is_some_and(|until| std::time::Instant::now() < until);
        if waiting_for_loop_seek {
            if raw_pos_secs < eff_end - 0.05 {
                state.replay_scrub.pending_loop_seek_until = None;
                state.replay_scrub.current_position_secs = raw_pos_secs;
            }
        } else {
            state.replay_scrub.pending_loop_seek_until = None;
            if !state.replay_scrub.is_paused {
                state.replay_scrub.current_position_secs = raw_pos_secs;
                if eff_end > eff_start + 0.05 && state.replay_scrub.current_position_secs >= eff_end
                {
                    state.replay_scrub.loop_to_start();
                    repaint = true;
                }
            }
        }
    }

    if let Some(err) = failed_error {
        state.replay_scrub.close();
        state.sync_local_audio_volume();
        state.error(format!("Replay scrub preview error: {err}"));
        return true;
    }

    if let Some(color_image) = latest_image {
        match &mut state.replay_scrub.texture {
            Some(tex) => tex.set(color_image, egui::TextureOptions::LINEAR),
            None => {
                state.replay_scrub.texture = Some(ctx.load_texture(
                    "replay_scrub_preview",
                    color_image,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        repaint = true;
    }

    let size = crate::ui::scaling::fit_window(
        SCRUB_WINDOW_SIZE,
        crate::ui::scaling::monitor_size(ctx, &state.ui.scale),
    )
    .size;

    let mut action = ScrubAction::None;

    ctx.show_viewport_immediate(
        crate::ui::scaling::viewport_id("replay_scrub_window", ctx, size),
        egui::ViewportBuilder::default()
            .with_title("Michadame Replay Scrub Player")
            .with_inner_size(size)
            .with_min_inner_size(egui::vec2(680.0, 480.0).min(size)),
        |ctx, _class| {
            state.ui.any_pointer_down |= ctx.input(|i| i.pointer.any_down());
            action = draw_scrub_viewport_contents(&mut state.replay_scrub, ctx);
            if ctx.input(|i| i.viewport().close_requested()) && action == ScrubAction::None {
                action = ScrubAction::Cancel;
            }
        },
    );

    match action {
        ScrubAction::None => {}
        ScrubAction::Cancel => {
            state.replay_scrub.close();
            state.sync_local_audio_volume();
            repaint = true;
        }
        ScrubAction::Save => {
            let (start_secs, end_secs) = state.replay_scrub.effective_markers();
            let snapshot_opt = state.replay_scrub.snapshot.take();
            state.replay_scrub.close();
            state.sync_local_audio_volume();
            if let Some(snapshot) = snapshot_opt {
                if let Err(e) = state.replay.save_trimmed(snapshot, start_secs, end_secs) {
                    state.error(format!("Replay save failed: {e}"));
                }
            }
            repaint = true;
        }
    }

    if state.replay_scrub.is_open()
        && (state.replay_scrub.loading
            || !state.replay_scrub.is_paused
            || state.replay_scrub.is_seeking)
    {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }

    repaint
}

fn draw_scrub_viewport_contents(scrub: &mut ReplayScrubState, ctx: &egui::Context) -> ScrubAction {
    let mut action = ScrubAction::None;

    // Keyboard shortcuts within the scrub player window
    if !ctx.wants_keyboard_input() {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            return ScrubAction::Cancel;
        }
        if !scrub.loading {
            if ctx.input(|i| i.key_pressed(egui::Key::Space) || i.key_pressed(egui::Key::P)) {
                scrub.toggle_pause();
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
                scrub.seek_to(scrub.current_position_secs - SKIP_STEP_SECS);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
                scrub.seek_to(scrub.current_position_secs + SKIP_STEP_SECS);
            }
        }
    }

    // --- Top Header Panel ---
    egui::TopBottomPanel::top("replay_scrub_header")
        .frame(
            egui::Frame::none()
                .fill(Color32::from_rgb(20, 20, 22))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(52, 52, 56)))
                .inner_margin(egui::Margin::symmetric(14.0, 10.0)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("◆ Replay Buffer Scrub Player")
                        .monospace()
                        .size(14.0)
                        .strong()
                        .color(Color32::from_rgb(235, 235, 240)),
                );
                if !scrub.loading && scrub.duration_secs > 0.0 {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!(
                            "({} · {}×{})",
                            format_time_tenths(scrub.duration_secs),
                            scrub.video_size.0,
                            scrub.video_size.1
                        ))
                        .monospace()
                        .size(11.5)
                        .color(Color32::from_rgb(150, 150, 160)),
                    );
                }
            });
        });

    // --- Bottom Controls Panel ---
    egui::TopBottomPanel::bottom("replay_scrub_controls")
        .frame(
            egui::Frame::none()
                .fill(Color32::from_rgb(22, 22, 24))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(52, 52, 56)))
                .inner_margin(egui::Margin::symmetric(14.0, 12.0)),
        )
        .show(ctx, |ui| {
            ui.set_enabled(!scrub.loading);

            let duration = scrub.duration_secs.max(0.1);
            let (eff_start, eff_end) = scrub.effective_markers();
            let clip_duration = (eff_end - eff_start).max(0.0);

            // 1. Single Scrub Bar with Markers & Playhead
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format_time_tenths(scrub.current_position_secs))
                        .monospace()
                        .size(12.0)
                        .strong()
                        .color(Color32::WHITE),
                );

                let avail_bar_w = (ui.available_width() - 68.0).max(120.0);
                draw_timeline_marker_bar(ui, scrub, avail_bar_w, duration, eff_start, eff_end);

                ui.label(
                    RichText::new(format_time_tenths(duration))
                        .monospace()
                        .size(12.0)
                        .color(Color32::from_rgb(165, 165, 175)),
                );
            });

            ui.add_space(6.0);

            // 2. Marker Readout Row
            ui.horizontal(|ui| {
                let start_label = match scrub.start_marker_secs {
                    Some(s) => format!("Start: {}", format_time_tenths(s)),
                    None => "Start: 00:00.0".to_string(),
                };
                let end_label = match scrub.end_marker_secs {
                    Some(e) => format!("End: {}", format_time_tenths(e)),
                    None => format!("End: {}", format_time_tenths(duration)),
                };
                ui.label(
                    RichText::new(start_label)
                        .monospace()
                        .size(11.0)
                        .color(if scrub.start_marker_secs.is_some() {
                            Color32::from_rgb(90, 230, 145)
                        } else {
                            Color32::from_rgb(145, 145, 155)
                        }),
                );
                ui.label(
                    RichText::new("·")
                        .monospace()
                        .size(11.0)
                        .color(Color32::from_rgb(100, 100, 110)),
                );
                ui.label(
                    RichText::new(end_label)
                        .monospace()
                        .size(11.0)
                        .color(if scrub.end_marker_secs.is_some() {
                            Color32::from_rgb(255, 185, 80)
                        } else {
                            Color32::from_rgb(145, 145, 155)
                        }),
                );
                ui.label(
                    RichText::new("·")
                        .monospace()
                        .size(11.0)
                        .color(Color32::from_rgb(100, 100, 110)),
                );
                ui.label(
                    RichText::new(format!("Selected Clip: {:.1}s", clip_duration))
                        .monospace()
                        .size(11.0)
                        .strong()
                        .color(Color32::from_rgb(130, 195, 255)),
                );
            });

            ui.add_space(8.0);

            // 3. Play/Pause, Start/End Marker, and Cancel/Save Buttons Row
            ui.horizontal(|ui| {
                // Play / Pause
                let play_label = if scrub.is_paused || scrub.is_eof {
                    "▶ Play"
                } else {
                    "⏸ Pause"
                };
                let play_btn = egui::Button::new(
                    RichText::new(play_label)
                        .monospace()
                        .size(12.0)
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(Color32::from_rgb(38, 78, 122))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(80, 140, 205)))
                .rounding(egui::Rounding::ZERO);
                if ui
                    .add_sized([86.0, 28.0], play_btn)
                    .on_hover_text("Play or pause preview (Space / P)")
                    .clicked()
                {
                    scrub.toggle_pause();
                }

                ui.add_space(12.0);

                // Start Marker
                let start_btn = egui::Button::new(
                    RichText::new("Start Marker")
                        .monospace()
                        .size(11.5)
                        .strong()
                        .color(Color32::from_rgb(175, 250, 200)),
                )
                .fill(Color32::from_rgb(24, 66, 40))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(65, 165, 100)))
                .rounding(egui::Rounding::ZERO);
                if ui
                    .add_sized([116.0, 28.0], start_btn)
                    .on_hover_text("Set the clip Start marker at the current cursor position")
                    .clicked()
                {
                    scrub.place_start_here();
                }

                // End Marker
                let end_btn = egui::Button::new(
                    RichText::new("End Marker")
                        .monospace()
                        .size(11.5)
                        .strong()
                        .color(Color32::from_rgb(255, 220, 160)),
                )
                .fill(Color32::from_rgb(78, 52, 20))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(185, 125, 45)))
                .rounding(egui::Rounding::ZERO);
                if ui
                    .add_sized([116.0, 28.0], end_btn)
                    .on_hover_text("Set the clip End marker at the current cursor position")
                    .clicked()
                {
                    scrub.place_end_here();
                }

                if scrub.start_marker_secs.is_some() || scrub.end_marker_secs.is_some() {
                    let reset_btn = egui::Button::new(
                        RichText::new("Reset")
                            .monospace()
                            .size(11.0)
                            .color(Color32::from_rgb(200, 200, 205)),
                    )
                    .fill(Color32::from_rgb(44, 44, 48))
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(76, 76, 84)))
                    .rounding(egui::Rounding::ZERO);
                    if ui
                        .add_sized([58.0, 28.0], reset_btn)
                        .on_hover_text("Clear Start and End markers (select entire replay buffer)")
                        .clicked()
                    {
                        scrub.clear_markers();
                    }
                }

                // Right-aligned Cancel and Save buttons
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let save_btn = egui::Button::new(
                        RichText::new("💾 Save")
                            .monospace()
                            .size(12.5)
                            .strong()
                            .color(Color32::from_rgb(220, 255, 225)),
                    )
                    .fill(Color32::from_rgb(28, 92, 46))
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(65, 180, 95)))
                    .rounding(egui::Rounding::ZERO);
                    if ui
                        .add_sized([100.0, 28.0], save_btn)
                        .on_hover_text(
                            "Cut the replay buffer from Start to End marker and save to the Michadame replay folder",
                        )
                        .clicked()
                    {
                        action = ScrubAction::Save;
                    }

                    ui.add_space(6.0);

                    let cancel_btn = egui::Button::new(
                        RichText::new("Cancel")
                            .monospace()
                            .size(12.0)
                            .strong()
                            .color(Color32::from_rgb(255, 205, 205)),
                    )
                    .fill(Color32::from_rgb(82, 26, 26))
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(145, 55, 55)))
                    .rounding(egui::Rounding::ZERO);
                    if ui
                        .add_sized([88.0, 28.0], cancel_btn)
                        .on_hover_text("Discard snapshot, close scrub player, and return to application (Esc)")
                        .clicked()
                    {
                        action = ScrubAction::Cancel;
                    }
                });
            });
        });

    // --- Central Video Preview Panel ---
    egui::CentralPanel::default()
        .frame(
            egui::Frame::none()
                .fill(Color32::from_rgb(12, 12, 14))
                .inner_margin(egui::Margin::symmetric(12.0, 10.0)),
        )
        .show(ctx, |ui| {
            let avail = ui.available_size();
            if scrub.loading {
                ui.centered_and_justified(|ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            RichText::new("Preparing replay buffer snapshot preview…")
                                .monospace()
                                .size(13.0)
                                .color(Color32::from_rgb(190, 190, 200)),
                        );
                    });
                });
                return;
            }

            let (vw, vh) = (
                scrub.video_size.0.max(1) as f32,
                scrub.video_size.1.max(1) as f32,
            );
            let scale = (avail.x / vw).min(avail.y / vh).max(0.01);
            let draw_size = egui::vec2(vw * scale, vh * scale);
            let (outer_rect, resp) = ui.allocate_exact_size(avail, egui::Sense::click());
            if resp.clicked() {
                scrub.toggle_pause();
            }

            let video_rect = egui::Rect::from_center_size(outer_rect.center(), draw_size);
            let painter = ui.painter();
            painter.rect_filled(video_rect, 0.0, Color32::BLACK);

            if let Some(ref tex) = scrub.texture {
                painter.image(
                    tex.id(),
                    video_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                painter.text(
                    video_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Loading video frame…",
                    egui::FontId::monospace(12.0),
                    Color32::from_rgb(140, 140, 150),
                );
            }

            painter.rect_stroke(
                video_rect,
                0.0,
                egui::Stroke::new(1.0, Color32::from_rgb(55, 55, 62)),
            );
        });

    action
}

fn draw_timeline_marker_bar(
    ui: &mut egui::Ui,
    scrub: &mut ReplayScrubState,
    bar_width: f32,
    duration: f64,
    eff_start: f64,
    eff_end: f64,
) {
    let bar_height = 22.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(bar_width, bar_height),
        egui::Sense::click_and_drag(),
    );

    if response.drag_started() {
        scrub.is_seeking = true;
        scrub.was_playing_before_scrub = !scrub.is_paused && !scrub.is_eof;
        if let Some(ref player) = scrub.player {
            let _ = player.command_sender.send(PlayerCommand::Pause);
        }
    }
    if (response.clicked() || response.dragged()) && rect.width() > 1.0 {
        if let Some(pos) = response.interact_pointer_pos() {
            let frac = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
            scrub.seek_to(frac * duration);
            ui.ctx().request_repaint();
        }
    }
    if response.drag_stopped() {
        scrub.is_seeking = false;
        if scrub.was_playing_before_scrub {
            let (_, eff_end) = scrub.effective_markers();
            if scrub.current_position_secs >= eff_end - 0.02 {
                scrub.loop_to_start();
            } else if let Some(ref player) = scrub.player {
                player.is_paused_shared.store(false, Ordering::Relaxed);
                let _ = player.command_sender.send(PlayerCommand::Play);
            }
        }
        ui.ctx().request_repaint();
    }

    let painter = ui.painter();
    let track_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 6.0),
        egui::pos2(rect.right(), rect.bottom() - 2.0),
    );
    painter.rect_filled(track_rect, 2.0, Color32::from_rgb(35, 35, 40));
    painter.rect_stroke(
        track_rect,
        2.0,
        egui::Stroke::new(1.0, Color32::from_rgb(60, 60, 68)),
    );

    let x_for_sec = |sec: f64| -> f32 {
        let frac = (sec / duration).clamp(0.0, 1.0) as f32;
        rect.left() + frac * rect.width()
    };

    // Highlighted selection range between Start and End markers
    let start_x = x_for_sec(eff_start);
    let end_x = x_for_sec(eff_end);
    if end_x > start_x {
        let sel_rect = egui::Rect::from_min_max(
            egui::pos2(start_x, track_rect.top()),
            egui::pos2(end_x, track_rect.bottom()),
        );
        painter.rect_filled(
            sel_rect,
            0.0,
            Color32::from_rgba_unmultiplied(45, 135, 200, 110),
        );
    }

    // Start Marker Flag & Vertical Line
    let start_color = if scrub.start_marker_secs.is_some() {
        Color32::from_rgb(75, 235, 135)
    } else {
        Color32::from_rgb(80, 130, 100)
    };
    painter.vline(
        start_x,
        rect.top()..=rect.bottom(),
        egui::Stroke::new(2.0, start_color),
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(start_x, rect.top()),
            egui::pos2((start_x + 8.0).min(rect.right()), rect.top()),
            egui::pos2(start_x, rect.top() + 8.0),
        ],
        start_color,
        egui::Stroke::NONE,
    ));

    // End Marker Flag & Vertical Line
    let end_color = if scrub.end_marker_secs.is_some() {
        Color32::from_rgb(255, 175, 65)
    } else {
        Color32::from_rgb(145, 115, 75)
    };
    painter.vline(
        end_x,
        rect.top()..=rect.bottom(),
        egui::Stroke::new(2.0, end_color),
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(end_x, rect.top()),
            egui::pos2(end_x, rect.top() + 8.0),
            egui::pos2((end_x - 8.0).max(rect.left()), rect.top()),
        ],
        end_color,
        egui::Stroke::NONE,
    ));

    // Current Playhead Cursor Line
    let cur_x = x_for_sec(scrub.current_position_secs);
    painter.vline(
        cur_x,
        rect.top()..=rect.bottom(),
        egui::Stroke::new(2.0, Color32::WHITE),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_custom_markers_compute_expected_range() {
        let mut scrub = ReplayScrubState {
            duration_secs: 60.0,
            ..Default::default()
        };
        // No markers placed -> defaults to full buffer [0.0, 60.0]
        assert_eq!(scrub.effective_markers(), (0.0, 60.0));

        // Place start at 12.5s
        scrub.current_position_secs = 12.5;
        scrub.place_start_here();
        assert_eq!(scrub.effective_markers(), (12.5, 60.0));

        // Place end at 42.0s
        scrub.current_position_secs = 42.0;
        scrub.place_end_here();
        assert_eq!(scrub.effective_markers(), (12.5, 42.0));

        // Placing start after end clears the old end marker so range remains valid
        scrub.current_position_secs = 50.0;
        scrub.place_start_here();
        assert_eq!(scrub.start_marker_secs, Some(50.0));
        assert_eq!(scrub.end_marker_secs, None);
        assert_eq!(scrub.effective_markers(), (50.0, 60.0));

        scrub.clear_markers();
        assert_eq!(scrub.effective_markers(), (0.0, 60.0));
    }

    #[test]
    fn format_time_tenths_formats_mm_ss_tenths() {
        assert_eq!(format_time_tenths(0.0), "00:00.0");
        assert_eq!(format_time_tenths(5.4), "00:05.4");
        assert_eq!(format_time_tenths(65.8), "01:05.8");
    }

    #[test]
    fn loop_to_start_and_toggle_pause_at_end_marker_restart_from_start_marker() {
        let mut scrub = ReplayScrubState {
            duration_secs: 30.0,
            current_position_secs: 8.0,
            ..Default::default()
        };
        scrub.place_start_here();
        scrub.current_position_secs = 20.0;
        scrub.place_end_here();
        assert_eq!(scrub.effective_markers(), (8.0, 20.0));

        // Reached end marker while playing -> loop_to_start jumps back to 8.0s and keeps playing
        scrub.loop_to_start();
        assert_eq!(scrub.current_position_secs, 8.0);
        assert!(!scrub.is_paused);
        assert!(!scrub.is_eof);

        // Paused at end marker -> toggle_pause restarts from start marker (8.0s)
        scrub.current_position_secs = 20.0;
        scrub.is_paused = true;
        scrub.toggle_pause();
        assert_eq!(scrub.current_position_secs, 8.0);
        assert!(!scrub.is_paused);
    }

    #[test]
    fn steam_deck_mode_and_short_buffer_do_not_open_scrub_window() {
        let ctx = egui::Context::default();
        let mut state = AppState::default();

        // 1. Short buffer (< 5s): pressing F5 or requesting scrub preview silently does nothing
        state.replay.scrub_open_requested = true;
        assert!(!draw_replay_scrub_window(&mut state, &ctx));
        assert!(!state.replay_scrub.is_open());
        assert!(!state.replay_scrub.loading);

        // 2. Steam Deck mode: always disabled even if scrub_open_requested or window_open was set
        state.steam_deck = Some(crate::steam_deck::SteamDeckState::default());
        state.replay.scrub_open_requested = true;
        state.replay_scrub.window_open = true;
        assert!(!draw_replay_scrub_window(&mut state, &ctx));
        assert!(!state.replay.scrub_open_requested);
        assert!(!state.replay_scrub.is_open());
    }

    #[test]
    fn scrub_repaint_callback_never_spawns_phantom_unscaled_viewport() {
        let immediate_viewports = Arc::new(std::sync::Mutex::new(Vec::<egui::ViewportId>::new()));
        let immediate_viewports_clone = Arc::clone(&immediate_viewports);

        egui::Context::set_immediate_viewport_renderer(move |ctx, viewport| {
            let mut input = egui::RawInput {
                viewport_id: viewport.ids.this,
                ..Default::default()
            };
            input.viewports.entry(viewport.ids.this).or_default();
            let output = ctx.run(input, |ui_ctx| {
                (viewport.viewport_ui_cb)(ui_ctx);
            });
            *immediate_viewports_clone.lock().unwrap() =
                output.viewport_output.keys().copied().collect();
        });

        let ctx = egui::Context::default();
        let mut state = AppState::default();

        let snapshot = ScrubSnapshot {
            packets: Vec::new(),
            video: ffmpeg_next::codec::Parameters::new(),
            audio: ffmpeg_next::codec::Parameters::new(),
            directory: String::new(),
            preview_file: Arc::new(tempfile::NamedTempFile::new().unwrap()),
            duration_secs: 10.0,
            width: 1280,
            height: 720,
        };

        state.replay_scrub.start_with_snapshot(
            snapshot,
            0.0,
            Arc::new(AtomicU8::new(0)),
            Arc::new(AtomicU8::new(0)),
            &ctx,
        );

        // Wait briefly for the player thread to emit its event and invoke `request_repaint`.
        if let Some(ref mut player) = state.replay_scrub.player {
            if let Some(handle) = player.player_thread.take() {
                let _ = handle.join();
            }
        }

        let mut root_input = egui::RawInput::default();
        root_input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default();
        let root_output = ctx.run(root_input, |ui_ctx| {
            let _ = draw_replay_scrub_window(&mut state, ui_ctx);
        });

        let expected_scrub_id =
            crate::ui::scaling::viewport_id("replay_scrub_window", &ctx, SCRUB_WINDOW_SIZE);
        let unscaled_phantom_id = egui::ViewportId::from_hash_of("replay_scrub_window");
        assert_ne!(expected_scrub_id, unscaled_phantom_id);

        let seen_in_immediate = immediate_viewports.lock().unwrap().clone();
        assert!(
            !seen_in_immediate.contains(&unscaled_phantom_id),
            "phantom unscaled viewport leaked into immediate viewport_output: {seen_in_immediate:?}"
        );
        assert!(
            !root_output
                .viewport_output
                .contains_key(&unscaled_phantom_id),
            "phantom unscaled viewport leaked into root viewport_output"
        );

        state.replay_scrub.close();
    }

    #[test]
    fn scrub_frame_converter_caches_swscale_context_and_supports_yuv_and_nv12() {
        use ffmpeg_next::format::Pixel;

        let mut converter = ScrubFrameConverter::default();
        let rate = crate::replay::config::Rate::new(60, 1);

        // 1. Repeated 640x360 YUV420P frames reuse the exact same swscale::Context (reinit_count == 1)
        let w = 640u32;
        let h = 360u32;
        let y_len = (w * h) as usize;
        let c_len = ((w / 2) * (h / 2)) as usize;
        let mut yuv_bytes = vec![128u8; y_len + 2 * c_len];
        yuv_bytes[..y_len].fill(180);

        let yuv_frame = RawFrame {
            captured_at: 0,
            rate,
            width: w,
            height: h,
            data: yuv_bytes,
            format: Pixel::YUV420P,
            color_range: ColorRange::Limited,
        };

        for _ in 0..8 {
            let img =
                raw_frame_to_color_image(&yuv_frame, &mut converter, ScalerFilter::FastBilinear)
                    .expect("YUV420P conversion should succeed");
            assert_eq!(img.size, [640, 360]);
        }
        assert_eq!(converter.reinit_count(), 1);

        // 2. Changing ColorRange or ScalerFilter reinitializes once and caches again
        let mut yuv_full = RawFrame {
            captured_at: 16_667,
            rate,
            width: w,
            height: h,
            data: yuv_frame.data.clone(),
            format: Pixel::YUV420P,
            color_range: ColorRange::Full,
        };
        let rgba_raw = converter
            .convert_to_rgba_frame(&yuv_full, ScalerFilter::Bilinear)
            .expect("pooled RGBA RawFrame conversion should succeed");
        assert_eq!(rgba_raw.format, Pixel::RGBA);
        assert_eq!((rgba_raw.width, rgba_raw.height), (640, 360));
        assert_eq!(rgba_raw.data.len(), 640 * 360 * 4);
        assert_eq!(converter.reinit_count(), 2);

        yuv_full.captured_at = 33_333;
        let _ = converter
            .convert_to_rgba_frame(&yuv_full, ScalerFilter::Bilinear)
            .unwrap();
        assert_eq!(converter.reinit_count(), 2);

        // 3. 1920x1080 NV12 downscales to 1280x720 and reuses context across frames
        let hd_w = 1920u32;
        let hd_h = 1080u32;
        let nv12_len = (hd_w * hd_h + hd_w * (hd_h / 2)) as usize;
        let mut nv12_data = checkout_frame_buffer(nv12_len + SWS_SIMD_PADDING_BYTES);
        nv12_data.truncate(nv12_len);
        nv12_data[..(hd_w * hd_h) as usize].fill(200);
        nv12_data[(hd_w * hd_h) as usize..].fill(128);
        let nv12_frame = RawFrame {
            captured_at: 50_000,
            rate,
            width: hd_w,
            height: hd_h,
            data: nv12_data,
            format: Pixel::NV12,
            color_range: ColorRange::Limited,
        };

        let hd_img_1 =
            raw_frame_to_color_image(&nv12_frame, &mut converter, ScalerFilter::FastBilinear)
                .expect("NV12 conversion should succeed");
        let hd_img_2 =
            raw_frame_to_color_image(&nv12_frame, &mut converter, ScalerFilter::FastBilinear)
                .expect("second NV12 conversion should reuse cached context");
        assert_eq!(hd_img_1.size, [1280, 720]);
        assert_eq!(hd_img_2.size, [1280, 720]);
        assert_eq!(converter.reinit_count(), 3);
    }
}
