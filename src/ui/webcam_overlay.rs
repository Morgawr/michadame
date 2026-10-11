use crate::app::{models::WebcamResizeCorner, AppState};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};

const PC98_BEZEL_BG: Color32 = Color32::from_rgb(32, 29, 26); // #201d1a deep charcoal umber
const PC98_OUTER_HIGHLIGHT: Color32 = Color32::from_rgb(92, 85, 76); // #5c554c warm antique pewter
const PC98_OUTER_SHADOW: Color32 = Color32::from_rgb(17, 16, 14); // #11100e deep shadow
const PC98_MID_HIGHLIGHT: Color32 = Color32::from_rgb(61, 55, 48); // #3d3730
const PC98_MID_SHADOW: Color32 = Color32::from_rgb(25, 23, 21); // #191715
const PC98_INNER_SHADOW: Color32 = Color32::from_rgb(14, 13, 12); // #0e0d0c inset groove
const PC98_INNER_HIGHLIGHT: Color32 = Color32::from_rgb(66, 60, 53); // #423c35
const PC98_BRACKET_COLOR: Color32 = Color32::from_rgb(74, 67, 58); // #4a433a vintage bronze slate
const PC98_BRACKET_ACTIVE: Color32 = Color32::from_rgb(130, 118, 102); // #827666 illuminated bracket
const PC98_STANDBY_BG: Color32 = Color32::from_rgb(20, 18, 17); // #141211
const PC98_STANDBY_TEXT: Color32 = Color32::from_rgb(90, 82, 71); // #5a5247

pub const FRAME_PADDING: f32 = 8.0;
pub const MIN_CONTENT_WIDTH: f32 = 64.0;
pub const CORNER_HIT_SIZE: f32 = 18.0;

/// Computes effective image dimensions, aspect ratio, and texture UV mapping
/// after applying cutout margin coordinates. Clamps cutout values to maintain
/// at least 16 pixels of image area.
pub fn calculate_effective_dimensions(
    raw_w: u32,
    raw_h: u32,
    cut: &crate::config::models::WebcamCutout,
) -> (u32, u32, f32, Rect) {
    let max_cut_x = raw_w.saturating_sub(16);
    let max_cut_y = raw_h.saturating_sub(16);

    let cut_l = cut.left.min(max_cut_x);
    let cut_r = cut.right.min(max_cut_x.saturating_sub(cut_l));
    let cut_t = cut.top.min(max_cut_y);
    let cut_b = cut.bottom.min(max_cut_y.saturating_sub(cut_t));

    let eff_w = raw_w.saturating_sub(cut_l + cut_r).max(1);
    let eff_h = raw_h.saturating_sub(cut_t + cut_b).max(1);
    let aspect_ratio = eff_w as f32 / eff_h as f32;

    let uv_min = Pos2::new(cut_l as f32 / raw_w as f32, cut_t as f32 / raw_h as f32);
    let uv_max = Pos2::new(
        1.0 - (cut_r as f32 / raw_w as f32),
        1.0 - (cut_b as f32 / raw_h as f32),
    );
    let uv = Rect::from_min_max(uv_min, uv_max);

    (eff_w, eff_h, aspect_ratio, uv)
}

/// Calculates new normalized width and position when dragging one of the 4 corner handles.
/// Preserves the exact aspect ratio with zero stretching, anchoring the opposite corner.
pub fn calculate_resized_geometry(
    corner: WebcamResizeCorner,
    current_p: Pos2,
    outer_rect: Rect,
    viewport_rect: Rect,
    aspect_ratio: f32,
) -> Option<(f32, Option<f32>, Option<f32>)> {
    let viewport_w = viewport_rect.width();
    let viewport_h = viewport_rect.height();

    match corner {
        WebcamResizeCorner::BottomRight => {
            let new_total_w = (current_p.x - outer_rect.left())
                .clamp(MIN_CONTENT_WIDTH + FRAME_PADDING * 2.0, viewport_w - (outer_rect.left() - viewport_rect.min.x));
            let new_content_w = new_total_w - FRAME_PADDING * 2.0;
            let new_content_h = new_content_w / aspect_ratio;
            let new_total_h = new_content_h + FRAME_PADDING * 2.0;

            if outer_rect.top() + new_total_h <= viewport_rect.max.y {
                Some((new_content_w / viewport_w, None, None))
            } else {
                None
            }
        }
        WebcamResizeCorner::TopLeft => {
            let new_total_w = (outer_rect.right() - current_p.x)
                .clamp(MIN_CONTENT_WIDTH + FRAME_PADDING * 2.0, outer_rect.right() - viewport_rect.min.x);
            let new_content_w = new_total_w - FRAME_PADDING * 2.0;
            let new_content_h = new_content_w / aspect_ratio;
            let new_total_h = new_content_h + FRAME_PADDING * 2.0;

            let new_top = outer_rect.bottom() - new_total_h;
            if new_top >= viewport_rect.min.y {
                let new_left = outer_rect.right() - new_total_w;
                Some((
                    new_content_w / viewport_w,
                    Some((new_left - viewport_rect.min.x) / viewport_w),
                    Some((new_top - viewport_rect.min.y) / viewport_h),
                ))
            } else {
                None
            }
        }
        WebcamResizeCorner::TopRight => {
            let new_total_w = (current_p.x - outer_rect.left())
                .clamp(MIN_CONTENT_WIDTH + FRAME_PADDING * 2.0, viewport_rect.max.x - outer_rect.left());
            let new_content_w = new_total_w - FRAME_PADDING * 2.0;
            let new_content_h = new_content_w / aspect_ratio;
            let new_total_h = new_content_h + FRAME_PADDING * 2.0;

            let new_top = outer_rect.bottom() - new_total_h;
            if new_top >= viewport_rect.min.y {
                Some((
                    new_content_w / viewport_w,
                    None,
                    Some((new_top - viewport_rect.min.y) / viewport_h),
                ))
            } else {
                None
            }
        }
        WebcamResizeCorner::BottomLeft => {
            let new_total_w = (outer_rect.right() - current_p.x)
                .clamp(MIN_CONTENT_WIDTH + FRAME_PADDING * 2.0, outer_rect.right() - viewport_rect.min.x);
            let new_content_w = new_total_w - FRAME_PADDING * 2.0;
            let new_content_h = new_content_w / aspect_ratio;
            let new_total_h = new_content_h + FRAME_PADDING * 2.0;

            if outer_rect.top() + new_total_h <= viewport_rect.max.y {
                let new_left = outer_rect.right() - new_total_w;
                Some((
                    new_content_w / viewport_w,
                    Some((new_left - viewport_rect.min.x) / viewport_w),
                    None,
                ))
            } else {
                None
            }
        }
    }
}

pub fn draw_webcam_overlay(
    state: &mut AppState,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    viewport_rect: Rect,
) {
    if !state.webcam.config.enabled {
        return;
    }

    // 1. Drain latest received frames
    if let Some(rx) = &state.webcam.frame_receiver {
        while let Ok(frame) = rx.try_recv() {
            state.webcam.latest_frame = Some(frame);
        }
    }

    // 2. Upload/update egui texture handle
    if let Some(frame) = &state.webcam.latest_frame {
        let color_image = egui::ColorImage::from_rgba_unmultiplied(
            [frame.width as usize, frame.height as usize],
            &frame.rgba,
        );
        if let Some(tex) = &mut state.webcam.texture {
            tex.set(color_image, egui::TextureOptions::LINEAR);
        } else {
            state.webcam.texture = Some(ctx.load_texture(
                "webcam_texture",
                color_image,
                egui::TextureOptions::LINEAR,
            ));
        }
    }

    // 3. Compute effective dimensions and aspect ratio from cutout coordinates
    let (raw_w, raw_h) = state
        .webcam
        .latest_frame
        .as_ref()
        .map(|f| (f.width, f.height))
        .or(state.webcam.config.resolution)
        .unwrap_or((640, 480));

    let (_eff_w, _eff_h, aspect_ratio, uv) =
        calculate_effective_dimensions(raw_w, raw_h, &state.webcam.config.cutout);

    // 4. Determine screen bounds
    let viewport_w = viewport_rect.width();
    let viewport_h = viewport_rect.height();

    let target_content_w = (viewport_w * state.webcam.config.width_pct)
        .clamp(MIN_CONTENT_WIDTH, (viewport_w - FRAME_PADDING * 2.0).max(MIN_CONTENT_WIDTH));
    let target_content_h = target_content_w / aspect_ratio;

    let total_w = target_content_w + FRAME_PADDING * 2.0;
    let total_h = target_content_h + FRAME_PADDING * 2.0;

    let mut min_x = viewport_rect.min.x + viewport_w * state.webcam.config.pos_x_pct;
    let mut min_y = viewport_rect.min.y + viewport_h * state.webcam.config.pos_y_pct;

    min_x = min_x.clamp(viewport_rect.min.x, (viewport_rect.max.x - total_w).max(viewport_rect.min.x));
    min_y = min_y.clamp(viewport_rect.min.y, (viewport_rect.max.y - total_h).max(viewport_rect.min.y));

    let outer_rect = Rect::from_min_size(egui::pos2(min_x, min_y), Vec2::new(total_w, total_h));
    let inner_rect = outer_rect.shrink(FRAME_PADDING);

    // 5. Hit testing and mouse interaction
    let pointer_pos = ui.input(|i| i.pointer.hover_pos());
    let primary_down = ui.input(|i| i.pointer.primary_down());
    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());

    let nw_rect = Rect::from_min_size(outer_rect.left_top(), Vec2::splat(CORNER_HIT_SIZE));
    let ne_rect = Rect::from_min_size(
        egui::pos2(outer_rect.right() - CORNER_HIT_SIZE, outer_rect.top()),
        Vec2::splat(CORNER_HIT_SIZE),
    );
    let sw_rect = Rect::from_min_size(
        egui::pos2(outer_rect.left(), outer_rect.bottom() - CORNER_HIT_SIZE),
        Vec2::splat(CORNER_HIT_SIZE),
    );
    let se_rect = Rect::from_min_size(
        egui::pos2(outer_rect.right() - CORNER_HIT_SIZE, outer_rect.bottom() - CORNER_HIT_SIZE),
        Vec2::splat(CORNER_HIT_SIZE),
    );

    let mut hovered_corner = None;
    if let Some(pos) = pointer_pos {
        if nw_rect.contains(pos) {
            hovered_corner = Some(WebcamResizeCorner::TopLeft);
        } else if ne_rect.contains(pos) {
            hovered_corner = Some(WebcamResizeCorner::TopRight);
        } else if sw_rect.contains(pos) {
            hovered_corner = Some(WebcamResizeCorner::BottomLeft);
        } else if se_rect.contains(pos) {
            hovered_corner = Some(WebcamResizeCorner::BottomRight);
        }
    }

    let is_body_hovered = pointer_pos.is_some_and(|pos| outer_rect.contains(pos))
        && hovered_corner.is_none()
        && state.webcam.resizing_corner.is_none();

    if primary_pressed {
        if let Some(corner) = hovered_corner {
            state.webcam.resizing_corner = Some(corner);
            state.webcam.resize_start_pointer = pointer_pos;
            state.webcam.resize_start_width_pct = Some(state.webcam.config.width_pct);
            state.webcam.resize_start_pos = Some([state.webcam.config.pos_x_pct, state.webcam.config.pos_y_pct]);
            state.webcam.is_dragging = false;
        } else if is_body_hovered {
            state.webcam.is_dragging = true;
            state.webcam.drag_start_pointer = pointer_pos;
            state.webcam.drag_start_pos = Some([state.webcam.config.pos_x_pct, state.webcam.config.pos_y_pct]);
            state.webcam.resizing_corner = None;
        }
    }

    let mut config_changed = false;

    if primary_down {
        if let (Some(corner), Some(current_p)) = (state.webcam.resizing_corner, pointer_pos) {
            match corner {
                WebcamResizeCorner::BottomRight | WebcamResizeCorner::TopLeft => {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
                }
                WebcamResizeCorner::TopRight | WebcamResizeCorner::BottomLeft => {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNeSw);
                }
            }
            if let Some((new_width_pct, new_pos_x, new_pos_y)) =
                calculate_resized_geometry(corner, current_p, outer_rect, viewport_rect, aspect_ratio)
            {
                state.webcam.config.width_pct = new_width_pct;
                if let Some(pos_x) = new_pos_x {
                    state.webcam.config.pos_x_pct = pos_x;
                }
                if let Some(pos_y) = new_pos_y {
                    state.webcam.config.pos_y_pct = pos_y;
                }
                config_changed = true;
            }
        } else if state.webcam.is_dragging {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if let (Some(start_p), Some(start_pos), Some(current_p)) = (
                state.webcam.drag_start_pointer,
                state.webcam.drag_start_pos,
                pointer_pos,
            ) {
                let delta = current_p - start_p;
                let start_min_x = viewport_rect.min.x + viewport_w * start_pos[0];
                let start_min_y = viewport_rect.min.y + viewport_h * start_pos[1];

                let new_min_x = (start_min_x + delta.x)
                    .clamp(viewport_rect.min.x, (viewport_rect.max.x - total_w).max(viewport_rect.min.x));
                let new_min_y = (start_min_y + delta.y)
                    .clamp(viewport_rect.min.y, (viewport_rect.max.y - total_h).max(viewport_rect.min.y));

                state.webcam.config.pos_x_pct = (new_min_x - viewport_rect.min.x) / viewport_w;
                state.webcam.config.pos_y_pct = (new_min_y - viewport_rect.min.y) / viewport_h;
                config_changed = true;
            }
        }
    } else {
        if state.webcam.is_dragging || state.webcam.resizing_corner.is_some() {
            state.webcam.is_dragging = false;
            state.webcam.resizing_corner = None;
            state.webcam.drag_start_pointer = None;
            state.webcam.drag_start_pos = None;
            state.webcam.resize_start_pointer = None;
            state.webcam.resize_start_width_pct = None;
            state.webcam.resize_start_pos = None;
            crate::config::save_config(state);
            crate::config::save_global_hardware_config(state);
        }

        if let Some(corner) = hovered_corner {
            match corner {
                WebcamResizeCorner::TopLeft | WebcamResizeCorner::BottomRight => {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
                }
                WebcamResizeCorner::TopRight | WebcamResizeCorner::BottomLeft => {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNeSw);
                }
            }
        } else if is_body_hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
    }

    if config_changed {
        ctx.request_repaint();
    }

    // 6. Draw PC-98 somber visual novel border
    let painter = ui.painter();

    // Base chassis matte background
    painter.rect_filled(outer_rect, egui::Rounding::ZERO, PC98_BEZEL_BG);

    // Outer chiseled contour layer 1 (outermost rim)
    painter.line_segment([outer_rect.left_top(), outer_rect.right_top()], Stroke::new(1.0, PC98_OUTER_HIGHLIGHT));
    painter.line_segment([outer_rect.left_top(), outer_rect.left_bottom()], Stroke::new(1.0, PC98_OUTER_HIGHLIGHT));
    painter.line_segment([outer_rect.left_bottom(), outer_rect.right_bottom()], Stroke::new(1.0, PC98_OUTER_SHADOW));
    painter.line_segment([outer_rect.right_top(), outer_rect.right_bottom()], Stroke::new(1.0, PC98_OUTER_SHADOW));

    // Outer chiseled contour layer 2 (inset line)
    let mid_rect = outer_rect.shrink(1.0);
    painter.line_segment([mid_rect.left_top(), mid_rect.right_top()], Stroke::new(1.0, PC98_MID_HIGHLIGHT));
    painter.line_segment([mid_rect.left_top(), mid_rect.left_bottom()], Stroke::new(1.0, PC98_MID_HIGHLIGHT));
    painter.line_segment([mid_rect.left_bottom(), mid_rect.right_bottom()], Stroke::new(1.0, PC98_MID_SHADOW));
    painter.line_segment([mid_rect.right_top(), mid_rect.right_bottom()], Stroke::new(1.0, PC98_MID_SHADOW));

    // Corner vintage brackets (L-shaped decorative accents at each corner)
    let bracket_len = 9.0;
    let bracket_thick = 2.0;

    let draw_corner_bracket = |painter: &egui::Painter, corner: Pos2, dir_x: f32, dir_y: f32, active: bool| {
        let color = if active { PC98_BRACKET_ACTIVE } else { PC98_BRACKET_COLOR };
        let stroke = Stroke::new(bracket_thick, color);
        let h_end = Pos2::new(corner.x + dir_x * bracket_len, corner.y);
        let v_end = Pos2::new(corner.x, corner.y + dir_y * bracket_len);
        painter.line_segment([corner, h_end], stroke);
        painter.line_segment([corner, v_end], stroke);
    };

    let b_offset = 2.0;
    draw_corner_bracket(
        painter,
        Pos2::new(outer_rect.left() + b_offset, outer_rect.top() + b_offset),
        1.0,
        1.0,
        hovered_corner == Some(WebcamResizeCorner::TopLeft) || state.webcam.resizing_corner == Some(WebcamResizeCorner::TopLeft),
    );
    draw_corner_bracket(
        painter,
        Pos2::new(outer_rect.right() - b_offset, outer_rect.top() + b_offset),
        -1.0,
        1.0,
        hovered_corner == Some(WebcamResizeCorner::TopRight) || state.webcam.resizing_corner == Some(WebcamResizeCorner::TopRight),
    );
    draw_corner_bracket(
        painter,
        Pos2::new(outer_rect.left() + b_offset, outer_rect.bottom() - b_offset),
        1.0,
        -1.0,
        hovered_corner == Some(WebcamResizeCorner::BottomLeft) || state.webcam.resizing_corner == Some(WebcamResizeCorner::BottomLeft),
    );
    draw_corner_bracket(
        painter,
        Pos2::new(outer_rect.right() - b_offset, outer_rect.bottom() - b_offset),
        -1.0,
        -1.0,
        hovered_corner == Some(WebcamResizeCorner::BottomRight) || state.webcam.resizing_corner == Some(WebcamResizeCorner::BottomRight),
    );

    // Inner recessed bezel groove framing the video viewport
    let inner_recess = inner_rect.expand(1.0);
    painter.line_segment([inner_recess.left_top(), inner_recess.right_top()], Stroke::new(1.0, PC98_INNER_SHADOW));
    painter.line_segment([inner_recess.left_top(), inner_recess.left_bottom()], Stroke::new(1.0, PC98_INNER_SHADOW));
    painter.line_segment([inner_recess.left_bottom(), inner_recess.right_bottom()], Stroke::new(1.0, PC98_INNER_HIGHLIGHT));
    painter.line_segment([inner_recess.right_top(), inner_recess.right_bottom()], Stroke::new(1.0, PC98_INNER_HIGHLIGHT));

    // 7. Video image / Standby screen
    if let Some(texture) = &state.webcam.texture {
        painter.image(texture.id(), inner_rect, uv, Color32::WHITE);
    } else {
        // Standby screen
        painter.rect_filled(inner_rect, egui::Rounding::ZERO, PC98_STANDBY_BG);

        // Fine vintage raster stripes
        let step = 4.0;
        let mut y = inner_rect.top() + step;
        while y < inner_rect.bottom() {
            painter.line_segment(
                [Pos2::new(inner_rect.left(), y), Pos2::new(inner_rect.right(), y)],
                Stroke::new(1.0, Color32::from_rgb(26, 24, 22)),
            );
            y += step;
        }

        // Standby text label
        painter.text(
            inner_rect.center(),
            egui::Align2::CENTER_CENTER,
            "WEBCAM STANDBY",
            egui::FontId::monospace(10.0),
            PC98_STANDBY_TEXT,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::models::WebcamCutout;

    #[test]
    fn test_calculate_effective_dimensions_default() {
        let cut = WebcamCutout::default();
        let (eff_w, eff_h, aspect, uv) = calculate_effective_dimensions(640, 480, &cut);
        assert_eq!(eff_w, 640);
        assert_eq!(eff_h, 480);
        assert!((aspect - (640.0 / 480.0)).abs() < 1e-5);
        assert_eq!(uv.min, Pos2::new(0.0, 0.0));
        assert_eq!(uv.max, Pos2::new(1.0, 1.0));
    }

    #[test]
    fn test_calculate_effective_dimensions_with_shaved_margins() {
        let cut = WebcamCutout {
            left: 40,
            right: 60,
            top: 20,
            bottom: 30,
        };
        let (eff_w, eff_h, aspect, uv) = calculate_effective_dimensions(640, 480, &cut);
        assert_eq!(eff_w, 540);
        assert_eq!(eff_h, 430);
        assert!((aspect - (540.0 / 430.0)).abs() < 1e-5);
        assert!((uv.min.x - 40.0 / 640.0).abs() < 1e-5);
        assert!((uv.min.y - 20.0 / 480.0).abs() < 1e-5);
        assert!((uv.max.x - (1.0 - 60.0 / 640.0)).abs() < 1e-5);
        assert!((uv.max.y - (1.0 - 30.0 / 480.0)).abs() < 1e-5);
    }

    #[test]
    fn test_calculate_effective_dimensions_extreme_cutout_clamping() {
        let cut = WebcamCutout {
            left: 1000,
            right: 1000,
            top: 1000,
            bottom: 1000,
        };
        let (eff_w, eff_h, aspect, _) = calculate_effective_dimensions(640, 480, &cut);
        assert!(eff_w >= 16);
        assert!(eff_h >= 16);
        assert!(aspect > 0.0);
    }

    #[test]
    fn test_aspect_ratio_preserving_corner_resize_bottom_right() {
        let aspect_ratio = 16.0 / 9.0;
        let viewport = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(1920.0, 1080.0));
        let outer = Rect::from_min_size(Pos2::new(100.0, 100.0), Vec2::new(320.0, 180.0));

        let current_p = Pos2::new(500.0, 400.0);
        let res = calculate_resized_geometry(
            WebcamResizeCorner::BottomRight,
            current_p,
            outer,
            viewport,
            aspect_ratio,
        );
        assert!(res.is_some());
        let (new_width_pct, new_x, new_y) = res.unwrap();
        assert!(new_x.is_none());
        assert!(new_y.is_none());

        let new_content_w = new_width_pct * viewport.width();
        let new_content_h = new_content_w / aspect_ratio;
        assert!((new_content_w / new_content_h - aspect_ratio).abs() < 1e-4);
    }

    #[test]
    fn test_aspect_ratio_preserving_corner_resize_top_left() {
        let aspect_ratio = 4.0 / 3.0;
        let viewport = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(1920.0, 1080.0));
        let outer = Rect::from_min_size(Pos2::new(400.0, 300.0), Vec2::new(320.0, 240.0));

        let current_p = Pos2::new(200.0, 150.0);
        let res = calculate_resized_geometry(
            WebcamResizeCorner::TopLeft,
            current_p,
            outer,
            viewport,
            aspect_ratio,
        );
        assert!(res.is_some());
        let (new_width_pct, new_x, new_y) = res.unwrap();
        assert!(new_x.is_some());
        assert!(new_y.is_some());

        let new_content_w = new_width_pct * viewport.width();
        let new_content_h = new_content_w / aspect_ratio;
        assert!((new_content_w / new_content_h - aspect_ratio).abs() < 1e-4);

        // Anchored bottom-right: outer.right() and outer.bottom() must match new bounds
        let new_total_w = new_content_w + FRAME_PADDING * 2.0;
        let new_total_h = new_content_h + FRAME_PADDING * 2.0;
        let new_left = new_x.unwrap() * viewport.width();
        let new_top = new_y.unwrap() * viewport.height();
        assert!((new_left + new_total_w - outer.right()).abs() < 1e-4);
        assert!((new_top + new_total_h - outer.bottom()).abs() < 1e-4);
    }

    #[test]
    fn test_aspect_ratio_preserving_corner_resize_top_right() {
        let aspect_ratio = 16.0 / 9.0;
        let viewport = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(1920.0, 1080.0));
        let outer = Rect::from_min_size(Pos2::new(200.0, 300.0), Vec2::new(320.0, 180.0));

        let current_p = Pos2::new(600.0, 200.0);
        let res = calculate_resized_geometry(
            WebcamResizeCorner::TopRight,
            current_p,
            outer,
            viewport,
            aspect_ratio,
        );
        assert!(res.is_some());
        let (new_width_pct, new_x, new_y) = res.unwrap();
        assert!(new_x.is_none()); // left is anchored
        assert!(new_y.is_some()); // top moves up

        let new_content_w = new_width_pct * viewport.width();
        let new_content_h = new_content_w / aspect_ratio;
        assert!((new_content_w / new_content_h - aspect_ratio).abs() < 1e-4);

        // Anchored bottom-left
        let new_total_h = new_content_h + FRAME_PADDING * 2.0;
        let new_top = new_y.unwrap() * viewport.height();
        assert!((new_top + new_total_h - outer.bottom()).abs() < 1e-4);
    }

    #[test]
    fn test_aspect_ratio_preserving_corner_resize_bottom_left() {
        let aspect_ratio = 4.0 / 3.0;
        let viewport = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(1920.0, 1080.0));
        let outer = Rect::from_min_size(Pos2::new(500.0, 200.0), Vec2::new(320.0, 240.0));

        let current_p = Pos2::new(300.0, 450.0);
        let res = calculate_resized_geometry(
            WebcamResizeCorner::BottomLeft,
            current_p,
            outer,
            viewport,
            aspect_ratio,
        );
        assert!(res.is_some());
        let (new_width_pct, new_x, new_y) = res.unwrap();
        assert!(new_x.is_some()); // left moves left
        assert!(new_y.is_none()); // top is anchored

        let new_content_w = new_width_pct * viewport.width();
        let new_content_h = new_content_w / aspect_ratio;
        assert!((new_content_w / new_content_h - aspect_ratio).abs() < 1e-4);

        // Anchored top-right
        let new_total_w = new_content_w + FRAME_PADDING * 2.0;
        let new_left = new_x.unwrap() * viewport.width();
        assert!((new_left + new_total_w - outer.right()).abs() < 1e-4);
    }

    #[test]
    fn test_no_emojis_in_webcam_labels_and_strings() {
        let text = "WEBCAM STANDBY";
        assert!(text.chars().all(|c| c.is_ascii()));
    }
}
