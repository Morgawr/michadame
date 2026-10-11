//! Studio window (`L` hotkey): quick-access dashboard for Twitch streaming,
//! audio volumes, microphone toggles, replay buffer, and stream diagnostics.

use crate::app::AppState;
use crate::twitch::BroadcastStatus;
use crate::ui::controls::technical_group;
use eframe::egui;
use std::sync::atomic::Ordering;
use std::time::Duration;

const LABEL_COLOR: egui::Color32 = egui::Color32::from_rgb(153, 153, 153);
const MUTED_COLOR: egui::Color32 = egui::Color32::from_rgb(119, 119, 119);
pub const STUDIO_WINDOW_SIZE: [f32; 2] = [540.0, 440.0];

/// Computes the stream health status string and display color based on broadcast state and dropped frames.
pub fn evaluate_stream_health(
    status: &BroadcastStatus,
    video_drops: u64,
    audio_drops: u64,
) -> (&'static str, egui::Color32) {
    match status {
        BroadcastStatus::Live { .. } => {
            let total_drops = video_drops.saturating_add(audio_drops);
            if total_drops == 0 {
                ("Excellent", egui::Color32::from_rgb(80, 220, 120))
            } else if total_drops < 20 {
                ("Good", egui::Color32::from_rgb(168, 224, 99))
            } else if total_drops < 100 {
                ("Fair", egui::Color32::from_rgb(241, 196, 15))
            } else {
                ("Poor", egui::Color32::from_rgb(231, 76, 60))
            }
        }
        BroadcastStatus::Connecting => ("Connecting…", egui::Color32::from_rgb(230, 190, 60)),
        BroadcastStatus::Error(_) => ("Error", egui::Color32::from_rgb(240, 70, 70)),
        BroadcastStatus::Offline => ("Offline", MUTED_COLOR),
    }
}

/// Formats the elapsed broadcast duration into a readable `HH:MM:SS` string.
pub fn format_stream_duration(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    format!("{hours:02}:{mins:02}:{secs:02}")
}

/// Renders a hardware-style studio volume mixer slider with an embedded live audio feedback VU meter.
///
/// Features:
/// - Recessed track with subtle border and dark background.
/// - Live VU meter bar dynamically filling the track behind the fader thumb (green -> orange -> red).
/// - Tactile hardware fader thumb with center indicator stripe and grip texture.
/// - Full click, drag, and mouse-wheel support.
/// - Clean percentage readout without duplicate `%` characters.
/// - Proper disabled styling when unusable (e.g. microphone disabled).
pub fn studio_mixer_slider(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    live_peak: f32,
    enabled: bool,
    width: f32,
) -> egui::Response {
    let height = 22.0;
    let size = egui::vec2(width.max(80.0), height);
    let (rect, mut response) = ui.allocate_exact_size(
        size,
        if enabled {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::hover()
        },
    );

    let thumb_w = 14.0;
    let track_margin = thumb_w / 2.0;
    let usable_w = (rect.width() - thumb_w).max(1.0);

    if enabled {
        if response.clicked() || response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                let fraction = ((pos.x - (rect.min.x + track_margin)) / usable_w).clamp(0.0, 1.0);
                let new_val = *range.start() + fraction * (*range.end() - *range.start());
                let stepped = (new_val * 100.0).round() / 100.0;
                let clamped = stepped.clamp(*range.start(), *range.end());
                if (*value - clamped).abs() > 0.0001 {
                    *value = clamped;
                    response.mark_changed();
                }
            }
        }

        if response.hovered() {
            let scroll_y = ui.input(|i| i.raw_scroll_delta.y);
            if scroll_y != 0.0 {
                let step = 0.02 * (*range.end() - *range.start());
                let delta = if scroll_y > 0.0 { step } else { -step };
                let stepped = ((*value + delta) * 100.0).round() / 100.0;
                let clamped = stepped.clamp(*range.start(), *range.end());
                if (*value - clamped).abs() > 0.0001 {
                    *value = clamped;
                    response.mark_changed();
                }
            }
        }
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();

        // 1. Recessed Track Background
        let track_rect = egui::Rect::from_min_max(
            egui::pos2(rect.min.x, rect.min.y + 2.0),
            egui::pos2(rect.max.x, rect.max.y - 2.0),
        );

        let (track_bg, track_stroke) = if enabled {
            (
                egui::Color32::from_rgb(14, 14, 16),
                egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 48, 54)),
            )
        } else {
            (
                egui::Color32::from_rgb(20, 20, 22),
                egui::Stroke::new(1.0, egui::Color32::from_rgb(32, 34, 38)),
            )
        };
        painter.rect_filled(track_rect, egui::Rounding::same(2.0), track_bg);
        painter.rect_stroke(track_rect, egui::Rounding::same(2.0), track_stroke);

        // 2. Embedded Live VU Meter Bar
        if enabled && live_peak > 0.005 {
            let inner_track = track_rect.shrink(1.0);
            let peak = live_peak.clamp(0.0, 1.0);
            let fill_w = (peak * inner_track.width()).min(inner_track.width());

            let p_green_end = 0.70 * inner_track.width();
            let p_orange_end = 0.88 * inner_track.width();

            // Green segment (safe range: 0.0 .. 0.70)
            let green_w = fill_w.min(p_green_end);
            if green_w > 0.0 {
                let r_green = egui::Rect::from_min_size(
                    inner_track.min,
                    egui::vec2(green_w, inner_track.height()),
                );
                painter.rect_filled(
                    r_green,
                    egui::Rounding::ZERO,
                    egui::Color32::from_rgb(46, 204, 113),
                );
            }

            // Amber/Orange segment (nominal high: 0.70 .. 0.88)
            if fill_w > p_green_end {
                let orange_w = (fill_w - p_green_end).min(p_orange_end - p_green_end);
                let r_orange = egui::Rect::from_min_size(
                    egui::pos2(inner_track.min.x + p_green_end, inner_track.min.y),
                    egui::vec2(orange_w, inner_track.height()),
                );
                painter.rect_filled(
                    r_orange,
                    egui::Rounding::ZERO,
                    egui::Color32::from_rgb(243, 156, 18),
                );
            }

            // Red segment (peak/clip range: 0.88 .. 1.00)
            if fill_w > p_orange_end {
                let red_w = fill_w - p_orange_end;
                let r_red = egui::Rect::from_min_size(
                    egui::pos2(inner_track.min.x + p_orange_end, inner_track.min.y),
                    egui::vec2(red_w, inner_track.height()),
                );
                painter.rect_filled(
                    r_red,
                    egui::Rounding::ZERO,
                    egui::Color32::from_rgb(231, 76, 60),
                );
            }

            // Subtle studio VU graduation marks (at -20dB / 70% and -9dB / 88%)
            painter.line_segment(
                [
                    egui::pos2(inner_track.min.x + p_green_end, inner_track.min.y),
                    egui::pos2(inner_track.min.x + p_green_end, inner_track.max.y),
                ],
                egui::Stroke::new(1.0, egui::Color32::from_rgba_premultiplied(0, 0, 0, 160)),
            );
            painter.line_segment(
                [
                    egui::pos2(inner_track.min.x + p_orange_end, inner_track.min.y),
                    egui::pos2(inner_track.min.x + p_orange_end, inner_track.max.y),
                ],
                egui::Stroke::new(1.0, egui::Color32::from_rgba_premultiplied(0, 0, 0, 160)),
            );
        }

        // 3. Studio Fader Thumb (Hardware console fader cap)
        let norm_val = ((*value - *range.start()) / (*range.end() - *range.start())).clamp(0.0, 1.0);
        let thumb_center_x = rect.min.x + track_margin + norm_val * usable_w;
        let thumb_rect = egui::Rect::from_center_size(
            egui::pos2(thumb_center_x, rect.center().y),
            egui::vec2(thumb_w, rect.height()),
        );

        if enabled {
            let is_active = response.hovered() || response.dragged();
            let cap_color = if is_active {
                egui::Color32::from_rgb(80, 86, 96)
            } else {
                egui::Color32::from_rgb(58, 62, 70)
            };
            let cap_stroke = if is_active {
                egui::Stroke::new(1.5, egui::Color32::from_rgb(215, 222, 235))
            } else {
                egui::Stroke::new(1.0, egui::Color32::from_rgb(135, 142, 155))
            };

            // Fader cap body
            painter.rect_filled(thumb_rect, egui::Rounding::same(2.0), cap_color);
            painter.rect_stroke(thumb_rect, egui::Rounding::same(2.0), cap_stroke);

            // Tactile horizontal ribbing notches on the fader cap
            let rib_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(38, 41, 46));
            painter.line_segment(
                [
                    egui::pos2(thumb_rect.min.x + 2.0, thumb_rect.center().y - 4.0),
                    egui::pos2(thumb_rect.max.x - 2.0, thumb_rect.center().y - 4.0),
                ],
                rib_stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(thumb_rect.min.x + 2.0, thumb_rect.center().y + 4.0),
                    egui::pos2(thumb_rect.max.x - 2.0, thumb_rect.center().y + 4.0),
                ],
                rib_stroke,
            );

            // Crisp white center position indicator stripe
            painter.line_segment(
                [
                    egui::pos2(thumb_center_x, thumb_rect.min.y + 2.0),
                    egui::pos2(thumb_center_x, thumb_rect.max.y - 2.0),
                ],
                egui::Stroke::new(2.0, egui::Color32::WHITE),
            );
        } else {
            // Disabled thumb styling: muted grey
            let cap_color = egui::Color32::from_rgb(36, 36, 38);
            let cap_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(52, 52, 56));
            painter.rect_filled(thumb_rect, egui::Rounding::same(2.0), cap_color);
            painter.rect_stroke(thumb_rect, egui::Rounding::same(2.0), cap_stroke);
            painter.line_segment(
                [
                    egui::pos2(thumb_center_x, thumb_rect.min.y + 3.0),
                    egui::pos2(thumb_center_x, thumb_rect.max.y - 3.0),
                ],
                egui::Stroke::new(1.5, egui::Color32::from_rgb(68, 68, 72)),
            );
        }
    }

    response
}

/// Draws the Studio viewport window when opened.
pub fn draw_studio_window(state: &mut AppState, ctx: &egui::Context) -> bool {
    if !state.ui.studio_window_open {
        return false;
    }

    let mut repaint_requested = false;

    ctx.show_viewport_immediate(
        egui::ViewportId::from_hash_of("studio_window"),
        egui::ViewportBuilder::default()
            .with_title("Michadame Studio")
            .with_inner_size(STUDIO_WINDOW_SIZE)
            .with_min_inner_size(STUDIO_WINDOW_SIZE)
            .with_max_inner_size(STUDIO_WINDOW_SIZE)
            .with_resizable(false),
        |ctx, class| {
            assert!(
                class == egui::ViewportClass::Immediate,
                "This egui backend doesn't support multiple viewports"
            );

            // Hotkeys inside Studio window:
            // Esc or L closes the Studio window.
            if crate::ui::check_toggle_key(ctx, egui::Key::L)
                || crate::ui::check_toggle_key(ctx, egui::Key::Escape)
            {
                state.ui.studio_window_open = false;
                return;
            }

            egui::CentralPanel::default().show(ctx, |ui| {
                repaint_requested |= draw_studio_contents(state, ui);
            });

            if ctx.input(|i| i.viewport().close_requested()) {
                state.ui.studio_window_open = false;
            }
        },
    );

    repaint_requested
}

fn draw_studio_contents(state: &mut AppState, ui: &mut egui::Ui) -> bool {
    let mut changed = false;

    // Smooth 60 FPS repaints to animate live VU meters while Studio is active
    ui.ctx().request_repaint_after(Duration::from_millis(16));

    ui.add_space(2.0);

    // --- Section 1: Twitch Broadcast & Chat ---
    technical_group(ui, "Twitch Broadcast & Chat:", |ui| {
        let status = state.twitch.broadcast_status();
        let is_live = matches!(status, BroadcastStatus::Live { .. });
        let is_connecting = matches!(status, BroadcastStatus::Connecting);
        let active = is_live || is_connecting;

        ui.horizontal(|ui| {
            if active {
                let btn = egui::Button::new(
                    egui::RichText::new("■ Stop Streaming")
                        .monospace()
                        .strong()
                        .size(12.0)
                        .color(egui::Color32::WHITE),
                )
                .fill(egui::Color32::from_rgb(204, 51, 51));
                if ui.add_sized([160.0, 28.0], btn).clicked() {
                    state.twitch.stop_broadcast();
                    changed = true;
                }
            } else {
                let btn = egui::Button::new(
                    egui::RichText::new("● Go Live on Twitch")
                        .monospace()
                        .strong()
                        .size(12.0)
                        .color(egui::Color32::WHITE),
                )
                .fill(egui::Color32::from_rgb(145, 70, 255));
                if ui.add_sized([160.0, 28.0], btn).clicked() {
                    let audio_tap = state.replay.audio.clone();
                    let render_dev = state.replay.config.render_device.clone();
                    if let Err(_) = state.twitch.start_broadcast(audio_tap, &render_dev) {
                        // Error notification is dispatched inside start_broadcast
                    }
                    changed = true;
                }
            }

            ui.add_space(8.0);

            // Channel Indicator
            if state.twitch.config.channel.is_empty() {
                ui.label(
                    egui::RichText::new("Channel: (not configured)")
                        .monospace()
                        .size(11.0)
                        .color(MUTED_COLOR),
                );
            } else {
                ui.label(
                    egui::RichText::new(format!("#{}", state.twitch.config.channel))
                        .monospace()
                        .size(11.0)
                        .strong()
                        .color(egui::Color32::from_rgb(190, 150, 255)),
                );
            }
        });

        ui.add_space(5.0);

        // Row 2: Niconico Style Chat Toggle
        ui.horizontal(|ui| {
            let mut nico_enabled = state.twitch.config.niconico_enabled;
            if ui
                .checkbox(&mut nico_enabled, "Niconico Style Chat")
                .on_hover_text(
                    "Fly live chat messages across the video screen from right to left (niconico style).\nComments are drawn under the CRT effects and are included in replays.",
                )
                .changed()
            {
                state.twitch.config.niconico_enabled = nico_enabled;
                state.twitch.config_dirty = false;
                crate::config::save_config(state);
                changed = true;
            }

            ui.add_space(10.0);

            if nico_enabled {
                ui.label(
                    egui::RichText::new("● Comments Active")
                        .color(egui::Color32::from_rgb(46, 204, 113))
                        .strong()
                        .monospace()
                        .size(10.5),
                );
            } else {
                ui.label(
                    egui::RichText::new("○ Comments Disabled")
                        .color(MUTED_COLOR)
                        .monospace()
                        .size(10.5),
                );
            }
        });
    });

    ui.add_space(5.0);

    // --- Section 2: Twitch Stream Diagnostics ---
    technical_group(ui, "Twitch Stream Diagnostics:", |ui| {
        let status = state.twitch.broadcast_status();
        let (fps, kbps, vdrop, adrop) = if let Some(m) = state.twitch.broadcast_metrics() {
            let fps = m.current_fps.load(Ordering::Relaxed) as f32 / 10.0;
            let kbps = m.current_bitrate_kbps.load(Ordering::Relaxed);
            let vdrop = m.video_dropped.load(Ordering::Relaxed);
            let adrop = m.audio_dropped.load(Ordering::Relaxed);
            (fps, kbps, vdrop, adrop)
        } else {
            (0.0, 0, 0, 0)
        };

        let (health_str, health_color) = evaluate_stream_health(&status, vdrop, adrop);
        let viewer_count = state.twitch.viewer_count();

        match &status {
            BroadcastStatus::Live { started_at } => {
                let duration_str = format_stream_duration(started_at.elapsed());
                let viewers_str = match viewer_count {
                    Some(v) => format!("{v}"),
                    None => "--".to_string(),
                };

                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "● LIVE {duration_str}  |  {fps:.1} FPS  |  "
                        ))
                        .monospace()
                        .strong()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(80, 220, 120)),
                    );
                    ui.label(
                        egui::RichText::new(format!("Health: {health_str}"))
                            .monospace()
                            .strong()
                            .size(11.0)
                            .color(health_color),
                    );
                    ui.label(
                        egui::RichText::new(format!("  |  Viewers: {viewers_str}"))
                            .monospace()
                            .strong()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(80, 220, 120)),
                    );
                });

                ui.add_space(3.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Bitrate: {kbps} kbps  |  Dropped Frames: V {vdrop} / A {adrop}"
                        ))
                        .monospace()
                        .size(10.5)
                        .color(LABEL_COLOR),
                    );
                });
            }
            BroadcastStatus::Connecting => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        egui::RichText::new("Connecting to Twitch ingest…")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(230, 190, 60)),
                    );
                });
            }
            BroadcastStatus::Error(err) => {
                ui.label(
                    egui::RichText::new(format!("Stream Error: {err}"))
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(240, 70, 70)),
                );
            }
            BroadcastStatus::Offline => {
                let viewers_str = match viewer_count {
                    Some(v) => format!("{v}"),
                    None => "0".to_string(),
                };
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "○ Stream Offline  |  FPS: --  |  Health: Offline  |  Viewers: {viewers_str}"
                        ))
                        .monospace()
                        .size(11.0)
                        .color(MUTED_COLOR),
                    );
                });
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Bitrate: 0 kbps  |  Dropped Frames: V 0 / A 0")
                            .monospace()
                            .size(10.5)
                            .color(MUTED_COLOR),
                    );
                });
            }
        }
    });

    ui.add_space(5.0);

    // --- Section 3: Audio Controls with Studio Mixer Sliders ---
    technical_group(ui, "Audio Controls:", |ui| {
        let label_width = 125.0;
        let readout_width = 46.0;

        // Row 1: Master Volume slider with live audio peak feedback
        ui.horizontal(|ui| {
            ui.add_sized(
                [label_width, 22.0],
                egui::Label::new(
                    egui::RichText::new("Master Volume:")
                        .monospace()
                        .size(11.0)
                        .color(LABEL_COLOR),
                ),
            );
            let slider_width = (ui.available_width() - readout_width - 8.0).max(100.0);
            let mut vol = state.hardware.master_volume;
            let peak = state.hardware.live_audio_peak;
            let resp = studio_mixer_slider(ui, &mut vol, 0.0..=1.0, peak, true, slider_width);
            if resp.changed() {
                state.set_master_volume(vol);
                crate::config::save_config(state);
                crate::config::save_global_hardware_config(state);
                changed = true;
            }
            ui.add_sized(
                [readout_width, 22.0],
                egui::Label::new(
                    egui::RichText::new(format!("{:.0}%", vol * 100.0))
                        .monospace()
                        .size(11.0)
                        .strong()
                        .color(egui::Color32::WHITE),
                ),
            );
        });

        ui.add_space(5.0);

        // Row 2: Microphone Toggle & Live Status Indicator (NO emoji!)
        ui.horizontal(|ui| {
            let mut mic_enabled = state.mic.config.enabled;
            if ui
                .checkbox(&mut mic_enabled, "Enable Microphone")
                .on_hover_text("Enables microphone audio input capture and recording into the replay buffer.")
                .changed()
            {
                state.mic.config.enabled = mic_enabled;
                if mic_enabled && state.mic.config.device_name.is_none() {
                    state.mic.config.device_name = state
                        .hardware
                        .audio_sources
                        .first()
                        .map(|(_, name)| name.clone());
                }
                crate::config::save_config(state);
                crate::config::save_global_hardware_config(state);
                state.sync_mic_stream();
                changed = true;
            }

            ui.add_space(10.0);

            let is_stream_active = state.mic.active_stream.is_some();
            if is_stream_active {
                ui.label(
                    egui::RichText::new("● Mic Active")
                        .color(egui::Color32::from_rgb(46, 204, 113))
                        .strong()
                        .monospace()
                        .size(10.5),
                );
            } else if state.mic.config.enabled {
                ui.label(
                    egui::RichText::new("○ Connecting...")
                        .color(egui::Color32::from_rgb(241, 196, 15))
                        .monospace()
                        .size(10.5),
                );
            } else {
                ui.label(
                    egui::RichText::new("○ Mic Disabled")
                        .color(MUTED_COLOR)
                        .monospace()
                        .size(10.5),
                );
            }
        });

        ui.add_space(5.0);

        // Row 3: Microphone Volume slider with live mic peak feedback
        // Kept visible at all times, but unusable and greyed out when microphone is disabled
        ui.horizontal(|ui| {
            ui.add_sized(
                [label_width, 22.0],
                egui::Label::new(
                    egui::RichText::new("Mic Volume:")
                        .monospace()
                        .size(11.0)
                        .color(LABEL_COLOR),
                ),
            );

            let slider_width = (ui.available_width() - readout_width - 8.0).max(100.0);
            let mut mic_vol = state.mic.config.volume;
            let mic_peak = state.hardware.live_mic_peak;
            let mic_enabled = state.mic.config.enabled;
            let resp = studio_mixer_slider(
                ui,
                &mut mic_vol,
                0.0..=1.5,
                mic_peak,
                mic_enabled,
                slider_width,
            );
            if resp.changed() {
                state.mic.config.volume = mic_vol;
                if let Some(stream) = &state.mic.active_stream {
                    if let Ok(mut w) = stream.config.write() {
                        *w = state.mic.config.clone();
                    }
                }
                crate::config::save_global_hardware_config(state);
                changed = true;
            }

            let readout_color = if mic_enabled {
                egui::Color32::WHITE
            } else {
                MUTED_COLOR
            };
            ui.add_sized(
                [readout_width, 22.0],
                egui::Label::new(
                    egui::RichText::new(format!("{:.0}%", mic_vol * 100.0))
                        .monospace()
                        .size(11.0)
                        .strong()
                        .color(readout_color),
                ),
            );
        });
    });

    ui.add_space(5.0);

    // --- Section 4: Replay Buffer ---
    technical_group(ui, "Replay Buffer:", |ui| {
        let mut replay_enabled = state.replay.runtime.is_some();
        let streaming = state.ui.video_window_open;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    streaming || replay_enabled,
                    egui::Checkbox::new(&mut replay_enabled, "Enable Replay Buffer"),
                )
                .on_hover_text("Enables the continuous live replay buffer for clipping.")
                .changed()
            {
                if replay_enabled {
                    if let Err(e) = state.replay.enable() {
                        state.replay.last_status.message = e.to_string();
                        state.error(format!("Failed to enable replay buffer: {e}"));
                    }
                } else {
                    state.replay.disable();
                }
                changed = true;
            }

            ui.add_space(10.0);

            if replay_enabled {
                ui.label(
                    egui::RichText::new("● Buffer Active")
                        .color(egui::Color32::from_rgb(46, 204, 113))
                        .strong()
                        .monospace()
                        .size(10.5),
                );
            } else {
                ui.label(
                    egui::RichText::new("○ Buffer Idle")
                        .color(MUTED_COLOR)
                        .monospace()
                        .size(10.5),
                );
            }
        });

        let status = state.replay.status();
        if !status.message.is_empty() || status.saving {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                if status.saving {
                    ui.label(
                        egui::RichText::new("Saving clip…")
                            .monospace()
                            .size(10.5)
                            .color(egui::Color32::from_rgb(241, 196, 15)),
                    );
                }
                if !status.message.is_empty() {
                    ui.label(
                        egui::RichText::new(&status.message)
                            .monospace()
                            .size(10.5)
                            .color(LABEL_COLOR),
                    );
                }
            });
        }
    });

    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_evaluate_stream_health_offline_and_connecting() {
        let (offline_health, _) = evaluate_stream_health(&BroadcastStatus::Offline, 0, 0);
        assert_eq!(offline_health, "Offline");

        let (connecting_health, _) = evaluate_stream_health(&BroadcastStatus::Connecting, 0, 0);
        assert_eq!(connecting_health, "Connecting…");

        let (err_health, _) = evaluate_stream_health(&BroadcastStatus::Error("err".into()), 0, 0);
        assert_eq!(err_health, "Error");
    }

    #[test]
    fn test_evaluate_stream_health_live_dropped_frames() {
        let live = BroadcastStatus::Live {
            started_at: Instant::now(),
        };

        let (health, _) = evaluate_stream_health(&live, 0, 0);
        assert_eq!(health, "Excellent");

        let (health, _) = evaluate_stream_health(&live, 10, 5);
        assert_eq!(health, "Good");

        let (health, _) = evaluate_stream_health(&live, 50, 20);
        assert_eq!(health, "Fair");

        let (health, _) = evaluate_stream_health(&live, 80, 50);
        assert_eq!(health, "Poor");
    }

    #[test]
    fn test_format_stream_duration() {
        assert_eq!(format_stream_duration(Duration::from_secs(0)), "00:00:00");
        assert_eq!(format_stream_duration(Duration::from_secs(65)), "00:01:05");
        assert_eq!(format_stream_duration(Duration::from_secs(3661)), "01:01:01");
        assert_eq!(format_stream_duration(Duration::from_secs(36000)), "10:00:00");
    }

    #[test]
    fn test_percentage_format_no_double_percent() {
        let formatted = format!("{:.0}%", 0.75 * 100.0);
        assert_eq!(formatted, "75%");
        assert!(!formatted.contains("%%"));

        let formatted_max = format!("{:.0}%", 1.50 * 100.0);
        assert_eq!(formatted_max, "150%");
    }

    #[test]
    fn test_niconico_toggle_state() {
        let mut state = AppState::default();
        assert!(!state.twitch.config.niconico_enabled);
        state.twitch.config.niconico_enabled = true;
        assert!(state.twitch.config.niconico_enabled);
    }
}
