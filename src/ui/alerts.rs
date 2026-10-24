//! Twitch video alerts renderer: draws animated event notification cards
//! (followers, subscriptions, bits, and raids) directly onto the video surface
//! before CRT shaders and capture pipelines (like niconico comments).

use crate::app::AppState;
use crate::twitch::alerts::{ActiveAlert, AlertKind};
use eframe::egui::{self, epaint, Color32, FontId, Pos2, Rect, Rounding, Shape, Stroke, Vec2};
use std::time::Instant;

const TRANSITION_DURATION: f32 = 0.45;

/// Updates and lays out the active video alert notice, tessellating it and handing
/// primitives to `CrtFilterRenderer` to be drawn on the video image.
pub fn update(state: &mut AppState, ctx: &egui::Context, video_rect: Rect) {
    if state.twitch.is_roll_active() {
        if let Some(r) = &state.crt_renderer {
            r.lock().unwrap().alert_primitives = Vec::new();
        }
        return;
    }
    let primitives = if state.twitch.config.alerts_enabled {
        if let Some(active) = &state.twitch.alerts.active {
            let now = Instant::now();
            let shapes = build_alert_shapes(active, ctx, video_rect, now);
            if shapes.is_empty() {
                Vec::new()
            } else {
                ctx.request_repaint();
                ctx.tessellate(shapes, ctx.pixels_per_point())
            }
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    if let Some(r) = &state.crt_renderer {
        r.lock().unwrap().alert_primitives = primitives;
    }
}

fn ease_out_cubic(x: f32) -> f32 {
    1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3)
}

fn ease_in_cubic(x: f32) -> f32 {
    x.clamp(0.0, 1.0).powi(3)
}

fn build_alert_shapes(
    active: &ActiveAlert,
    _ctx: &egui::Context,
    video_rect: Rect,
    now: Instant,
) -> Vec<epaint::ClippedShape> {
    if video_rect.width() <= 10.0 || video_rect.height() <= 10.0 {
        return Vec::new();
    }

    let elapsed = now.saturating_duration_since(active.started).as_secs_f32();
    let total = active.duration.as_secs_f32().max(0.1);

    if elapsed >= total {
        return Vec::new();
    }

    // Animation progress & opacity
    let (slide, alpha) = if elapsed < TRANSITION_DURATION {
        let f = (elapsed / TRANSITION_DURATION).clamp(0.0, 1.0);
        (ease_out_cubic(f), f)
    } else if elapsed > total - TRANSITION_DURATION {
        let f = ((total - elapsed) / TRANSITION_DURATION).clamp(0.0, 1.0);
        (ease_in_cubic(f), f)
    } else {
        (1.0, 1.0)
    };

    // Unified PC-98 system / office palette: mellowed brown, warm gray, beige, and dark CRT slate
    let frame_color = Color32::from_rgb(186, 174, 158).linear_multiply(alpha); // PC-98 computer beige / warm gray
    let hairline_color = Color32::from_rgb(76, 70, 64).linear_multiply(alpha);  // Muted dark stone
    let bg_color = Color32::from_rgba_premultiplied(22, 20, 18, (alpha * 242.0) as u8); // Deep charcoal-brown CRT slate
    let header_bg = Color32::from_rgba_premultiplied(36, 32, 28, (alpha * 220.0) as u8); // Muted dark sepia-charcoal banner
    let rule_color = Color32::from_rgb(126, 116, 104).linear_multiply(alpha);  // Muted beige divider rule
    let header_text_color = Color32::from_rgb(216, 206, 192).linear_multiply(alpha); // Mellow warm ivory
    let primary_text_color = Color32::from_rgb(246, 242, 236).linear_multiply(alpha); // Crisp warm cream
    let detail_text_color = Color32::from_rgb(164, 154, 142).linear_multiply(alpha); // Muted stone gray
    let msg_text_color = Color32::from_rgb(136, 126, 114).linear_multiply(alpha);    // Soft warm taupe
    let gauge_bg = Color32::from_rgba_premultiplied(30, 27, 24, (alpha * 180.0) as u8);
    let gauge_fill = Color32::from_rgb(168, 156, 140).linear_multiply(alpha);  // Muted parchment fill

    let (header_text, primary_text, detail_text, user_msg) =
        format_alert_text(&active.event.kind);

    let has_detail = detail_text.is_some();
    let has_msg = user_msg.is_some();

    // 3x Scale: Width 600-840pt, Height 140-184pt
    let card_w = (video_rect.width() * 0.75).clamp(600.0, 840.0);
    let card_h = if has_msg {
        184.0
    } else if has_detail {
        152.0
    } else {
        136.0
    };

    let center_x = video_rect.center().x;
    let target_top = video_rect.min.y + (video_rect.height() * 0.04).clamp(16.0, 48.0);
    let offscreen_top = video_rect.min.y - (card_h + 20.0);
    let current_top = offscreen_top + (target_top - offscreen_top) * slide;

    let card_rect = Rect::from_min_size(
        Pos2::new(center_x - card_w / 2.0, current_top),
        Vec2::new(card_w, card_h),
    );

    let mut shapes = Vec::new();

    // 1. Deep Slate CRT Bezel Background
    shapes.push(Shape::rect_filled(card_rect, Rounding::same(2.0), bg_color));

    // 2. Outer PC-98 Bezel Border Stroke (2.5px)
    shapes.push(Shape::rect_stroke(
        card_rect,
        Rounding::same(2.0),
        Stroke::new(2.5, frame_color),
    ));

    // 3. Inner Hairline Stroke (1.0px) inset by 3.5px
    let inner_rect = card_rect.shrink(3.5);
    shapes.push(Shape::rect_stroke(
        inner_rect,
        Rounding::same(1.0),
        Stroke::new(1.0, hairline_color),
    ));

    // 4. Header Bar (Dark Sepia Banner + Muted Separator Rule)
    let header_h = 42.0;
    let header_rect = Rect::from_min_max(
        card_rect.min,
        Pos2::new(card_rect.max.x, card_rect.min.y + header_h),
    );
    shapes.push(Shape::rect_filled(
        header_rect,
        Rounding {
            nw: 2.0,
            ne: 2.0,
            sw: 0.0,
            se: 0.0,
        },
        header_bg,
    ));

    // Divider Rule
    let rule_y = card_rect.min.y + header_h;
    shapes.push(Shape::line_segment(
        [
            Pos2::new(card_rect.min.x, rule_y),
            Pos2::new(card_rect.max.x, rule_y),
        ],
        Stroke::new(1.5, rule_color),
    ));

    // 5. Technical Reticle Corner Brackets
    let bracket_len = 10.0;
    let bracket_stroke = Stroke::new(2.0, frame_color);
    let b_offset = 6.0;

    // Top-Left
    let tl = Pos2::new(card_rect.min.x + b_offset, card_rect.min.y + b_offset);
    shapes.push(Shape::line_segment(
        [tl, Pos2::new(tl.x + bracket_len, tl.y)],
        bracket_stroke,
    ));
    shapes.push(Shape::line_segment(
        [tl, Pos2::new(tl.x, tl.y + bracket_len)],
        bracket_stroke,
    ));

    // Top-Right
    let tr = Pos2::new(card_rect.max.x - b_offset, card_rect.min.y + b_offset);
    shapes.push(Shape::line_segment(
        [tr, Pos2::new(tr.x - bracket_len, tr.y)],
        bracket_stroke,
    ));
    shapes.push(Shape::line_segment(
        [tr, Pos2::new(tr.x, tr.y + bracket_len)],
        bracket_stroke,
    ));

    // Bottom-Left
    let bl = Pos2::new(card_rect.min.x + b_offset, card_rect.max.y - b_offset);
    shapes.push(Shape::line_segment(
        [bl, Pos2::new(bl.x + bracket_len, bl.y)],
        bracket_stroke,
    ));
    shapes.push(Shape::line_segment(
        [bl, Pos2::new(bl.x, bl.y - bracket_len)],
        bracket_stroke,
    ));

    // Bottom-Right
    let br = Pos2::new(card_rect.max.x - b_offset, card_rect.max.y - b_offset);
    shapes.push(Shape::line_segment(
        [br, Pos2::new(br.x - bracket_len, br.y)],
        bracket_stroke,
    ));
    shapes.push(Shape::line_segment(
        [br, Pos2::new(br.x, br.y - bracket_len)],
        bracket_stroke,
    ));

    // 6. Header Title Text (Massive, Bold Monospace: 22pt)
    let header_pos = Pos2::new(card_rect.min.x + 24.0, card_rect.min.y + 21.0);
    shapes.push(_ctx.fonts(|f| {
        Shape::text(
            f,
            header_pos,
            egui::Align2::LEFT_CENTER,
            header_text,
            FontId::monospace(22.0),
            header_text_color,
        )
    }));

    // 7. Primary Text (User Name: 36pt Bold)
    let primary_pos = Pos2::new(card_rect.min.x + 24.0, card_rect.min.y + 75.0);
    shapes.push(_ctx.fonts(|f| {
        Shape::text(
            f,
            primary_pos,
            egui::Align2::LEFT_CENTER,
            primary_text,
            FontId::proportional(36.0),
            primary_text_color,
        )
    }));

    // 8. Detail Line (18pt Monospace)
    if let Some(detail) = detail_text {
        let detail_pos = Pos2::new(card_rect.min.x + 25.0, card_rect.min.y + 114.0);
        shapes.push(_ctx.fonts(|f| {
            Shape::text(
                f,
                detail_pos,
                egui::Align2::LEFT_CENTER,
                detail,
                FontId::monospace(18.0),
                detail_text_color,
            )
        }));
    }

    // 9. User Message Line (if present, 16pt Proportional)
    if let Some(msg) = user_msg {
        let msg_pos = Pos2::new(card_rect.min.x + 25.0, card_rect.min.y + 144.0);
        shapes.push(_ctx.fonts(|f| {
            Shape::text(
                f,
                msg_pos,
                egui::Align2::LEFT_CENTER,
                format!("\"{}\"", msg),
                FontId::proportional(16.0),
                msg_text_color,
            )
        }));
    }

    // 10. Mellow Duration Gauge (Bottom Progress Bar)
    let gauge_h = 4.0;
    let gauge_pad = 8.0;
    let gauge_bg_rect = Rect::from_min_max(
        Pos2::new(
            card_rect.min.x + gauge_pad,
            card_rect.max.y - gauge_h - 4.0,
        ),
        Pos2::new(card_rect.max.x - gauge_pad, card_rect.max.y - 4.0),
    );
    shapes.push(Shape::rect_filled(
        gauge_bg_rect,
        Rounding::ZERO,
        gauge_bg,
    ));

    let remaining_frac = 1.0 - (elapsed / total).clamp(0.0, 1.0);
    let fill_w = gauge_bg_rect.width() * remaining_frac;
    if fill_w > 0.0 {
        let gauge_fill_rect = Rect::from_min_size(
            gauge_bg_rect.min,
            Vec2::new(fill_w, gauge_h),
        );
        shapes.push(Shape::rect_filled(
            gauge_fill_rect,
            Rounding::ZERO,
            gauge_fill,
        ));
    }

    vec![epaint::ClippedShape {
        clip_rect: video_rect,
        shape: Shape::Vec(shapes),
    }]
}

fn format_alert_text(
    kind: &AlertKind,
) -> (String, String, Option<String>, Option<String>) {
    match kind {
        AlertKind::Follow { user } => (
            "[ SYSTEM NOTICE // NEW FOLLOWER ]".to_string(),
            user.clone(),
            Some("NOW FOLLOWING THE BROADCAST".to_string()),
            None,
        ),
        AlertKind::Subscription {
            user,
            tier,
            months,
            is_gift,
            gifter,
            message,
        } => {
            if *is_gift {
                let gifter_name = gifter.as_deref().unwrap_or("ANONYMOUS");
                (
                    "[ SYSTEM NOTICE // GIFT SUBSCRIPTION ]".to_string(),
                    format!("{gifter_name} -> {user}"),
                    Some(format!("GIFTED A {} SUBSCRIPTION", tier.to_uppercase())),
                    message.clone(),
                )
            } else {
                let tier_upper = tier.to_uppercase();
                let detail = if *months > 1 {
                    format!("{tier_upper} // {months} MONTHS")
                } else {
                    format!("{tier_upper} SUBSCRIPTION")
                };
                (
                    "[ SYSTEM NOTICE // NEW SUBSCRIBER ]".to_string(),
                    user.clone(),
                    Some(detail),
                    message.clone(),
                )
            }
        }
        AlertKind::Bits {
            user,
            amount,
            message,
        } => (
            format!("[ SYSTEM NOTICE // CHEER // {amount} BITS ]"),
            user.clone(),
            Some(format!("CHEERED {amount} BITS")),
            message.clone(),
        ),
        AlertKind::Raid { user, viewers } => (
            "[ SYSTEM NOTICE // INCOMING RAID ]".to_string(),
            user.clone(),
            Some(format!("RAIDING WITH {viewers} VIEWERS")),
            None,
        ),
    }
}
