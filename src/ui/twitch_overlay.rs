//! Twitch chat overlay: a full-height strip on the right edge of the video window,
//! drawn above everything else (like the Debug window).

use crate::app::AppState;
use crate::twitch::{
    emotes::Fragment, ChatMessage, ConnectionStatus, MessageKind, StreamStatus, TwitchState,
};
use eframe::egui;
use eframe::epaint::{text::LayoutJob, TextShape};
use std::sync::Arc;
use std::time::Instant;

const HEADER_H: f32 = 24.0;
const FOOTER_H: f32 = 32.0;
const MIN_WIDTH: f32 = 160.0;
/// Invisible glyph used to reserve space for an emote inside a text galley.
const EMOTE_PLACEHOLDER: char = 'M';

/// Called for the root video viewport after the video player.
pub fn draw(state: &mut AppState, ctx: &egui::Context) {
    if ctx.viewport_id() != egui::ViewportId::ROOT {
        return;
    }
    let tw = &mut state.twitch;
    if tw.config.chat_overlay_enabled && toggle_pressed(ctx) {
        tw.overlay_hidden = !tw.overlay_hidden;
    }
    if !tw.config.chat_overlay_enabled || tw.overlay_hidden {
        tw.overlay_rect = None;
        return;
    }

    let screen = ctx.screen_rect();
    let width = (screen.width() * tw.config.overlay_width_pct.clamp(0.1, 0.5))
        .max(MIN_WIDTH)
        .min(screen.width());
    let rect = egui::Rect::from_min_max(egui::pos2(screen.max.x - width, screen.min.y), screen.max);
    tw.overlay_rect = Some(rect);
    let now = Instant::now();

    // `Order::Debug` is painted after the `Tooltip` layer where the replay buffer
    // captures overlays, so chat is never recorded in replays (even with
    // "capture overlays" enabled) while still sitting above everything on screen.
    egui::Area::new(egui::Id::new("twitch-chat-overlay"))
        .order(egui::Order::Debug)
        .fixed_pos(rect.min)
        .constrain(false)
        .movable(false)
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());
            draw_contents(ui, tw, rect, now);
        });

    if let Some(after) = tw.next_fade_repaint(now) {
        ctx.request_repaint_after(after);
    }
}

/// `T` (no modifiers, window focused, not typing), same rules as the Debug `D` key.
fn toggle_pressed(ctx: &egui::Context) -> bool {
    !ctx.wants_keyboard_input()
        && ctx.input(|i| i.focused)
        && ctx.input_mut(|i| {
            let pressed = i.events.iter().any(|event| {
                matches!(event,
                    egui::Event::Key { key: egui::Key::T, pressed: true, repeat: false, modifiers, .. }
                    if modifiers.is_none()
                )
            });
            pressed && i.consume_key(egui::Modifiers::NONE, egui::Key::T)
        })
}

fn draw_contents(ui: &mut egui::Ui, tw: &mut TwitchState, rect: egui::Rect, now: Instant) {
    let painter = ui.painter_at(rect);
    let bg_alpha = (tw.config.overlay_opacity.clamp(0.0, 1.0) * 255.0) as u8;
    painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(bg_alpha));
    painter.vline(
        rect.left() + 0.5,
        rect.y_range(),
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(28)),
    );

    let header = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), HEADER_H));
    let footer = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - FOOTER_H), rect.max);
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), header.bottom()),
        egui::pos2(rect.right(), footer.top()),
    );

    draw_header(ui, tw, header);
    draw_messages(ui, tw, body, now);
    draw_footer(ui, tw, footer);
}

fn draw_header(ui: &mut egui::Ui, tw: &mut TwitchState, header: egui::Rect) {
    let painter = ui.painter_at(header);
    painter.rect_filled(header, 0.0, egui::Color32::from_black_alpha(90));
    painter.hline(
        header.x_range(),
        header.bottom() - 0.5,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(20)),
    );

    // Right side: whether the stream is actually on air (Helix API), never guessed.
    let (dot, live_text) = match &tw.stream_status {
        StreamStatus::Live => (egui::Color32::from_rgb(80, 200, 120), "LIVE"),
        StreamStatus::Offline => (egui::Color32::from_rgb(220, 70, 70), "offline"),
        StreamStatus::Unknown(_) => (egui::Color32::from_gray(110), "unknown"),
    };

    // Left side: channel name, plus chat connection state when not connected.
    let chat_state = match &tw.status {
        ConnectionStatus::Connected { .. } => None,
        ConnectionStatus::Connecting => Some("connecting…"),
        ConnectionStatus::Reconnecting { .. } => Some("reconnecting…"),
        ConnectionStatus::Disabled => Some("disconnected"),
    };
    let title = if tw.config.channel.is_empty() {
        "Twitch chat".to_string()
    } else {
        format!("#{}", tw.config.channel)
    };
    let title_rect = painter.text(
        egui::pos2(header.left() + 8.0, header.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::monospace(12.0),
        egui::Color32::from_gray(225),
    );
    if let Some(state) = chat_state {
        painter.text(
            egui::pos2(title_rect.right() + 6.0, header.center().y),
            egui::Align2::LEFT_CENTER,
            state,
            egui::FontId::monospace(10.0),
            egui::Color32::from_gray(130),
        );
    }

    let close_rect = egui::Rect::from_center_size(
        egui::pos2(header.right() - 13.0, header.center().y),
        egui::vec2(18.0, 18.0),
    );
    let live_rect = painter.text(
        egui::pos2(close_rect.left() - 6.0, header.center().y),
        egui::Align2::RIGHT_CENTER,
        live_text,
        egui::FontId::monospace(10.0),
        if matches!(tw.stream_status, StreamStatus::Unknown(_)) {
            egui::Color32::from_gray(140)
        } else {
            dot
        },
    );
    painter.circle_filled(egui::pos2(live_rect.left() - 7.0, header.center().y), 4.0, dot);
    // No hover tooltip: tooltips render beneath the chat's Debug layer.
    let close = ui.interact(close_rect, ui.id().with("twitch-chat-hide"), egui::Sense::click());
    let color = if close.hovered() {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_gray(160)
    };
    painter.text(close_rect.center(), egui::Align2::CENTER_CENTER, "×", egui::FontId::proportional(16.0), color);
    if close.clicked() {
        tw.overlay_hidden = true;
    }
}

fn draw_messages(ui: &mut egui::Ui, tw: &mut TwitchState, body: egui::Rect, now: Instant) {
    let inner = body.shrink2(egui::vec2(8.0, 6.0));
    if inner.width() <= 10.0 || inner.height() <= 10.0 {
        return;
    }
    let painter = ui.painter_at(body);
    let font_size = tw.config.font_size.clamp(8.0, 40.0);
    let font_id = egui::FontId::proportional(font_size);
    let line_h = (font_size * 1.45).round();
    let emote_size = line_h;
    let placeholder_w = ui.fonts(|f| f.glyph_width(&font_id, EMOTE_PLACEHOLDER));

    if tw.messages.is_empty() {
        let hint = if tw.config.channel.is_empty() {
            "Set your channel in\nControls → Twitch".to_string()
        } else {
            format!("Waiting for messages in #{}…", tw.config.channel)
        };
        painter.text(
            inner.center_bottom() - egui::vec2(0.0, 4.0),
            egui::Align2::CENTER_BOTTOM,
            hint,
            egui::FontId::proportional(12.0),
            egui::Color32::from_gray(120),
        );
        return;
    }

    let mut wanted_emotes: Vec<String> = Vec::new();
    let mut y = inner.bottom();
    for msg in tw.messages.iter().rev() {
        if y <= inner.top() {
            break;
        }
        let alpha = tw.message_alpha(msg, now);
        let (galley, emote_sections) =
            layout_message(ui, msg, &font_id, line_h, emote_size, placeholder_w, inner.width());
        y -= galley.size().y;
        let origin = egui::pos2(inner.left(), y);
        painter.add(TextShape {
            opacity_factor: alpha,
            ..TextShape::new(origin, galley.clone(), egui::Color32::from_gray(220))
        });

        for row in &galley.rows {
            for glyph in &row.glyphs {
                let Some(Some(id)) = emote_sections.get(glyph.section_index as usize) else {
                    continue;
                };
                let emote_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        origin.x + glyph.pos.x,
                        origin.y + row.rect.center().y - emote_size / 2.0,
                    ),
                    egui::vec2(emote_size, emote_size),
                );
                match tw.emotes.texture(id) {
                    Some(tex) => {
                        let size = tex.size_vec2();
                        // Keep aspect ratio inside the square slot.
                        let scale = (emote_size / size.x.max(1.0)).min(emote_size / size.y.max(1.0));
                        let draw = egui::Rect::from_center_size(emote_rect.center(), size * scale);
                        painter.image(
                            tex.id(),
                            draw,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE.gamma_multiply(alpha),
                        );
                    }
                    None => wanted_emotes.push(id.clone()),
                }
            }
        }
        y -= 4.0;
    }

    let ctx = ui.ctx().clone();
    for id in wanted_emotes {
        tw.emotes.request(&id, &ctx);
    }
}

/// Lay out a message as a single wrapped galley. Emotes are reserved as transparent
/// placeholder glyphs widened to the emote size; returns, per section, the emote id.
fn layout_message(
    ui: &egui::Ui,
    msg: &ChatMessage,
    font_id: &egui::FontId,
    line_h: f32,
    emote_size: f32,
    placeholder_w: f32,
    wrap_width: f32,
) -> (Arc<egui::Galley>, Vec<Option<String>>) {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    let base = egui::TextFormat {
        font_id: font_id.clone(),
        line_height: Some(line_h),
        color: egui::Color32::from_gray(225),
        valign: egui::Align::Center,
        ..Default::default()
    };
    let mut sections: Vec<Option<String>> = Vec::new();
    let mut push = |job: &mut LayoutJob, text: &str, format: egui::TextFormat, emote: Option<String>| {
        job.append(text, 0.0, format);
        sections.push(emote);
    };

    let name_color = readable_name_color(msg.color, &msg.login);
    let text_format = match msg.kind {
        MessageKind::System => egui::TextFormat {
            color: egui::Color32::from_gray(150),
            italics: true,
            ..base.clone()
        },
        MessageKind::Action => egui::TextFormat {
            color: name_color,
            italics: true,
            ..base.clone()
        },
        MessageKind::Chat => base.clone(),
    };

    match msg.kind {
        MessageKind::System => {}
        MessageKind::Action => {
            push(&mut job, &format!("{} ", msg.display_name), egui::TextFormat { color: name_color, ..base.clone() }, None);
        }
        MessageKind::Chat => {
            push(&mut job, &msg.display_name, egui::TextFormat { color: name_color, ..base.clone() }, None);
            push(&mut job, ": ", base.clone(), None);
        }
    }

    for fragment in &msg.fragments {
        match fragment {
            Fragment::Text(text) => push(&mut job, text, text_format.clone(), None),
            Fragment::Emote { id, .. } => {
                let format = egui::TextFormat {
                    color: egui::Color32::TRANSPARENT,
                    extra_letter_spacing: (emote_size - placeholder_w).max(0.0),
                    ..base.clone()
                };
                push(&mut job, &EMOTE_PLACEHOLDER.to_string(), format, Some(id.clone()));
            }
        }
    }

    let galley = ui.fonts(|f| f.layout_job(job));
    (galley, sections)
}

/// Twitch user color, or a stable fallback, brightened enough to read on a dark background.
fn readable_name_color(color: Option<egui::Color32>, login: &str) -> egui::Color32 {
    const FALLBACK: [egui::Color32; 8] = [
        egui::Color32::from_rgb(255, 99, 99),
        egui::Color32::from_rgb(99, 160, 255),
        egui::Color32::from_rgb(90, 210, 120),
        egui::Color32::from_rgb(255, 170, 70),
        egui::Color32::from_rgb(200, 120, 255),
        egui::Color32::from_rgb(70, 210, 210),
        egui::Color32::from_rgb(255, 120, 200),
        egui::Color32::from_rgb(220, 210, 90),
    ];
    let c = color.unwrap_or_else(|| {
        let hash = login.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
        FALLBACK[hash as usize % FALLBACK.len()]
    });
    let luma = 0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32;
    if luma >= 110.0 {
        return c;
    }
    // Blend toward white until readable.
    let t = ((110.0 - luma) / 255.0 * 1.6).clamp(0.0, 0.75);
    let mix = |v: u8| (v as f32 + (255.0 - v as f32) * t).round() as u8;
    egui::Color32::from_rgb(mix(c.r()), mix(c.g()), mix(c.b()))
}

fn draw_footer(ui: &mut egui::Ui, tw: &mut TwitchState, footer: egui::Rect) {
    ui.painter_at(footer).hline(
        footer.x_range(),
        footer.top() + 0.5,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(20)),
    );
    let can_send = tw.can_send();
    let hint = if can_send {
        "Send a message…"
    } else if tw.token.is_none() {
        "Log in via Controls → Twitch to chat"
    } else {
        "Connecting…"
    };
    let input_rect = footer.shrink2(egui::vec2(6.0, 5.0));
    ui.allocate_ui_at_rect(input_rect, |ui| {
        let response = ui.add_enabled(
            can_send,
            egui::TextEdit::singleline(&mut tw.input)
                .id(egui::Id::new("twitch-chat-input"))
                .hint_text(hint)
                .char_limit(500)
                .desired_width(input_rect.width()),
        );
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            tw.send_input();
            response.request_focus();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_name_colors_are_brightened() {
        let dark = readable_name_color(Some(egui::Color32::from_rgb(0, 0, 255)), "x");
        let luma = 0.2126 * dark.r() as f32 + 0.7152 * dark.g() as f32 + 0.0722 * dark.b() as f32;
        assert!(luma > 60.0);
        let bright = egui::Color32::from_rgb(255, 200, 0);
        assert_eq!(readable_name_color(Some(bright), "x"), bright);
        // Fallback is stable per login.
        assert_eq!(readable_name_color(None, "alice"), readable_name_color(None, "alice"));
    }

    #[test]
    fn toggle_key_hides_and_shows_overlay() {
        let ctx = egui::Context::default();
        let mut state = AppState::default();
        state.twitch.config.chat_overlay_enabled = true;
        let press = || {
            let mut input = egui::RawInput {
                focused: true,
                ..Default::default()
            };
            for pressed in [true, false] {
                input.events.push(egui::Event::Key {
                    key: egui::Key::T,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            input
        };
        let _ = ctx.run(egui::RawInput::default(), |ctx| draw(&mut state, ctx));
        let rect = state.twitch.overlay_rect.expect("overlay visible");
        // Must be above the Tooltip layer where replay capture of overlays happens.
        let layer = ctx.layer_id_at(rect.center()).expect("overlay layer");
        assert_eq!(layer.order, egui::Order::Debug);
        assert!(layer.order > egui::Order::Tooltip);
        let _ = ctx.run(press(), |ctx| draw(&mut state, ctx));
        assert!(state.twitch.overlay_hidden);
        assert!(state.twitch.overlay_rect.is_none());
        let _ = ctx.run(press(), |ctx| draw(&mut state, ctx));
        assert!(!state.twitch.overlay_hidden);

        // Disabled overlay ignores the key entirely.
        state.twitch.config.chat_overlay_enabled = false;
        let _ = ctx.run(press(), |ctx| draw(&mut state, ctx));
        assert!(!state.twitch.overlay_hidden);
        assert!(state.twitch.overlay_rect.is_none());
    }
}
