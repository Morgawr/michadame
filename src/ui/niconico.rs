//! Niconico-style chat comments: each chat message flies across the video from
//! right to left. The shapes are tessellated here and handed to the CRT
//! renderer, which draws them onto the image *before* the CRT post effects, so
//! they also end up in replays and mining screenshots.

use crate::app::AppState;
use crate::twitch::emotes::Fragment;
use crate::twitch::{Comment, TwitchState};
use eframe::egui::{self, epaint, Color32, Pos2, Rect, Shape};
use std::sync::Arc;
use std::time::Instant;

enum Piece {
    Text { white: Arc<egui::Galley> },
    Image { texture: egui::TextureId, size: egui::Vec2 },
}

/// Lay out comments for this frame and pass them to the renderer.
pub fn update(state: &mut AppState, ctx: &egui::Context, video_rect: Rect) {
    let primitives = if state.twitch.config.niconico_enabled && !state.twitch.comments.is_empty() {
        let screen = comment_area(video_rect);
        let shapes = build_shapes(&mut state.twitch, ctx, screen, Instant::now());
        if shapes.is_empty() {
            Vec::new()
        } else {
            // Comments move every frame.
            ctx.request_repaint();
            ctx.tessellate(shapes, ctx.pixels_per_point())
        }
    } else {
        Vec::new()
    };
    if let Some(r) = &state.crt_renderer {
        r.lock().unwrap().comment_primitives = primitives;
    }
}

/// The visible video picture area, where comments fly.
/// Using video_rect directly ensures that toggling the CRT shader on and off
/// does not jump or shift comment vertical positions.
#[inline]
pub fn comment_area(video_rect: Rect) -> Rect {
    video_rect
}

/// Comment font size in points for a given picture area.
pub fn font_size(size_pct: f32, area: Rect) -> f32 {
    (size_pct.clamp(0.02, 0.3) * area.height()).clamp(8.0, 160.0)
}

/// Left x of a comment: starts just off the right edge, ends just off the left.
pub fn comment_x(area: Rect, text_width: f32, progress: f32) -> f32 {
    area.right() - progress * (area.width() + text_width)
}

fn build_shapes(
    tw: &mut TwitchState,
    ctx: &egui::Context,
    area: Rect,
    now: Instant,
) -> Vec<epaint::ClippedShape> {
    if area.width() <= 1.0 || area.height() <= 1.0 {
        return Vec::new();
    }
    let size = font_size(tw.config.niconico_size_pct, area);
    let font = egui::FontId::proportional(size);
    let outline = (size * 0.06).clamp(1.0, 4.0);
    let mut shapes = Vec::new();
    let mut wanted_emotes = Vec::new();

    for comment in &tw.comments {
        let progress = comment.progress(now);
        if !(0.0..1.0).contains(&progress) {
            continue;
        }
        let pieces = layout_pieces(comment, tw, ctx, &font, size, &mut wanted_emotes);
        let width: f32 = pieces.iter().map(piece_width).sum();
        let height = pieces.iter().map(piece_height).fold(size, f32::max);
        let mut x = comment_x(area, width, progress);
        if x > area.right() || x + width < area.left() {
            continue;
        }
        let y = area.top() + comment.y_norm.clamp(0.0, 1.0) * (area.height() - height).max(0.0);

        for piece in pieces {
            match piece {
                Piece::Text { white } => {
                    let pos = Pos2::new(x, y + (height - white.size().y) * 0.5);
                    // Black border: offset copies all around, then white on top.
                    for (dx, dy) in [
                        (-1.0, -1.0),
                        (0.0, -1.0),
                        (1.0, -1.0),
                        (-1.0, 0.0),
                        (1.0, 0.0),
                        (-1.0, 1.0),
                        (0.0, 1.0),
                        (1.0, 1.0),
                    ] {
                        let p = pos + egui::vec2(dx, dy) * outline;
                        shapes.push(Shape::Text(epaint::TextShape {
                            override_text_color: Some(Color32::BLACK),
                            ..epaint::TextShape::new(p, white.clone(), Color32::BLACK)
                        }));
                    }
                    x += white.size().x;
                    shapes.push(Shape::Text(epaint::TextShape::new(pos, white, Color32::WHITE)));
                }
                Piece::Image { texture, size: img } => {
                    let rect = Rect::from_min_size(Pos2::new(x, y + (height - img.y) * 0.5), img);
                    shapes.push(Shape::image(
                        texture,
                        rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    ));
                    x += img.x;
                }
            }
        }
    }

    for id in wanted_emotes {
        tw.emotes.request(&id, ctx);
    }
    shapes
        .into_iter()
        .map(|shape| epaint::ClippedShape { clip_rect: area, shape })
        .collect()
}

fn layout_pieces(
    comment: &Comment,
    tw: &TwitchState,
    ctx: &egui::Context,
    font: &egui::FontId,
    size: f32,
    wanted_emotes: &mut Vec<String>,
) -> Vec<Piece> {
    let text_piece = |text: &str| Piece::Text {
        white: ctx.fonts(|f| f.layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE)),
    };
    let mut pieces = Vec::new();
    for fragment in &comment.fragments {
        match fragment {
            Fragment::Text(text) => {
                // Newlines would break the single-line flight path.
                let text = text.replace(['\n', '\r'], " ");
                if !text.is_empty() {
                    pieces.push(text_piece(&text));
                }
            }
            Fragment::Emote { id, name } => match tw.emotes.texture(id) {
                Some(tex) => {
                    let tex_size = tex.size_vec2();
                    let h = size * 1.2;
                    let w = if tex_size.y > 0.0 { h * tex_size.x / tex_size.y } else { h };
                    pieces.push(Piece::Image {
                        texture: tex.id(),
                        size: egui::vec2(w, h),
                    });
                }
                None => {
                    wanted_emotes.push(id.clone());
                    pieces.push(text_piece(name));
                }
            },
        }
    }
    pieces
}

fn piece_width(piece: &Piece) -> f32 {
    match piece {
        Piece::Text { white } => white.size().x,
        Piece::Image { size, .. } => size.x,
    }
}

fn piece_height(piece: &Piece) -> f32 {
    match piece {
        Piece::Text { white } => white.size().y,
        Piece::Image { size, .. } => size.y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::twitch::ChatMessage;
    use std::time::Duration;

    fn area() -> Rect {
        Rect::from_min_size(Pos2::new(100.0, 50.0), egui::vec2(800.0, 600.0))
    }

    #[test]
    fn comments_cross_from_right_to_left() {
        let a = area();
        assert_eq!(comment_x(a, 200.0, 0.0), a.right());
        assert_eq!(comment_x(a, 200.0, 1.0), a.left() - 200.0);
        assert!(comment_x(a, 200.0, 0.5) < a.right());
        assert_eq!(font_size(0.05, a), 30.0);
    }

    #[test]
    fn builds_outlined_clipped_shapes() {
        let ctx = egui::Context::default();
        let mut tw = TwitchState::default();
        tw.config.niconico_enabled = true;
        let start = Instant::now();
        let msg = ChatMessage {
            id: Some("1".into()),
            login: "a".into(),
            display_name: "A".into(),
            color: None,
            fragments: vec![Fragment::Text("こんにちは world".into())],
            kind: crate::twitch::MessageKind::Chat,
            received: start,
        };
        tw.spawn_comment(&msg, start, &mut rand::thread_rng());
        let mut shapes = Vec::new();
        let midway = start + tw.comments[0].duration / 2;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            shapes = build_shapes(&mut tw, ctx, area(), midway);
        });
        // 8 outline copies + 1 white text, all clipped to the picture.
        assert_eq!(shapes.len(), 9);
        assert!(shapes.iter().all(|s| s.clip_rect == area()));
        // Before spawning / after leaving, nothing is drawn.
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            shapes = build_shapes(&mut tw, ctx, area(), start + Duration::from_secs(120));
        });
        assert!(shapes.is_empty());
    }

    #[test]
    fn comment_area_is_stable_with_video_rect() {
        let vr = area();
        assert_eq!(comment_area(vr), vr);
    }
}
