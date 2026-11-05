//! "Faded Indigo" retro theme: a PC-98 era visual novel look (dusty violet, muted gold, brick)
//! shared by the dictionary popup and the mining bank.
//!
//! Design notes:
//! - Window translucency uses a 2×2 checkerboard dither instead of alpha blending, the way
//!   16-color PC-98 games faked it. Under the CRT shader the dither blends into a soft
//!   translucent fill.
//! - Lines are thick (≥ 2 units) and contrast stays moderate (warm near-black / cream instead
//!   of pure black / white) so the CRT blur and bloom do not wash out or blow up details.
//! - Corner gems palette-cycle in 4 hard steps and the page cursor (`▼`) blinks, both driven
//!   by `ui.input(|i| i.time)`.

use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke};

// ---------------------------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------------------------

/// Window fill (dusty indigo).
pub const WINDOW: Color32 = Color32::from_rgb(26, 24, 34);
/// Deeper background used behind windows (e.g. bank panels).
pub const BACKDROP: Color32 = Color32::from_rgb(17, 16, 23);
/// Outer frame line (parchment).
pub const FRAME: Color32 = Color32::from_rgb(206, 198, 186);
/// Inner rule / dividers (faded violet).
pub const RULE: Color32 = Color32::from_rgb(102, 92, 122);
/// Hard outline and drop shadow (warm near-black).
pub const EDGE: Color32 = Color32::from_rgb(8, 7, 12);
/// Body text.
pub const TEXT: Color32 = Color32::from_rgb(220, 214, 204);
/// Headword text (cream).
pub const HEAD: Color32 = Color32::from_rgb(238, 232, 222);
/// Readings, sense numbers and accents (muted gold).
pub const GOLD: Color32 = Color32::from_rgb(196, 166, 118);
/// Pitch accent lines and warnings (brick).
pub const BRICK: Color32 = Color32::from_rgb(166, 94, 88);
/// Secondary / muted text.
pub const MUTED: Color32 = Color32::from_rgb(140, 132, 148);

/// Deinflection chip.
pub const DEINFLECT_BG: Color32 = Color32::from_rgb(74, 52, 70);
pub const DEINFLECT_BORDER: Color32 = Color32::from_rgb(134, 104, 126);
pub const DEINFLECT_TEXT: Color32 = Color32::from_rgb(230, 214, 224);

/// Frequency chip (faded teal).
pub const FREQ_BG: Color32 = Color32::from_rgb(34, 50, 56);
pub const FREQ_BORDER: Color32 = Color32::from_rgb(102, 138, 144);
pub const FREQ_TEXT: Color32 = Color32::from_rgb(166, 192, 196);

/// Generic tag chip.
pub const TAG_BG: Color32 = Color32::from_rgb(34, 31, 44);
pub const TAG_BORDER: Color32 = Color32::from_rgb(92, 84, 104);
pub const TAG_TEXT: Color32 = Color32::from_rgb(164, 156, 172);

/// "Mined" chip (gold on umber).
pub const MINED_BG: Color32 = Color32::from_rgb(74, 62, 44);
pub const MINED_BORDER: Color32 = GOLD;
pub const MINED_TEXT: Color32 = Color32::from_rgb(234, 220, 192);

/// Custom-name chip (slate).
pub const CUSTOM_BG: Color32 = Color32::from_rgb(40, 46, 64);
pub const CUSTOM_BORDER: Color32 = Color32::from_rgb(102, 114, 148);
pub const CUSTOM_TEXT: Color32 = Color32::from_rgb(176, 186, 210);

/// Example sentences.
pub const EXAMPLE_ACCENT: Color32 = Color32::from_rgb(168, 122, 96);
/// `rgba(168, 122, 96, 0.12)` premultiplied.
pub const EXAMPLE_BG: Color32 = Color32::from_rgba_premultiplied(20, 15, 12, 31);
pub const EXAMPLE_JA: Color32 = Color32::from_rgb(230, 224, 214);
pub const EXAMPLE_EN: Color32 = Color32::from_rgb(162, 154, 168);

/// Bevel button faces.
pub const BUTTON_MINE: Color32 = Color32::from_rgb(110, 68, 88);
pub const BUTTON_MINE_HOVER: Color32 = Color32::from_rgb(136, 88, 110);
pub const BUTTON_CLIP: Color32 = Color32::from_rgb(68, 60, 86);
pub const BUTTON_CLIP_HOVER: Color32 = Color32::from_rgb(90, 80, 112);
pub const BUTTON_DANGER_HOVER: Color32 = Color32::from_rgb(92, 44, 44);
pub const DANGER_TEXT: Color32 = Color32::from_rgb(220, 160, 150);
/// Bevel highlight (top / left) and lowlight (bottom / right).
pub const BEVEL_HI: Color32 = Color32::from_rgb(210, 202, 192);
pub const BEVEL_LO: Color32 = Color32::from_rgb(14, 12, 20);

/// OCR word highlight (dashed gold box).
pub const WORD_HIGHLIGHT_STROKE: Color32 = GOLD;
/// `rgba(196, 166, 118, 0.16)` premultiplied.
pub const WORD_HIGHLIGHT_FILL: Color32 = Color32::from_rgba_premultiplied(31, 27, 19, 40);

/// Corner gem palette cycle (4 hard steps).
pub const GEM_CYCLE: [Color32; 4] = [BRICK, GOLD, FREQ_BORDER, DEINFLECT_BORDER];

/// Opaque / see-through alpha of the two checkerboard cells of the window dither.
const DITHER_ALPHA_SOLID: u8 = 247;
const DITHER_ALPHA_SHEER: u8 = 140;

/// Seconds per palette-cycle step of the corner gems.
const GEM_STEP_SECS: f64 = 0.8;
/// Blink period of the page cursor.
const BLINK_PERIOD_SECS: f64 = 1.0;

// ---------------------------------------------------------------------------------------------
// Animation helpers
// ---------------------------------------------------------------------------------------------

/// Index into [`GEM_CYCLE`] for the given time and per-gem offset.
pub fn gem_color(time: f64, offset: usize) -> Color32 {
    let step = (time / GEM_STEP_SECS).floor().max(0.0) as usize;
    GEM_CYCLE[(step + offset) % GEM_CYCLE.len()]
}

/// True during the visible half of the cursor blink.
pub fn blink_on(time: f64) -> bool {
    (time % BLINK_PERIOD_SECS) < BLINK_PERIOD_SECS * 0.5
}

/// Requests a repaint exactly when the next blink or gem step happens.
pub fn request_animation_repaint(ctx: &egui::Context, time: f64) {
    let half = BLINK_PERIOD_SECS * 0.5;
    let to_blink = half - (time % half);
    let to_gem = GEM_STEP_SECS - (time % GEM_STEP_SECS);
    let secs = to_blink.min(to_gem).max(0.01);
    ctx.request_repaint_after(std::time::Duration::from_secs_f64(secs));
}

// ---------------------------------------------------------------------------------------------
// Window drawing
// ---------------------------------------------------------------------------------------------

/// The 2×2 checkerboard texture used for dithered window fills (nearest + repeat).
fn dither_texture(ctx: &egui::Context) -> egui::TextureId {
    let id = egui::Id::new("retro_theme_dither_texture");
    if let Some(handle) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return handle.id();
    }
    let solid =
        Color32::from_rgba_unmultiplied(WINDOW.r(), WINDOW.g(), WINDOW.b(), DITHER_ALPHA_SOLID);
    let sheer =
        Color32::from_rgba_unmultiplied(WINDOW.r(), WINDOW.g(), WINDOW.b(), DITHER_ALPHA_SHEER);
    let image = egui::ColorImage {
        size: [2, 2],
        pixels: vec![solid, sheer, sheer, solid],
    };
    let handle = ctx.load_texture(
        "retro_theme_dither",
        image,
        egui::TextureOptions::NEAREST_REPEAT,
    );
    let tex_id = handle.id();
    ctx.data_mut(|d| d.insert_temp(id, handle));
    tex_id
}

/// A rectangle filled with the checkerboard dither. `texel` is the size of one dither cell
/// in the coordinate space of `rect`.
pub fn dither_fill(ctx: &egui::Context, rect: Rect, texel: f32) -> Shape {
    let texel = texel.max(0.01);
    let uv = Rect::from_min_max(
        egui::pos2(0.0, 0.0),
        egui::pos2(rect.width() / (2.0 * texel), rect.height() / (2.0 * texel)),
    );
    let mut mesh = egui::Mesh::with_texture(dither_texture(ctx));
    mesh.add_rect_with_uv(rect, uv, Color32::WHITE);
    Shape::mesh(mesh)
}

/// A diamond ("gem") centered at `center`.
fn gem(center: Pos2, radius: f32, fill: Color32, outline: f32) -> Shape {
    Shape::convex_polygon(
        vec![
            center + egui::vec2(0.0, -radius),
            center + egui::vec2(radius, 0.0),
            center + egui::vec2(0.0, radius),
            center + egui::vec2(-radius, 0.0),
        ],
        fill,
        Stroke::new(outline, EDGE),
    )
}

/// Shapes for a full retro message window around `rect`: hard drop shadow, outline, dithered
/// fill, parchment frame, inner violet rule and palette-cycling corner gems.
///
/// `unit` scales line widths (1.0 ≈ the 380px mock, 2.0 for the large over-video popup).
/// `texel` is the dither cell size in `rect` coordinates. Pass `time = None` for static gems.
pub fn window_shapes(
    ctx: &egui::Context,
    rect: Rect,
    unit: f32,
    texel: f32,
    time: Option<f64>,
) -> Vec<Shape> {
    let frame_w = 2.0 * unit;
    let edge_w = 2.0 * unit;
    let shadow = 4.0 * unit;
    let rule_w = 1.5 * unit;
    let rule_inset = 4.5 * unit;

    let frame_rect = rect.expand(frame_w * 0.5);
    let outer = rect.expand(frame_w + edge_w);
    let mut shapes = Vec::with_capacity(12);

    // Hard drop shadow (only outside the window, like CSS box-shadow).
    shapes.push(Shape::rect_filled(
        Rect::from_min_max(
            egui::pos2(outer.max.x, outer.min.y + shadow),
            egui::pos2(outer.max.x + shadow, outer.max.y + shadow),
        ),
        0.0,
        EDGE,
    ));
    shapes.push(Shape::rect_filled(
        Rect::from_min_max(
            egui::pos2(outer.min.x + shadow, outer.max.y),
            egui::pos2(outer.max.x, outer.max.y + shadow),
        ),
        0.0,
        EDGE,
    ));
    // Dithered translucent fill.
    shapes.push(dither_fill(ctx, rect, texel));
    // Dark outline + parchment frame.
    shapes.push(Shape::rect_stroke(
        rect.expand(frame_w + edge_w * 0.5),
        0.0,
        Stroke::new(edge_w, EDGE),
    ));
    shapes.push(Shape::rect_stroke(
        frame_rect,
        0.0,
        Stroke::new(frame_w, FRAME),
    ));
    // Inner rule.
    shapes.push(Shape::rect_stroke(
        rect.shrink(rule_inset),
        0.0,
        Stroke::new(rule_w, RULE),
    ));
    // Corner gems.
    let radius = 5.5 * unit;
    let corners = [
        frame_rect.left_top(),
        frame_rect.right_top(),
        frame_rect.right_bottom(),
        frame_rect.left_bottom(),
    ];
    for (i, c) in corners.into_iter().enumerate() {
        let color = match time {
            Some(t) => gem_color(t, i),
            None => GEM_CYCLE[i % GEM_CYCLE.len()],
        };
        shapes.push(gem(c, radius, color, unit.max(1.0)));
    }
    shapes
}

/// Shapes for a lighter retro panel (used for mining bank rows): solid window fill, violet
/// rule and a hard 1-unit shadow. `accent` replaces the rule color (e.g. on hover).
pub fn panel_shapes(rect: Rect, unit: f32, accent: Option<Color32>) -> Vec<Shape> {
    let w = 2.0 * unit;
    vec![
        Shape::rect_filled(rect.translate(egui::vec2(w, w)), 0.0, EDGE),
        Shape::rect_filled(rect, 0.0, WINDOW),
        Shape::rect_stroke(
            rect.shrink(w * 0.5),
            0.0,
            Stroke::new(w, accent.unwrap_or(RULE)),
        ),
    ]
}

/// Paints a square bevel button face (PC-98 style: light top-left, dark bottom-right; inverted
/// when `pressed`).
pub fn paint_bevel(painter: &egui::Painter, rect: Rect, fill: Color32, pressed: bool, unit: f32) {
    let w = 2.0 * unit;
    let (tl, br) = if pressed {
        (BEVEL_LO, BEVEL_HI)
    } else {
        (BEVEL_HI, BEVEL_LO)
    };
    painter.rect_stroke(rect.expand(w * 0.5), 0.0, Stroke::new(unit.max(1.0), EDGE));
    painter.rect_filled(rect, 0.0, fill);
    let r = rect.shrink(w * 0.5);
    let tl_stroke = Stroke::new(w, tl);
    let br_stroke = Stroke::new(w, br);
    painter.line_segment([r.left_bottom(), r.left_top()], tl_stroke);
    painter.line_segment([r.left_top(), r.right_top()], tl_stroke);
    painter.line_segment([r.right_top(), r.right_bottom()], br_stroke);
    painter.line_segment([r.right_bottom(), r.left_bottom()], br_stroke);
}

/// Paints text with a hard drop shadow (like 400-line bitmap kanji).
pub fn paint_shadow_text(
    painter: &egui::Painter,
    pos: Pos2,
    anchor: egui::Align2,
    text: &str,
    font: egui::FontId,
    color: Color32,
    shadow: f32,
) {
    painter.text(
        pos + egui::vec2(shadow, shadow),
        anchor,
        text,
        font.clone(),
        EDGE,
    );
    painter.text(pos, anchor, text, font, color);
}

/// Allocates and paints a horizontal divider: violet rule with a small gold diamond centered.
pub fn divider(ui: &mut egui::Ui, unit: f32) {
    let height = 10.0 * unit;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let y = rect.center().y;
    let gap = 9.0 * unit;
    let stroke = Stroke::new(1.5 * unit, RULE);
    let painter = ui.painter();
    painter.hline(rect.left()..=rect.center().x - gap, y, stroke);
    painter.hline(rect.center().x + gap..=rect.right(), y, stroke);
    painter.add(gem(rect.center(), 4.0 * unit, GOLD, (0.75 * unit).max(1.0)));
}

/// Applies retro scrollbar / widget colors to `ui` (scoped to that ui).
pub fn apply_widget_visuals(ui: &mut egui::Ui) {
    let visuals = &mut ui.style_mut().visuals;
    visuals.extreme_bg_color = BACKDROP;
    let w = &mut visuals.widgets;
    for (state, fill, fg) in [
        (&mut w.inactive, RULE, TEXT),
        (&mut w.hovered, MUTED, HEAD),
        (&mut w.active, GOLD, HEAD),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = TAG_BG;
        state.bg_stroke = Stroke::new(1.5, fill);
        state.fg_stroke.color = fg;
        state.rounding = egui::Rounding::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gem_cycle_steps_through_palette() {
        assert_eq!(gem_color(0.0, 0), GEM_CYCLE[0]);
        assert_eq!(gem_color(GEM_STEP_SECS * 1.01, 0), GEM_CYCLE[1]);
        assert_eq!(gem_color(0.0, 3), GEM_CYCLE[3]);
        assert_eq!(gem_color(GEM_STEP_SECS * 4.01, 0), GEM_CYCLE[0]);
    }

    #[test]
    fn cursor_blinks_half_the_period() {
        assert!(blink_on(0.1));
        assert!(!blink_on(0.6));
        assert!(blink_on(1.1));
    }

    #[test]
    fn apply_widget_visuals_keeps_button_text_contrasted_against_background() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                apply_widget_visuals(ui);
                let w = &ui.style().visuals.widgets;
                for state in [&w.inactive, &w.hovered, &w.active] {
                    assert_ne!(state.fg_stroke.color, state.weak_bg_fill);
                    assert_ne!(state.fg_stroke.color, state.bg_fill);
                }
            });
        });
    }
}
