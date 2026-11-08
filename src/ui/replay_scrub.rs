//! Standalone "Replay Buffer Scrub Player" window (opened with F5):
//! previews a snapshot of the replay buffer at the moment F5 was pressed,
//! allows scrubbing and placing Start/End markers, and saves the trimmed clip to disk.

use crate::app::AppState;
use crate::replay::ScrubSnapshot;
use crate::video::player::{PlayerCommand, PlayerEvent, VideoFileConfig};
use crate::video::types::RawFrame;
use crossbeam_channel::Receiver;
use eframe::egui::{self, Color32, RichText};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

/// Designed size (points) of the Replay Buffer Scrub Player window.
const SCRUB_WINDOW_SIZE: egui::Vec2 = egui::vec2(960.0, 680.0);

/// Skip step in seconds for the Skip Back / Skip Forward buttons and Left/Right arrow keys.
const SKIP_STEP_SECS: f64 = 5.0;

pub struct ScrubPlayerHandle {
    pub command_sender: crossbeam_channel::Sender<PlayerCommand>,
    pub frame_rx: Receiver<Arc<RawFrame>>,
    pub event_rx: Receiver<PlayerEvent>,
    pub position_us: Arc<AtomicI64>,
    pub is_paused_shared: Arc<AtomicBool>,
    pub is_eof_shared: Arc<AtomicBool>,
    pub stop_requested: Arc<AtomicBool>,
    pub player_thread: Option<JoinHandle<()>>,
}

impl Drop for ScrubPlayerHandle {
    fn drop(&mut self) {
        self.stop_requested.store(true, Ordering::Release);
        let _ = self.command_sender.send(PlayerCommand::Stop);
        if let Some(handle) = self.player_thread.take() {
            let _ = std::thread::Builder::new()
                .name("scrub-player-cleanup".into())
                .spawn(move || {
                    let _ = handle.join();
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
        self.window_open = true;
        self.loading = true;
        self.current_position_secs = 0.0;
        self.duration_secs = 0.0;
        self.is_paused = false;
        self.is_eof = false;
        self.is_seeking = false;
        self.was_playing_before_scrub = false;
        self.start_marker_secs = None;
        self.end_marker_secs = None;
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
        self.loading = false;
        self.window_open = true;

        let preview_path = snapshot.preview_file.path().to_path_buf();
        self.snapshot = Some(snapshot);

        let (frame_tx, frame_rx) = crossbeam_channel::bounded::<Arc<RawFrame>>(4);
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<PlayerEvent>();
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<PlayerCommand>();

        let position_us = Arc::new(AtomicI64::new(0));
        let is_paused = Arc::new(AtomicBool::new(false));
        let is_eof = Arc::new(AtomicBool::new(false));
        let stop_requested = Arc::new(AtomicBool::new(false));

        let repaint_ctx = ctx.clone();
        let request_repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            repaint_ctx.request_repaint();
        });
        let notify_event = Arc::clone(&request_repaint);

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
                    frame_tx,
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
            frame_rx,
            event_rx,
            position_us,
            is_paused_shared: is_paused,
            is_eof_shared: is_eof,
            stop_requested,
            player_thread: handle,
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
        if let Some(ref player) = self.player {
            let _ = player.command_sender.send(PlayerCommand::Seek(clamped));
        }
    }

    pub fn toggle_pause(&self) {
        if let Some(ref player) = self.player {
            let _ = player.command_sender.send(PlayerCommand::TogglePause);
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

fn raw_frame_to_color_image(frame: &RawFrame) -> Option<egui::ColorImage> {
    use ffmpeg_next::format::Pixel;

    if frame.width == 0 || frame.height == 0 {
        return None;
    }

    if frame.format == Pixel::RGBA {
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

    let mut src = ffmpeg_next::frame::Video::new(frame.format, frame.width, frame.height);
    let planes = src.planes();
    let mut offset = 0usize;
    for plane in 0..planes {
        let row_bytes = match frame.format {
            Pixel::YUYV422 if plane == 0 => frame.width as usize * 2,
            Pixel::YUV422P | Pixel::YUV420P | Pixel::YUVJ422P | Pixel::YUVJ420P => {
                src.plane_width(plane) as usize
            }
            _ => return None,
        };
        let row_count = src.plane_height(plane) as usize;
        let stride = src.stride(plane);
        let plane_dst = src.data_mut(plane);
        for row in 0..row_count {
            let src_end = offset.checked_add(row_bytes)?;
            let src_slice = frame.data.get(offset..src_end)?;
            let dst_start = row.checked_mul(stride)?;
            let dst_end = dst_start.checked_add(row_bytes)?;
            plane_dst
                .get_mut(dst_start..dst_end)?
                .copy_from_slice(src_slice);
            offset = src_end;
        }
    }

    // Scale preview texture to at most 1280x720 for low-overhead UI texture uploads.
    let (dst_w, dst_h) = if frame.width > 1280 || frame.height > 720 {
        let scale = (1280.0 / frame.width as f32).min(720.0 / frame.height as f32);
        let w = ((frame.width as f32 * scale).round() as u32).max(2) & !1;
        let h = ((frame.height as f32 * scale).round() as u32).max(2) & !1;
        (w, h)
    } else {
        (frame.width, frame.height)
    };

    let mut scaler = ffmpeg_next::software::scaling::context::Context::get(
        frame.format,
        frame.width,
        frame.height,
        Pixel::RGBA,
        dst_w,
        dst_h,
        ffmpeg_next::software::scaling::flag::Flags::FAST_BILINEAR,
    )
    .ok()?;

    let mut dst = ffmpeg_next::frame::Video::new(Pixel::RGBA, dst_w, dst_h);
    scaler.run(&src, &mut dst).ok()?;

    let row_bytes = dst_w as usize * 4;
    let stride = dst.stride(0);
    let dst_data = dst.data(0);
    let mut rgba = Vec::with_capacity(row_bytes * dst_h as usize);
    for row in 0..dst_h as usize {
        let start = row * stride;
        let end = start + row_bytes;
        rgba.extend_from_slice(dst_data.get(start..end)?);
    }

    Some(egui::ColorImage::from_rgba_unmultiplied(
        [dst_w as usize, dst_h as usize],
        &rgba,
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScrubAction {
    None,
    Cancel,
    Save,
}

pub fn draw_replay_scrub_window(state: &mut AppState, ctx: &egui::Context) -> bool {
    let mut repaint = false;

    if state.replay.scrub_open_requested {
        state.replay.scrub_open_requested = false;
        if !state.replay_scrub.is_open() {
            match state.replay.request_scrub_preview() {
                Ok(()) => {
                    state.replay_scrub.open_pending();
                    state.sync_local_audio_volume();
                    repaint = true;
                }
                Err(e) => {
                    state.error(format!("Replay scrub failed: {e}"));
                }
            }
        }
    }

    if let Some(snapshot_res) = state.replay.take_scrub_snapshot() {
        match snapshot_res {
            Ok(snapshot) => {
                if state.replay_scrub.is_open() {
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
            Err(err) => {
                state.replay_scrub.close();
                state.sync_local_audio_volume();
                state.error(err);
                repaint = true;
            }
        }
    }

    if !state.replay_scrub.is_open() || !state.ui.scale.settled_once {
        return repaint;
    }

    // Poll scrub player frames and events
    let mut latest_frame = None;
    let mut failed_error = None;
    if let Some(ref mut player) = state.replay_scrub.player {
        while let Ok(frame) = player.frame_rx.try_recv() {
            latest_frame = Some(frame);
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
                    state.replay_scrub.is_eof = true;
                    state.replay_scrub.is_paused = true;
                    repaint = true;
                }
                PlayerEvent::Failed(err) => {
                    failed_error = Some(err);
                }
                PlayerEvent::Stopped => {}
            }
        }
        if !state.replay_scrub.is_seeking {
            state.replay_scrub.current_position_secs =
                (player.position_us.load(Ordering::Relaxed) as f64 / 1_000_000.0)
                    .clamp(0.0, state.replay_scrub.duration_secs.max(0.0));
        }
        state.replay_scrub.is_paused = player.is_paused_shared.load(Ordering::Relaxed);
        state.replay_scrub.is_eof = player.is_eof_shared.load(Ordering::Relaxed);
    }

    if let Some(err) = failed_error {
        state.replay_scrub.close();
        state.sync_local_audio_volume();
        state.error(format!("Replay scrub preview error: {err}"));
        return true;
    }

    if let Some(frame) = latest_frame {
        if let Some(color_image) = raw_frame_to_color_image(&frame) {
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

    if state.replay_scrub.is_open() && (state.replay_scrub.loading || !state.replay_scrub.is_paused)
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
        }
    }
    if response.drag_stopped() {
        scrub.is_seeking = false;
        if scrub.was_playing_before_scrub {
            if let Some(ref player) = scrub.player {
                let _ = player.command_sender.send(PlayerCommand::Play);
            }
        }
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
}
