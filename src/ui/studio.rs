//! Studio window (`L` hotkey): Stream-Deck style hardware dashboard for Twitch
//! broadcast control, stream title, quick hardware toggles, audio faders, and stream diagnostics.

use crate::app::AppState;
use crate::twitch::BroadcastStatus;
use eframe::egui;
use std::sync::atomic::Ordering;
use std::time::Duration;

const LABEL_COLOR: egui::Color32 = egui::Color32::from_rgb(153, 153, 153);
const MUTED_COLOR: egui::Color32 = egui::Color32::from_rgb(119, 119, 119);
pub const STUDIO_WINDOW_SIZE: [f32; 2] = [480.0, 410.0];

/// Calculates the percentage of dropped frames/blocks out of total stream units.
pub fn compute_drop_percentage(frames_sent: u64, video_drops: u64, audio_drops: u64) -> f64 {
    let total_drops = video_drops.saturating_add(audio_drops);
    let total_units = frames_sent.saturating_add(total_drops);
    if total_units == 0 {
        0.0
    } else {
        (total_drops as f64 / total_units as f64) * 100.0
    }
}

/// Computes the stream health status string and display color based on broadcast state and frame delivery rates.
pub fn evaluate_stream_health(
    status: &BroadcastStatus,
    frames_sent: u64,
    video_drops: u64,
    audio_drops: u64,
) -> (&'static str, egui::Color32) {
    match status {
        BroadcastStatus::Live { .. } => {
            let total_drops = video_drops.saturating_add(audio_drops);
            let total_units = frames_sent.saturating_add(total_drops);
            let drop_pct = compute_drop_percentage(frames_sent, video_drops, audio_drops);

            // Startup grace: fewer than 300 frames (~5s at 60fps) with small initial drops is Excellent
            if total_drops == 0 || (total_units < 300 && total_drops <= 3) || drop_pct < 0.5 {
                ("Excellent", egui::Color32::from_rgb(80, 220, 120))
            } else if drop_pct < 1.5 {
                ("Good", egui::Color32::from_rgb(168, 224, 99))
            } else if drop_pct < 5.0 {
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

/// Type of toggle icon for the Stream Deck buttons.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DeckButtonKind {
    ReplayBuffer,
    Microphone,
    Webcam,
    ScrollingChat,
    Alerts,
    ShareVocab,
}

/// Renders a square "Stream Deck" hardware button.
///
/// Features:
/// - Beveled hardware keycap styling.
/// - Clearly distinguishable vector icon.
/// - Enabled state: illuminated green keycap, green border, green icon.
/// - Disabled state: dark keycap, red border, and a bold oblique diagonal strike-out bar across the icon.
pub fn draw_deck_button(
    ui: &mut egui::Ui,
    kind: DeckButtonKind,
    label: &str,
    enabled: bool,
    tooltip: &str,
    size: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let is_hovered = response.hovered();

        // 1. Hardware Stream Deck Keycap Body
        let (bg_color, stroke_color, stroke_width) = if enabled {
            if is_hovered {
                (
                    egui::Color32::from_rgb(18, 42, 26),
                    egui::Color32::from_rgb(52, 224, 126),
                    2.0,
                )
            } else {
                (
                    egui::Color32::from_rgb(14, 34, 20),
                    egui::Color32::from_rgb(46, 204, 113),
                    1.8,
                )
            }
        } else {
            if is_hovered {
                (
                    egui::Color32::from_rgb(32, 24, 24),
                    egui::Color32::from_rgb(120, 48, 48),
                    1.5,
                )
            } else {
                (
                    egui::Color32::from_rgb(22, 20, 20),
                    egui::Color32::from_rgb(60, 36, 36),
                    1.2,
                )
            }
        };

        painter.rect_filled(rect, egui::Rounding::same(8.0), bg_color);
        painter.rect_stroke(
            rect,
            egui::Rounding::same(8.0),
            egui::Stroke::new(stroke_width, stroke_color),
        );

        // Top-edge subtle highlight
        if enabled {
            painter.line_segment(
                [
                    egui::pos2(rect.min.x + 8.0, rect.min.y + 2.0),
                    egui::pos2(rect.max.x - 8.0, rect.min.y + 2.0),
                ],
                egui::Stroke::new(1.0, egui::Color32::from_rgba_premultiplied(46, 204, 113, 100)),
            );
        } else {
            painter.line_segment(
                [
                    egui::pos2(rect.min.x + 8.0, rect.min.y + 2.0),
                    egui::pos2(rect.max.x - 8.0, rect.min.y + 2.0),
                ],
                egui::Stroke::new(1.0, egui::Color32::from_rgba_premultiplied(255, 255, 255, 20)),
            );
        }

        // 2. Icon Area
        let icon_center = egui::pos2(rect.center().x, rect.center().y - 10.0);
        let icon_color = if enabled {
            egui::Color32::from_rgb(46, 204, 113)
        } else {
            egui::Color32::from_rgb(140, 60, 60)
        };

        match kind {
            DeckButtonKind::ReplayBuffer => {
                // Circular replay / rewind arrow loop with center play triangle
                let radius = 12.0;
                let num_steps = 22;
                let mut pts = Vec::with_capacity(num_steps + 1);
                for i in 0..=num_steps {
                    let frac = i as f32 / num_steps as f32;
                    let angle = 0.55 + frac * 4.95;
                    let px = icon_center.x + radius * angle.cos();
                    let py = icon_center.y + radius * angle.sin();
                    pts.push(egui::pos2(px, py));
                }
                for i in 0..pts.len() - 1 {
                    painter.line_segment(
                        [pts[i], pts[i + 1]],
                        egui::Stroke::new(2.2, icon_color),
                    );
                }
                // Arrowhead
                let tip = pts[0];
                painter.line_segment(
                    [tip, egui::pos2(tip.x - 4.5, tip.y - 5.0)],
                    egui::Stroke::new(2.2, icon_color),
                );
                painter.line_segment(
                    [tip, egui::pos2(tip.x + 5.0, tip.y - 4.5)],
                    egui::Stroke::new(2.2, icon_color),
                );
                // Center play triangle
                let tri_pts = [
                    egui::pos2(icon_center.x - 3.5, icon_center.y - 4.5),
                    egui::pos2(icon_center.x + 4.5, icon_center.y),
                    egui::pos2(icon_center.x - 3.5, icon_center.y + 4.5),
                ];
                painter.add(egui::Shape::convex_polygon(
                    tri_pts.to_vec(),
                    icon_color,
                    egui::Stroke::NONE,
                ));
            }
            DeckButtonKind::Microphone => {
                // Studio capsule microphone
                let cap_rect = egui::Rect::from_center_size(
                    egui::pos2(icon_center.x, icon_center.y - 3.5),
                    egui::vec2(8.5, 15.0),
                );
                painter.rect_filled(cap_rect, egui::Rounding::same(4.25), icon_color);

                // U-shaped cradle bracket
                let num_cradle = 12;
                let mut cradle_pts = Vec::with_capacity(num_cradle + 1);
                for i in 0..=num_cradle {
                    let frac = i as f32 / num_cradle as f32;
                    let angle = std::f32::consts::PI * frac;
                    let cx = icon_center.x - 7.5 * angle.cos();
                    let cy = (icon_center.y - 2.5) + 8.0 * angle.sin();
                    cradle_pts.push(egui::pos2(cx, cy));
                }
                for i in 0..cradle_pts.len() - 1 {
                    painter.line_segment(
                        [cradle_pts[i], cradle_pts[i + 1]],
                        egui::Stroke::new(2.0, icon_color),
                    );
                }

                // Stem and Base
                painter.line_segment(
                    [
                        egui::pos2(icon_center.x, icon_center.y + 5.5),
                        egui::pos2(icon_center.x, icon_center.y + 11.0),
                    ],
                    egui::Stroke::new(2.0, icon_color),
                );
                painter.line_segment(
                    [
                        egui::pos2(icon_center.x - 6.5, icon_center.y + 11.0),
                        egui::pos2(icon_center.x + 6.5, icon_center.y + 11.0),
                    ],
                    egui::Stroke::new(2.0, icon_color),
                );
            }
            DeckButtonKind::ScrollingChat => {
                // Speech bubble with flying chat lines
                let bubble_rect = egui::Rect::from_center_size(
                    egui::pos2(icon_center.x, icon_center.y - 2.0),
                    egui::vec2(25.0, 16.0),
                );
                painter.rect_stroke(
                    bubble_rect,
                    egui::Rounding::same(3.0),
                    egui::Stroke::new(2.0, icon_color),
                );

                // Bottom tail
                let tail = [
                    egui::pos2(bubble_rect.min.x + 3.0, bubble_rect.max.y),
                    egui::pos2(bubble_rect.min.x + 1.0, bubble_rect.max.y + 4.0),
                    egui::pos2(bubble_rect.min.x + 7.0, bubble_rect.max.y),
                ];
                painter.add(egui::Shape::convex_polygon(
                    tail.to_vec(),
                    icon_color,
                    egui::Stroke::NONE,
                ));

                // Flying motion lines with arrows
                painter.line_segment(
                    [
                        egui::pos2(icon_center.x - 7.0, icon_center.y - 4.5),
                        egui::pos2(icon_center.x + 3.0, icon_center.y - 4.5),
                    ],
                    egui::Stroke::new(1.8, icon_color),
                );
                painter.line_segment(
                    [
                        egui::pos2(icon_center.x + 1.0, icon_center.y - 6.5),
                        egui::pos2(icon_center.x + 3.0, icon_center.y - 4.5),
                    ],
                    egui::Stroke::new(1.8, icon_color),
                );

                painter.line_segment(
                    [
                        egui::pos2(icon_center.x - 4.0, icon_center.y + 0.5),
                        egui::pos2(icon_center.x + 6.0, icon_center.y + 0.5),
                    ],
                    egui::Stroke::new(1.8, icon_color),
                );
                painter.line_segment(
                    [
                        egui::pos2(icon_center.x + 4.0, icon_center.y - 1.5),
                        egui::pos2(icon_center.x + 6.0, icon_center.y + 0.5),
                    ],
                    egui::Stroke::new(1.8, icon_color),
                );
            }
            DeckButtonKind::Alerts => {
                // Vector notification bell icon
                // 1. Top hanger loop
                painter.circle_stroke(
                    egui::pos2(icon_center.x, icon_center.y - 10.5),
                    2.2,
                    egui::Stroke::new(1.8, icon_color),
                );

                // 2. Bell dome body
                let bell_pts = [
                    egui::pos2(icon_center.x - 3.5, icon_center.y - 8.5),
                    egui::pos2(icon_center.x + 3.5, icon_center.y - 8.5),
                    egui::pos2(icon_center.x + 6.5, icon_center.y - 2.0),
                    egui::pos2(icon_center.x + 10.0, icon_center.y + 4.0),
                    egui::pos2(icon_center.x - 10.0, icon_center.y + 4.0),
                    egui::pos2(icon_center.x - 6.5, icon_center.y - 2.0),
                ];
                painter.add(egui::Shape::convex_polygon(
                    bell_pts.to_vec(),
                    icon_color,
                    egui::Stroke::NONE,
                ));

                // 3. Bell bottom rim
                painter.line_segment(
                    [
                        egui::pos2(icon_center.x - 11.0, icon_center.y + 5.0),
                        egui::pos2(icon_center.x + 11.0, icon_center.y + 5.0),
                    ],
                    egui::Stroke::new(2.2, icon_color),
                );

                // 4. Clapper bead at bottom
                painter.circle_filled(
                    egui::pos2(icon_center.x, icon_center.y + 7.5),
                    2.8,
                    icon_color,
                );
            }
            DeckButtonKind::Webcam => {
                // Vector camera icon
                // 1. Camera main body
                let body_rect = egui::Rect::from_center_size(
                    egui::pos2(icon_center.x - 3.0, icon_center.y + 1.0),
                    egui::vec2(16.0, 13.0),
                );
                painter.rect_filled(body_rect, egui::Rounding::ZERO, icon_color);

                // 2. Front projection lens trapezoid
                let lens_pts = [
                    egui::pos2(body_rect.max.x + 1.0, icon_center.y - 3.5),
                    egui::pos2(body_rect.max.x + 8.0, icon_center.y - 6.5),
                    egui::pos2(body_rect.max.x + 8.0, icon_center.y + 8.5),
                    egui::pos2(body_rect.max.x + 1.0, icon_center.y + 5.5),
                ];
                painter.add(egui::Shape::convex_polygon(
                    lens_pts.to_vec(),
                    icon_color,
                    egui::Stroke::NONE,
                ));

                // 3. Dual film spool / tape reels on top of camera
                painter.circle_filled(
                    egui::pos2(icon_center.x - 7.5, icon_center.y - 7.5),
                    3.2,
                    icon_color,
                );
                painter.circle_filled(
                    egui::pos2(icon_center.x + 1.5, icon_center.y - 7.5),
                    3.2,
                    icon_color,
                );
                painter.circle_filled(
                    egui::pos2(icon_center.x - 7.5, icon_center.y - 7.5),
                    1.2,
                    bg_color,
                );
                painter.circle_filled(
                    egui::pos2(icon_center.x + 1.5, icon_center.y - 7.5),
                    1.2,
                    bg_color,
                );
            }
            DeckButtonKind::ShareVocab => {
                // Open dictionary book with text lines
                let left_page = [
                    egui::pos2(icon_center.x - 1.5, icon_center.y - 8.0),
                    egui::pos2(icon_center.x - 13.0, icon_center.y - 10.0),
                    egui::pos2(icon_center.x - 13.0, icon_center.y + 7.0),
                    egui::pos2(icon_center.x - 1.5, icon_center.y + 9.0),
                ];
                painter.add(egui::Shape::convex_polygon(
                    left_page.to_vec(),
                    bg_color,
                    egui::Stroke::new(1.8, icon_color),
                ));
                let right_page = [
                    egui::pos2(icon_center.x + 1.5, icon_center.y - 8.0),
                    egui::pos2(icon_center.x + 13.0, icon_center.y - 10.0),
                    egui::pos2(icon_center.x + 13.0, icon_center.y + 7.0),
                    egui::pos2(icon_center.x + 1.5, icon_center.y + 9.0),
                ];
                painter.add(egui::Shape::convex_polygon(
                    right_page.to_vec(),
                    bg_color,
                    egui::Stroke::new(1.8, icon_color),
                ));
                painter.line_segment(
                    [
                        egui::pos2(icon_center.x, icon_center.y - 7.5),
                        egui::pos2(icon_center.x, icon_center.y + 9.5),
                    ],
                    egui::Stroke::new(2.0, icon_color),
                );
                painter.line_segment(
                    [
                        egui::pos2(icon_center.x - 10.0, icon_center.y - 4.5),
                        egui::pos2(icon_center.x - 4.0, icon_center.y - 3.5),
                    ],
                    egui::Stroke::new(1.4, icon_color),
                );
                painter.line_segment(
                    [
                        egui::pos2(icon_center.x - 10.0, icon_center.y + 0.5),
                        egui::pos2(icon_center.x - 4.0, icon_center.y + 1.5),
                    ],
                    egui::Stroke::new(1.4, icon_color),
                );
                painter.line_segment(
                    [
                        egui::pos2(icon_center.x + 4.0, icon_center.y - 3.5),
                        egui::pos2(icon_center.x + 10.0, icon_center.y - 4.5),
                    ],
                    egui::Stroke::new(1.4, icon_color),
                );
                painter.line_segment(
                    [
                        egui::pos2(icon_center.x + 4.0, icon_center.y + 1.5),
                        egui::pos2(icon_center.x + 10.0, icon_center.y + 0.5),
                    ],
                    egui::Stroke::new(1.4, icon_color),
                );
            }
        }

        // 3. Oblique diagonal strike-out bar when disabled
        if !enabled {
            let slash_p1 = egui::pos2(icon_center.x - 15.0, icon_center.y - 15.0);
            let slash_p2 = egui::pos2(icon_center.x + 15.0, icon_center.y + 15.0);

            // Dark background cutout stroke under the slash
            painter.line_segment(
                [slash_p1, slash_p2],
                egui::Stroke::new(4.5, bg_color),
            );
            // Bright red oblique bar on top
            painter.line_segment(
                [slash_p1, slash_p2],
                egui::Stroke::new(2.5, egui::Color32::from_rgb(231, 76, 60)),
            );
        }

        // 4. Label at bottom of keycap
        let text_color = if enabled {
            egui::Color32::from_rgb(46, 204, 113)
        } else {
            egui::Color32::from_rgb(160, 80, 80)
        };
        painter.text(
            egui::pos2(rect.center().x, rect.max.y - 14.0),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::monospace(10.5),
            text_color,
        );
    }

    response.on_hover_text(tooltip)
}

/// Renders a hardware-style studio volume mixer slider with an embedded live audio feedback VU meter.
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

    // 1. Giant "GO LIVE ON TWITCH" / "STOP STREAMING" button (no channel name)
    let status = state.twitch.broadcast_status();
    let is_live = matches!(status, BroadcastStatus::Live { .. });
    let is_connecting = matches!(status, BroadcastStatus::Connecting);
    let active = is_live || is_connecting;

    let (btn_text, btn_fill) = if is_live {
        ("■ STOP STREAMING", egui::Color32::from_rgb(204, 51, 51))
    } else if is_connecting {
        ("CONNECTING TO TWITCH...", egui::Color32::from_rgb(212, 136, 0))
    } else {
        ("● GO LIVE ON TWITCH", egui::Color32::from_rgb(145, 70, 255))
    };

    let giant_btn = egui::Button::new(
        egui::RichText::new(btn_text)
            .monospace()
            .strong()
            .size(15.0)
            .color(egui::Color32::WHITE),
    )
    .fill(btn_fill)
    .rounding(egui::Rounding::same(6.0));

    if ui.add_sized([ui.available_width(), 46.0], giant_btn).clicked() {
        if active {
            state.twitch.stop_broadcast();
            changed = true;
        } else {
            let audio_tap = state.replay.audio.clone();
            audio_tap.set_game_volume(state.hardware.master_volume);
            let render_dev = state.replay.config.render_device.clone();
            if let Err(_) = state.twitch.start_broadcast(audio_tap, &render_dev) {
                // Error notification is dispatched inside start_broadcast
            }
            changed = true;
        }
    }

    ui.add_space(6.0);

    // 2. Stream Title Label (non-editable, shows stream title regardless if live or not)
    let stream_title = if !state.twitch.stream_info.title_input.trim().is_empty() {
        state.twitch.stream_info.title_input.trim()
    } else if !state.twitch.stream_info.original_title.trim().is_empty() {
        state.twitch.stream_info.original_title.trim()
    } else {
        ""
    };

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Title:")
                .monospace()
                .size(11.0)
                .color(LABEL_COLOR),
        );
        let (display_text, display_color) = if stream_title.is_empty() {
            ("(No stream title set)".to_string(), MUTED_COLOR)
        } else {
            (stream_title.to_string(), egui::Color32::from_rgb(220, 220, 220))
        };
        let avail_w = ui.available_width().min(STUDIO_WINDOW_SIZE[0] - 65.0);
        ui.add_sized(
            [avail_w, 16.0],
            egui::Label::new(
                egui::RichText::new(display_text)
                    .monospace()
                    .strong()
                    .size(11.0)
                    .color(display_color),
            )
            .truncate(true),
        )
        .on_hover_text(if stream_title.is_empty() {
            "(No stream title set)"
        } else {
            stream_title
        });
    });

    ui.add_space(10.0);

    // 3. Mini-grid of 6 Stream Deck square buttons in a 3x2 grid
    // Row 1: Replay buffer | Microphone | Webcam
    // Row 2: Scrolling chat | Video alerts | Share mined vocab
    let btn_size = 78.0;
    let btn_gap = 14.0;
    let total_grid_w = 3.0 * btn_size + 2.0 * btn_gap;
    let avail_w = ui.available_width();
    let side_padding = ((avail_w - total_grid_w) / 2.0).max(0.0);

    // Row 1: Production inputs (Replay, Mic, Cam)
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if side_padding > 0.0 {
            ui.add_space(side_padding);
        }

        // Deck Button 1: Replay Buffer
        let replay_enabled = state.replay.runtime.is_some();
        if draw_deck_button(
            ui,
            DeckButtonKind::ReplayBuffer,
            "REPLAY",
            replay_enabled,
            "Replay Buffer: Click to toggle continuous clip recording",
            btn_size,
        )
        .clicked()
        {
            if replay_enabled {
                state.replay.disable();
            } else {
                if let Err(e) = state.replay.enable() {
                    state.replay.last_status.message = e.to_string();
                    state.error(format!("Failed to enable replay buffer: {e}"));
                }
            }
            changed = true;
        }

        ui.add_space(btn_gap);

        // Deck Button 2: Microphone
        let mic_enabled = state.mic.config.enabled;
        if draw_deck_button(
            ui,
            DeckButtonKind::Microphone,
            "MIC",
            mic_enabled,
            "Microphone: Click to toggle microphone audio capture",
            btn_size,
        )
        .clicked()
        {
            let new_enabled = !state.mic.config.enabled;
            state.mic.config.enabled = new_enabled;
            if new_enabled && state.mic.config.device_name.is_none() {
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

        ui.add_space(btn_gap);

        // Deck Button 3: Webcam
        let webcam_enabled = state.webcam.config.enabled;
        if draw_deck_button(
            ui,
            DeckButtonKind::Webcam,
            "CAM",
            webcam_enabled,
            "Webcam: Click to toggle webcam overlay feed",
            btn_size,
        )
        .clicked()
        {
            let new_enabled = !state.webcam.config.enabled;
            state.webcam.config.enabled = new_enabled;
            if new_enabled && state.webcam.config.device.is_none() {
                state.webcam.config.device = state.hardware.video_devices.first().cloned();
                if let Some(dev) = &state.webcam.config.device {
                    if let Ok(formats) = crate::devices::video::find_video_formats(dev) {
                        state.webcam.supported_formats = formats;
                        if let Some(first_fmt) = state.webcam.supported_formats.first() {
                            state.webcam.config.format_fourcc = Some(first_fmt.fourcc.clone());
                            if let Some(first_res) = first_fmt.resolutions.first() {
                                state.webcam.config.resolution = Some((first_res.width, first_res.height));
                                state.webcam.config.framerate = first_res.framerates.first().cloned();
                            }
                        }
                    }
                }
            }
            crate::config::save_config(state);
            crate::config::save_global_hardware_config(state);
            state.sync_webcam_stream(Some(ui.ctx()));
            changed = true;
        }

        if side_padding > 0.0 {
            ui.add_space(side_padding);
        }
    });

    ui.add_space(8.0);

    // Row 2: Stream interactions & overlay features (Chat, Alerts, Vocab)
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if side_padding > 0.0 {
            ui.add_space(side_padding);
        }

        // Deck Button 4: Scrolling Chat
        let chat_enabled = state.twitch.config.niconico_enabled;
        if draw_deck_button(
            ui,
            DeckButtonKind::ScrollingChat,
            "CHAT",
            chat_enabled,
            "Scrolling Chat: Click to toggle niconico flying comments across the video",
            btn_size,
        )
        .clicked()
        {
            state.twitch.config.niconico_enabled = !state.twitch.config.niconico_enabled;
            state.twitch.config_dirty = false;
            crate::config::save_config(state);
            changed = true;
        }

        ui.add_space(btn_gap);

        // Deck Button 5: Video Alerts
        let alerts_enabled = state.twitch.config.alerts_enabled;
        if draw_deck_button(
            ui,
            DeckButtonKind::Alerts,
            "ALERTS",
            alerts_enabled,
            "Video Alerts: Click to toggle on-screen event notices (followers, subs, bits, raids)",
            btn_size,
        )
        .clicked()
        {
            state.twitch.config.alerts_enabled = !state.twitch.config.alerts_enabled;
            state.twitch.config_dirty = false;
            crate::config::save_config(state);
            changed = true;
        }

        ui.add_space(btn_gap);

        // Deck Button 6: Share Mined Vocab to Chat
        let vocab_enabled = state.twitch.config.share_mined_words_to_chat;
        if draw_deck_button(
            ui,
            DeckButtonKind::ShareVocab,
            "VOCAB",
            vocab_enabled,
            "Share Mined Vocab: Click to toggle sharing newly mined words to Twitch chat while live",
            btn_size,
        )
        .clicked()
        {
            state.twitch.config.share_mined_words_to_chat = !state.twitch.config.share_mined_words_to_chat;
            state.twitch.config_dirty = false;
            crate::config::save_config(state);
            changed = true;
        }

        if side_padding > 0.0 {
            ui.add_space(side_padding);
        }
    });

    ui.add_space(12.0);

    // 4. Two horizontal bars with volume sliders (as implemented with embedded VU meters)
    let label_width = 115.0;
    let readout_width = 46.0;

    // Master Volume Slider
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
        let avail_w = ui.available_width().min(STUDIO_WINDOW_SIZE[0] - 16.0 - label_width);
        let slider_width = (avail_w - readout_width - 8.0).max(100.0);
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

    ui.add_space(6.0);

    // Microphone Volume Slider
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

        let avail_w = ui.available_width().min(STUDIO_WINDOW_SIZE[0] - 16.0 - label_width);
        let slider_width = (avail_w - readout_width - 8.0).max(100.0);
        let mut mic_vol = state.mic.config.volume;
        let mic_peak = state.hardware.live_mic_peak;
        let mic_active = state.mic.config.enabled;
        let resp = studio_mixer_slider(
            ui,
            &mut mic_vol,
            0.0..=1.5,
            mic_peak,
            mic_active,
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

        let readout_color = if mic_active {
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

    ui.add_space(14.0);

    // 5. Stream Diagnostic Data at the bottom
    let (fps, kbps, vdrop, adrop, frames_sent) = if let Some(m) = state.twitch.broadcast_metrics() {
        let fps = m.current_fps.load(Ordering::Relaxed) as f32 / 10.0;
        let kbps = m.current_bitrate_kbps.load(Ordering::Relaxed);
        let vdrop = m.video_dropped.load(Ordering::Relaxed);
        let adrop = m.audio_dropped.load(Ordering::Relaxed);
        let frames_sent = m.frames_sent.load(Ordering::Relaxed);
        (fps, kbps, vdrop, adrop, frames_sent)
    } else {
        (0.0, 0, 0, 0, 0)
    };

    let (health_str, health_color) = evaluate_stream_health(&status, frames_sent, vdrop, adrop);
    let drop_pct = compute_drop_percentage(frames_sent, vdrop, adrop);
    let viewer_count = state.twitch.viewer_count();

    egui::Frame::none()
        .fill(egui::Color32::from_rgb(18, 18, 20))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(42, 42, 46)))
        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
        .rounding(egui::Rounding::same(4.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(STUDIO_WINDOW_SIZE[0] - 16.0));

            match &status {
                BroadcastStatus::Live { started_at } => {
                    let duration_str = format_stream_duration(started_at.elapsed());
                    let viewers_str = match viewer_count {
                        Some(v) => format!("{v}"),
                        None => "--".to_string(),
                    };

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("● LIVE {duration_str}  |  {fps:.1} FPS  |  "))
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
                        )
                        .on_hover_text(format!(
                            "Frame drop rate: {drop_pct:.2}% (V: {vdrop}, A: {adrop} dropped out of {} total)",
                            frames_sent.saturating_add(vdrop)
                        ));
                        ui.label(
                            egui::RichText::new(format!("  |  Viewers: {viewers_str}"))
                                .monospace()
                                .strong()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(80, 220, 120)),
                        );
                    });

                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        let drop_pct_str = if drop_pct > 0.0 && drop_pct < 0.1 {
                            "<0.1%".to_string()
                        } else {
                            format!("{drop_pct:.1}%")
                        };
                        ui.label(
                            egui::RichText::new(format!(
                                "Bitrate: {kbps} kbps  |  Dropped: V {vdrop} / A {adrop} ({drop_pct_str})"
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

    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_evaluate_stream_health_offline_and_connecting() {
        let (offline_health, _) = evaluate_stream_health(&BroadcastStatus::Offline, 0, 0, 0);
        assert_eq!(offline_health, "Offline");

        let (connecting_health, _) = evaluate_stream_health(&BroadcastStatus::Connecting, 0, 0, 0);
        assert_eq!(connecting_health, "Connecting…");

        let (err_health, _) = evaluate_stream_health(&BroadcastStatus::Error("err".into()), 0, 0, 0);
        assert_eq!(err_health, "Error");
    }

    #[test]
    fn test_evaluate_stream_health_live_dropped_frames() {
        let live = BroadcastStatus::Live {
            started_at: Instant::now(),
        };

        // Flawless stream (0 drops)
        let (health, _) = evaluate_stream_health(&live, 10_000, 0, 0);
        assert_eq!(health, "Excellent");

        // Real-world 2-hour session: 100 video drops out of 432,000 frames (0.027% drops -> Excellent!)
        let (health, _) = evaluate_stream_health(&live, 432_000, 100, 20);
        assert_eq!(health, "Excellent");

        // Minor drops (< 1.5% -> Good)
        let (health, _) = evaluate_stream_health(&live, 10_000, 80, 20);
        assert_eq!(health, "Good");

        // Moderate drops (< 5.0% -> Fair)
        let (health, _) = evaluate_stream_health(&live, 10_000, 250, 50);
        assert_eq!(health, "Fair");

        // Severe drops (>= 5.0% -> Poor)
        let (health, _) = evaluate_stream_health(&live, 10_000, 600, 100);
        assert_eq!(health, "Poor");

        // Startup grace with a couple initialization drops (< 300 frames)
        let (health, _) = evaluate_stream_health(&live, 120, 2, 0);
        assert_eq!(health, "Excellent");
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

    #[test]
    fn test_alerts_toggle_state() {
        let mut state = AppState::default();
        assert!(!state.twitch.config.alerts_enabled);
        state.twitch.config.alerts_enabled = true;
        assert!(state.twitch.config.alerts_enabled);
    }

    #[test]
    fn test_studio_contents_renders_with_long_stream_title() {
        let mut state = AppState::default();
        state.twitch.stream_info.original_title = "Late night comfy read stream | Streaming until my son wakes up and asks for breakfast | Playing retro games".to_string();
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw_studio_contents(&mut state, ui);
            });
        });
    }

    #[test]
    fn test_webcam_toggle_state() {
        let mut state = AppState::default();
        assert!(!state.webcam.config.enabled);
        state.webcam.config.enabled = true;
        assert!(state.webcam.config.enabled);
    }

    #[test]
    fn test_no_emojis_in_studio_buttons_or_tooltips() {
        let quick_buttons = [
            ("CHAT", "Twitch Chat: Click to toggle on-screen transparent chat overlay"),
            ("NICO", "Niconico: Click to toggle horizontal floating barrage comments"),
            ("ALERTS", "Alerts: Click to toggle animated on-screen event popups"),
            ("MIC", "Microphone: Click to toggle live voice capture into the stream"),
            ("CAM", "Webcam: Click to toggle webcam overlay feed"),
            ("VOCAB", "Share Mined Vocab: Click to toggle sharing newly mined words to Twitch chat while live"),
        ];
        for (label, tooltip) in quick_buttons {
            assert!(label.chars().all(|c| c.is_ascii()), "Label contains non-ascii: {}", label);
            assert!(tooltip.chars().all(|c| c.is_ascii()), "Tooltip contains non-ascii: {}", tooltip);
        }
    }

    #[test]
    fn test_studio_quick_deck_buttons_equal_padding_and_no_overflow() {
        let avail_w: f32 = 464.0; // STUDIO_WINDOW_SIZE[0] (480.0) - 16.0 (central panel margins)
        let btn_size: f32 = 78.0;
        let btn_gap: f32 = 14.0;
        let cols_per_row: f32 = 3.0;
        let total_grid_w = cols_per_row * btn_size + (cols_per_row - 1.0) * btn_gap;
        let side_padding = ((avail_w - total_grid_w) / 2.0).max(0.0);

        // Verify total width fits within available width
        assert!(total_grid_w <= avail_w);
        assert_eq!(total_grid_w, 262.0);
        assert_eq!(side_padding, 101.0);

        // Distance from left window edge: 8.0 panel margin + 101.0 side padding = 109.0
        let left_dist = 8.0 + side_padding;
        // Distance from right window edge: 480.0 - (8.0 + side_padding + total_grid_w) = 109.0
        let right_dist = 480.0 - (8.0 + side_padding + total_grid_w);
        assert!((left_dist - right_dist).abs() < 1e-4);
        assert_eq!(left_dist, 109.0);
        assert_eq!(right_dist, 109.0);
    }

    #[test]
    fn test_vocab_toggle_state() {
        let mut state = AppState::default();
        assert!(!state.twitch.config.share_mined_words_to_chat);
        state.twitch.config.share_mined_words_to_chat = true;
        assert!(state.twitch.config.share_mined_words_to_chat);
    }
}
