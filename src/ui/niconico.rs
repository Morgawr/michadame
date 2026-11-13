//! Niconico-style chat comments: each chat message flies across the video from
//! right to left. The shapes are tessellated here and handed to the CRT
//! renderer, which draws them onto the image *before* the CRT post effects, so
//! they also end up in replays and mining screenshots.

use crate::app::AppState;
use crate::twitch::emotes::Fragment;
use crate::twitch::{Comment, TwitchState};
use eframe::egui::{self, epaint, Color32, Pos2, Rect, Shape};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

const HAD_PRIMITIVES_KEY: &str = "niconico_had_primitives";
const GALLEY_CACHE_KEY: &str = "niconico_galley_cache";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct GalleyCacheKey {
    fragments_ptr: usize,
    spawned: Instant,
    y_bits: u32,
    frag_idx: usize,
    is_emote_fallback: bool,
    font_size_bits: u32,
    ppp_bits: u32,
}

#[derive(Clone, Default)]
struct NiconicoGalleyCache {
    generation: u64,
    entries: HashMap<GalleyCacheKey, (Option<Arc<egui::Galley>>, u64)>,
}

#[inline]
fn had_primitives_id() -> egui::Id {
    egui::Id::new(HAD_PRIMITIVES_KEY)
}

#[inline]
fn galley_cache_id() -> egui::Id {
    egui::Id::new(GALLEY_CACHE_KEY)
}

fn evict_all_cached_galleys(ctx: &egui::Context) {
    let cache_id = galley_cache_id();
    let has_entries = ctx.data(|d| {
        d.get_temp::<NiconicoGalleyCache>(cache_id)
            .is_some_and(|c| !c.entries.is_empty())
    });
    if has_entries {
        ctx.data_mut(|d| {
            d.get_temp_mut_or_default::<NiconicoGalleyCache>(cache_id)
                .entries
                .clear();
        });
    }
}

/// Synchronizes `primitives` into the renderer only when `primitives` is non-empty
/// or on the transition from non-empty -> empty, avoiding idle `crt_renderer.lock()` contention.
fn sync_primitives_if_needed<F>(
    ctx: &egui::Context,
    renderer_token: Option<usize>,
    primitives: Vec<epaint::ClippedPrimitive>,
    write_fn: F,
) -> bool
where
    F: FnOnce(Vec<epaint::ClippedPrimitive>),
{
    let gate_id = had_primitives_id();
    let Some(token) = renderer_token else {
        let had_state = ctx.data(|d| d.get_temp::<(usize, bool)>(gate_id).is_some());
        if had_state {
            ctx.data_mut(|d| d.remove::<(usize, bool)>(gate_id));
        }
        return false;
    };

    let prev = ctx.data(|d| d.get_temp::<(usize, bool)>(gate_id));
    if primitives.is_empty() {
        if prev != Some((token, true)) {
            if prev != Some((token, false)) {
                ctx.data_mut(|d| d.insert_temp(gate_id, (token, false)));
            }
            return false;
        }
        write_fn(Vec::new());
        ctx.data_mut(|d| d.insert_temp(gate_id, (token, false)));
        true
    } else {
        write_fn(primitives);
        if prev != Some((token, true)) {
            ctx.data_mut(|d| d.insert_temp(gate_id, (token, true)));
        }
        true
    }
}

enum Piece {
    Text {
        white: Arc<egui::Galley>,
    },
    Image {
        texture: egui::TextureId,
        size: egui::Vec2,
    },
}

/// Lay out comments for this frame and pass them to the renderer.
pub fn update(state: &mut AppState, ctx: &egui::Context, video_rect: Rect) {
    let primitives = if !state.twitch.is_roll_active()
        && state.twitch.config.niconico_enabled
        && !state.twitch.comments.is_empty()
    {
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
        evict_all_cached_galleys(ctx);
        Vec::new()
    };

    let renderer_token = state.crt_renderer.as_ref().map(|r| Arc::as_ptr(r) as usize);
    let crt_renderer = &state.crt_renderer;
    sync_primitives_if_needed(ctx, renderer_token, primitives, |prims| {
        if let Some(r) = crt_renderer {
            r.lock().unwrap().comment_primitives = prims;
        }
    });
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
        evict_all_cached_galleys(ctx);
        return Vec::new();
    }
    let size = font_size(tw.config.niconico_size_pct, area);
    let font = egui::FontId::proportional(size);
    let outline = (size * 0.06).clamp(1.0, 4.0);
    let mut shapes = Vec::new();
    let mut wanted_emotes = Vec::new();

    let cache_id = galley_cache_id();
    let mut galley_cache = ctx
        .data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<NiconicoGalleyCache>(cache_id)));
    galley_cache.generation = galley_cache.generation.wrapping_add(1);
    let generation = galley_cache.generation;
    let font_size_bits = size.to_bits();
    let ppp_bits = ctx.pixels_per_point().to_bits();

    for comment in &tw.comments {
        let progress = comment.progress(now);
        if !(0.0..1.0).contains(&progress) {
            continue;
        }
        let pieces = layout_pieces(
            comment,
            tw,
            ctx,
            &font,
            size,
            font_size_bits,
            ppp_bits,
            generation,
            &mut galley_cache,
            &mut wanted_emotes,
        );
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
                    shapes.push(Shape::Text(epaint::TextShape::new(
                        pos,
                        white,
                        Color32::WHITE,
                    )));
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

    galley_cache
        .entries
        .retain(|_, (_, seen_gen)| *seen_gen == generation);
    ctx.data_mut(|d| d.insert_temp(cache_id, galley_cache));

    for id in wanted_emotes {
        tw.emotes.request(&id, ctx);
    }
    shapes
        .into_iter()
        .map(|shape| epaint::ClippedShape {
            clip_rect: area,
            shape,
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn layout_pieces(
    comment: &Comment,
    tw: &TwitchState,
    ctx: &egui::Context,
    font: &egui::FontId,
    size: f32,
    font_size_bits: u32,
    ppp_bits: u32,
    generation: u64,
    galley_cache: &mut NiconicoGalleyCache,
    wanted_emotes: &mut Vec<String>,
) -> Vec<Piece> {
    let fragments_ptr = comment.fragments.as_ptr() as usize;
    let y_bits = comment.y_norm.to_bits();
    let mut pieces = Vec::new();
    for (frag_idx, fragment) in comment.fragments.iter().enumerate() {
        match fragment {
            Fragment::Text(text) => {
                let key = GalleyCacheKey {
                    fragments_ptr,
                    spawned: comment.spawned,
                    y_bits,
                    frag_idx,
                    is_emote_fallback: false,
                    font_size_bits,
                    ppp_bits,
                };
                let cached = if let Some((opt_galley, seen_gen)) =
                    galley_cache.entries.get_mut(&key)
                {
                    *seen_gen = generation;
                    opt_galley.clone()
                } else {
                    // Newlines would break the single-line flight path.
                    let sanitized = if text.contains(['\n', '\r']) {
                        Cow::Owned(text.replace(['\n', '\r'], " "))
                    } else {
                        Cow::Borrowed(text.as_str())
                    };
                    let built = if sanitized.is_empty() {
                        None
                    } else {
                        Some(ctx.fonts(|f| {
                            f.layout_no_wrap(sanitized.into_owned(), font.clone(), Color32::WHITE)
                        }))
                    };
                    galley_cache
                        .entries
                        .insert(key, (built.clone(), generation));
                    built
                };
                if let Some(white) = cached {
                    pieces.push(Piece::Text { white });
                }
            }
            Fragment::Emote { id, name } => match tw.emotes.texture(id) {
                Some(tex) => {
                    let tex_size = tex.size_vec2();
                    let h = size * 1.2;
                    let w = if tex_size.y > 0.0 {
                        h * tex_size.x / tex_size.y
                    } else {
                        h
                    };
                    pieces.push(Piece::Image {
                        texture: tex.id(),
                        size: egui::vec2(w, h),
                    });
                }
                None => {
                    wanted_emotes.push(id.clone());
                    let key = GalleyCacheKey {
                        fragments_ptr,
                        spawned: comment.spawned,
                        y_bits,
                        frag_idx,
                        is_emote_fallback: true,
                        font_size_bits,
                        ppp_bits,
                    };
                    let white = if let Some((Some(galley), seen_gen)) =
                        galley_cache.entries.get_mut(&key)
                    {
                        *seen_gen = generation;
                        Arc::clone(galley)
                    } else {
                        let built = ctx.fonts(|f| {
                            f.layout_no_wrap(name.clone(), font.clone(), Color32::WHITE)
                        });
                        galley_cache
                            .entries
                            .insert(key, (Some(Arc::clone(&built)), generation));
                        built
                    };
                    pieces.push(Piece::Text { white });
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
            is_streamer: false,
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

    #[test]
    fn skips_idle_renderer_lock_and_clears_once_on_empty_transition() {
        let ctx = egui::Context::default();
        let token = Some(0xDEAD_BEEFusize);
        let mut lock_count = 0usize;
        let mut stored_len = 0usize;

        // Steady-state empty frames never acquire crt_renderer lock.
        for _ in 0..5 {
            let locked = sync_primitives_if_needed(&ctx, token, Vec::new(), |prims| {
                lock_count += 1;
                stored_len = prims.len();
            });
            assert!(!locked);
        }
        assert_eq!(lock_count, 0);

        // Non-empty frame acquires lock and stores primitives.
        let mut non_empty = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            non_empty = ctx.tessellate(
                vec![epaint::ClippedShape {
                    clip_rect: area(),
                    shape: Shape::rect_filled(area(), egui::Rounding::ZERO, Color32::WHITE),
                }],
                1.0,
            );
        });
        assert!(!non_empty.is_empty());
        assert!(sync_primitives_if_needed(&ctx, token, non_empty, |prims| {
            lock_count += 1;
            stored_len = prims.len();
        }));
        assert_eq!(lock_count, 1);
        assert!(stored_len > 0);

        // First empty frame after active comments locks ONCE to clear `comment_primitives`.
        assert!(sync_primitives_if_needed(
            &ctx,
            token,
            Vec::new(),
            |prims| {
                lock_count += 1;
                stored_len = prims.len();
            }
        ));
        assert_eq!(lock_count, 2);
        assert_eq!(stored_len, 0);

        // Subsequent empty frames skip locking completely again.
        for _ in 0..5 {
            assert!(!sync_primitives_if_needed(&ctx, token, Vec::new(), |_| {
                lock_count += 1;
            }));
        }
        assert_eq!(lock_count, 2);
    }

    #[test]
    fn caches_comment_galleys_across_frames_and_evicts_when_expired() {
        let ctx = egui::Context::default();
        let mut tw = TwitchState::default();
        tw.config.niconico_enabled = true;
        let start = Instant::now();
        let msg = ChatMessage {
            id: Some("cached-1".into()),
            login: "viewer".into(),
            display_name: "Viewer".into(),
            color: None,
            fragments: vec![Fragment::Text("再利用テスト\nline2".into())],
            kind: crate::twitch::MessageKind::Chat,
            received: start,
            is_streamer: false,
        };
        tw.spawn_comment(&msg, start, &mut rand::thread_rng());
        let dur = tw.comments[0].duration;

        let extract_white_galley = |shapes: &[epaint::ClippedShape]| -> Arc<egui::Galley> {
            match &shapes.last().expect("expected white text shape").shape {
                Shape::Text(t) => Arc::clone(&t.galley),
                other => panic!("expected Shape::Text, got {other:?}"),
            }
        };

        let mut shapes_1 = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            shapes_1 = build_shapes(&mut tw, ctx, area(), start + dur / 3);
        });
        let galley_1 = extract_white_galley(&shapes_1);

        let mut shapes_2 = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            shapes_2 = build_shapes(&mut tw, ctx, area(), start + dur / 2);
        });
        let galley_2 = extract_white_galley(&shapes_2);
        assert!(Arc::ptr_eq(&galley_1, &galley_2));

        let cached_count = ctx.data(|d| {
            d.get_temp::<NiconicoGalleyCache>(galley_cache_id())
                .map_or(0, |c| c.entries.len())
        });
        assert_eq!(cached_count, 1);

        // Changing font size invalidates and evicts the prior size entry on the same pass.
        tw.config.niconico_size_pct = 0.08;
        let mut shapes_resized = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            shapes_resized = build_shapes(&mut tw, ctx, area(), start + dur / 2);
        });
        let galley_resized = extract_white_galley(&shapes_resized);
        assert!(!Arc::ptr_eq(&galley_1, &galley_resized));
        let cached_after_resize = ctx.data(|d| {
            d.get_temp::<NiconicoGalleyCache>(galley_cache_id())
                .map_or(0, |c| c.entries.len())
        });
        assert_eq!(cached_after_resize, 1);

        // Once comment flight finishes, unseen cached galleys are evicted automatically.
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let expired = build_shapes(&mut tw, ctx, area(), start + Duration::from_secs(120));
            assert!(expired.is_empty());
        });
        let cached_after_expiry = ctx.data(|d| {
            d.get_temp::<NiconicoGalleyCache>(galley_cache_id())
                .map_or(0, |c| c.entries.len())
        });
        assert_eq!(cached_after_expiry, 0);
    }
}
