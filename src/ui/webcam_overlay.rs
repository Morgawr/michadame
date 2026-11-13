use crate::app::{models::WebcamResizeCorner, AppState};
use crate::dict::theme;
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use std::sync::{atomic::Ordering, Arc};

// "Faded Indigo" retro chrome, shared with the dictionary popup and mining bank
// (see `dict::theme`). Everything is drawn inside the `FRAME_PADDING` band:
//   [0.0, 1.5) dark outline · [1.5, 3.5) parchment frame · [3.5, 5.5) window fill ·
//   [5.5, 7.0) violet rule · [7.0, 8.0) recessed edge around the picture.
const EDGE_W: f32 = 1.5;
const FRAME_W: f32 = 2.0;
const RULE_W: f32 = 1.5;
const RULE_INSET: f32 = 5.5;
const GEM_RADIUS: f32 = 5.0;
const GEM_RADIUS_ACTIVE: f32 = 6.5;

pub const FRAME_PADDING: f32 = 8.0;
pub const MIN_CONTENT_WIDTH: f32 = 64.0;
pub const CORNER_HIT_SIZE: f32 = 18.0;
/// Label shown in the frame while no webcam picture has arrived yet.
const STANDBY_LABEL: &str = "WEBCAM STANDBY";
/// Label shown while a VTuber `.vrm` model is loading.
const AVATAR_STANDBY_LABEL: &str = "AVATAR STANDBY";
/// Label shown when VTuber mode is enabled before a `.vrm` file is chosen.
const AVATAR_NO_MODEL_LABEL: &str = "SELECT VRM AVATAR";

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

/// Calculates new normalized width and position when dragging one of the 4 corner handles
/// for a framed overlay (`FRAME_PADDING`).
/// Preserves the exact aspect ratio with zero stretching, anchoring the opposite corner.
pub fn calculate_resized_geometry(
    corner: WebcamResizeCorner,
    current_p: Pos2,
    outer_rect: Rect,
    viewport_rect: Rect,
    aspect_ratio: f32,
) -> Option<(f32, Option<f32>, Option<f32>)> {
    calculate_resized_geometry_with_padding(
        corner,
        current_p,
        outer_rect,
        viewport_rect,
        aspect_ratio,
        FRAME_PADDING,
    )
}

/// Calculates new normalized width and position when dragging one of the 4 corner handles
/// with an explicit frame padding (`FRAME_PADDING` for framed webcam, `0.0` for borderless cutout).
pub fn calculate_resized_geometry_with_padding(
    corner: WebcamResizeCorner,
    current_p: Pos2,
    outer_rect: Rect,
    viewport_rect: Rect,
    aspect_ratio: f32,
    padding: f32,
) -> Option<(f32, Option<f32>, Option<f32>)> {
    let viewport_w = viewport_rect.width();
    let viewport_h = viewport_rect.height();
    let pad2 = padding * 2.0;

    match corner {
        WebcamResizeCorner::BottomRight => {
            let new_total_w = (current_p.x - outer_rect.left()).clamp(
                MIN_CONTENT_WIDTH + pad2,
                viewport_w - (outer_rect.left() - viewport_rect.min.x),
            );
            let new_content_w = new_total_w - pad2;
            let new_content_h = new_content_w / aspect_ratio;
            let new_total_h = new_content_h + pad2;

            if outer_rect.top() + new_total_h <= viewport_rect.max.y {
                Some((new_content_w / viewport_w, None, None))
            } else {
                None
            }
        }
        WebcamResizeCorner::TopLeft => {
            let new_total_w = (outer_rect.right() - current_p.x).clamp(
                MIN_CONTENT_WIDTH + pad2,
                outer_rect.right() - viewport_rect.min.x,
            );
            let new_content_w = new_total_w - pad2;
            let new_content_h = new_content_w / aspect_ratio;
            let new_total_h = new_content_h + pad2;

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
            let new_total_w = (current_p.x - outer_rect.left()).clamp(
                MIN_CONTENT_WIDTH + pad2,
                viewport_rect.max.x - outer_rect.left(),
            );
            let new_content_w = new_total_w - pad2;
            let new_content_h = new_content_w / aspect_ratio;
            let new_total_h = new_content_h + pad2;

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
            let new_total_w = (outer_rect.right() - current_p.x).clamp(
                MIN_CONTENT_WIDTH + pad2,
                outer_rect.right() - viewport_rect.min.x,
            );
            let new_content_w = new_total_w - pad2;
            let new_content_h = new_content_w / aspect_ratio;
            let new_total_h = new_content_h + pad2;

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

fn paint_retro_window_frame(
    painter: &egui::Painter,
    outer_rect: Rect,
    inner_rect: Rect,
    time: f64,
    hovered_corner: Option<WebcamResizeCorner>,
    resizing_corner: Option<WebcamResizeCorner>,
) {
    // Fill only the 8px frame ring around `inner_rect` so `VtuberBackgroundKind::Transparent`
    // inside `FramedWebcam` preserves genuine alpha transparency over the underlying video!
    let top_band = Rect::from_min_max(
        outer_rect.min,
        Pos2::new(outer_rect.max.x, inner_rect.min.y),
    );
    let bottom_band = Rect::from_min_max(
        Pos2::new(outer_rect.min.x, inner_rect.max.y),
        outer_rect.max,
    );
    let left_band = Rect::from_min_max(
        Pos2::new(outer_rect.min.x, inner_rect.min.y),
        Pos2::new(inner_rect.min.x, inner_rect.max.y),
    );
    let right_band = Rect::from_min_max(
        Pos2::new(inner_rect.max.x, inner_rect.min.y),
        Pos2::new(outer_rect.max.x, inner_rect.max.y),
    );
    for band in [top_band, bottom_band, left_band, right_band] {
        painter.rect_filled(band, egui::Rounding::ZERO, theme::WINDOW);
    }

    painter.rect_stroke(
        outer_rect.shrink(EDGE_W * 0.5),
        egui::Rounding::ZERO,
        Stroke::new(EDGE_W, theme::EDGE),
    );
    let frame_rect = outer_rect.shrink(EDGE_W + FRAME_W * 0.5);
    painter.rect_stroke(
        frame_rect,
        egui::Rounding::ZERO,
        Stroke::new(FRAME_W, theme::FRAME),
    );

    // Inner violet rule, then a recessed dark edge framing the picture
    painter.rect_stroke(
        outer_rect.shrink(RULE_INSET + RULE_W * 0.5),
        egui::Rounding::ZERO,
        Stroke::new(RULE_W, theme::RULE),
    );
    painter.rect_stroke(
        inner_rect.expand(0.5),
        egui::Rounding::ZERO,
        Stroke::new(1.0, theme::EDGE),
    );

    paint_corner_gems(painter, frame_rect, time, hovered_corner, resizing_corner);
}

fn paint_corner_gems(
    painter: &egui::Painter,
    gem_rect: Rect,
    time: f64,
    hovered_corner: Option<WebcamResizeCorner>,
    resizing_corner: Option<WebcamResizeCorner>,
) {
    let corners = [
        (WebcamResizeCorner::TopLeft, gem_rect.left_top()),
        (WebcamResizeCorner::TopRight, gem_rect.right_top()),
        (WebcamResizeCorner::BottomRight, gem_rect.right_bottom()),
        (WebcamResizeCorner::BottomLeft, gem_rect.left_bottom()),
    ];
    for (i, (corner, center)) in corners.into_iter().enumerate() {
        let active = hovered_corner == Some(corner) || resizing_corner == Some(corner);
        let (radius, fill, outline) = if active {
            (GEM_RADIUS_ACTIVE, theme::GOLD, theme::HEAD)
        } else {
            (GEM_RADIUS, theme::gem_color(time, i), theme::EDGE)
        };
        painter.add(egui::Shape::convex_polygon(
            vec![
                center + Vec2::new(0.0, -radius),
                center + Vec2::new(radius, 0.0),
                center + Vec2::new(0.0, radius),
                center + Vec2::new(-radius, 0.0),
            ],
            fill,
            Stroke::new(1.0, outline),
        ));
    }
}

fn paint_standby_screen(painter: &egui::Painter, rect: Rect, label: &str, time: f64) {
    painter.rect_filled(rect, egui::Rounding::ZERO, theme::BACKDROP);
    let step = 4.0;
    let mut y = rect.top() + step;
    while y < rect.bottom() {
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            Stroke::new(1.0, theme::WINDOW),
        );
        y += step;
    }

    let font = egui::FontId::monospace(12.0);
    theme::paint_shadow_text(
        painter,
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        font,
        theme::MUTED,
        1.0,
    );
    if theme::blink_on(time) {
        theme::paint_shadow_text(
            painter,
            rect.center() + Vec2::new(0.0, 16.0),
            egui::Align2::CENTER_CENTER,
            "▼",
            crate::dict::render::dict_font(12.0),
            theme::GOLD,
            1.0,
        );
    }
}

pub fn draw_webcam_overlay(
    state: &mut AppState,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    viewport_rect: Rect,
) {
    if !state.webcam.config.enabled {
        state.webcam.last_outer_rect = None;
        return;
    }

    let is_vtuber =
        state.webcam.config.source_mode == crate::config::WebcamSourceMode::VtuberAvatar;
    let is_cutout = is_vtuber
        && state.webcam.config.vtuber.display_style
            == crate::config::VtuberDisplayStyle::BorderlessCutout;
    let effective_padding = if is_cutout { 0.0 } else { FRAME_PADDING };

    if !is_vtuber {
        // 1. Drain latest received frames (LocalCamera mode)
        let new_frame = state.webcam.poll_frame();

        // 2. Upload/update egui texture handle only when a new frame arrived or texture is uninitialized
        if new_frame || state.webcam.texture.is_none() {
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
        }
    }

    // 3. Compute effective dimensions and aspect ratio from active mode's layout & cutout
    let (raw_w, raw_h, active_pos_x, active_pos_y, active_width_pct, active_cutout) = if is_vtuber {
        let layout = state.webcam.config.vtuber.active_layout();
        let (rw, rh) = layout.aspect_preset.virtual_dimensions();
        (
            rw,
            rh,
            layout.pos_x_pct,
            layout.pos_y_pct,
            layout.width_pct,
            layout.cutout.clone(),
        )
    } else {
        let (rw, rh) = state
            .webcam
            .config
            .resolution
            .or_else(|| {
                state
                    .webcam
                    .latest_frame
                    .as_ref()
                    .map(|f| (f.width, f.height))
            })
            .unwrap_or((640, 480));
        (
            rw,
            rh,
            state.webcam.config.pos_x_pct,
            state.webcam.config.pos_y_pct,
            state.webcam.config.width_pct,
            state.webcam.config.cutout.clone(),
        )
    };

    let (_eff_w, _eff_h, aspect_ratio, uv) =
        calculate_effective_dimensions(raw_w, raw_h, &active_cutout);

    // 4. Determine screen bounds
    let viewport_w = viewport_rect.width();
    let viewport_h = viewport_rect.height();

    let target_content_w = (viewport_w * active_width_pct).clamp(
        MIN_CONTENT_WIDTH,
        (viewport_w - effective_padding * 2.0).max(MIN_CONTENT_WIDTH),
    );
    let target_content_h = target_content_w / aspect_ratio;

    let ppp = ctx.pixels_per_point().max(1.0);
    let uv_w = uv.width().clamp(0.1, 1.0);
    let uv_h = uv.height().clamp(0.1, 1.0);
    let desired_tex_w = ((target_content_w * ppp) / uv_w).ceil() as u32;
    let desired_tex_h = ((target_content_h * ppp) / uv_h).ceil() as u32;
    if is_vtuber {
        crate::devices::webcam::set_webcam_target_dimensions(640, 480);
    } else {
        crate::devices::webcam::set_webcam_target_dimensions(desired_tex_w, desired_tex_h);
    }

    let total_w = target_content_w + effective_padding * 2.0;
    let total_h = target_content_h + effective_padding * 2.0;

    let mut min_x = viewport_rect.min.x + viewport_w * active_pos_x;
    let mut min_y = viewport_rect.min.y + viewport_h * active_pos_y;

    min_x = min_x.clamp(
        viewport_rect.min.x,
        (viewport_rect.max.x - total_w).max(viewport_rect.min.x),
    );
    min_y = min_y.clamp(
        viewport_rect.min.y,
        (viewport_rect.max.y - total_h).max(viewport_rect.min.y),
    );

    let outer_rect = Rect::from_min_size(egui::pos2(min_x, min_y), Vec2::new(total_w, total_h));
    let inner_rect = outer_rect.shrink(effective_padding);
    state.webcam.last_outer_rect = Some(outer_rect);

    // 5. Hit testing and mouse interaction
    let pointer_pos = ui.input(|i| i.pointer.hover_pos());
    let primary_down = ui.input(|i| i.pointer.primary_down());
    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
    let pointer_occluded = pointer_pos.is_some_and(|pos| {
        state.twitch.overlay_contains(pos)
            || ctx.layer_id_at(pos).is_some_and(|l| {
                l.id != egui::Id::new("vtuber_cutout_handles")
                    && (l.order > egui::Order::Middle
                        || (l.order == egui::Order::Middle
                            && l.id != egui::Id::new("webcam_overlay")))
            })
    });

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
        egui::pos2(
            outer_rect.right() - CORNER_HIT_SIZE,
            outer_rect.bottom() - CORNER_HIT_SIZE,
        ),
        Vec2::splat(CORNER_HIT_SIZE),
    );

    let mut hovered_corner = None;
    if !pointer_occluded {
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
    }

    let is_body_hovered = !pointer_occluded
        && pointer_pos.is_some_and(|pos| outer_rect.contains(pos))
        && hovered_corner.is_none()
        && state.webcam.resizing_corner.is_none();

    if primary_pressed {
        if let Some(corner) = hovered_corner {
            state.webcam.resizing_corner = Some(corner);
            state.webcam.resize_start_pointer = pointer_pos;
            state.webcam.resize_start_width_pct = Some(active_width_pct);
            state.webcam.resize_start_pos = Some([active_pos_x, active_pos_y]);
            state.webcam.is_dragging = false;
        } else if is_body_hovered {
            state.webcam.is_dragging = true;
            state.webcam.drag_start_pointer = pointer_pos;
            state.webcam.drag_start_pos = Some([active_pos_x, active_pos_y]);
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
            let resized = if is_cutout {
                calculate_resized_geometry_with_padding(
                    corner,
                    current_p,
                    outer_rect,
                    viewport_rect,
                    aspect_ratio,
                    0.0,
                )
            } else {
                calculate_resized_geometry(
                    corner,
                    current_p,
                    outer_rect,
                    viewport_rect,
                    aspect_ratio,
                )
            };
            if let Some((new_width_pct, new_pos_x, new_pos_y)) = resized {
                if is_vtuber {
                    let layout = state.webcam.config.vtuber.active_layout_mut();
                    layout.width_pct = new_width_pct;
                    if let Some(pos_x) = new_pos_x {
                        layout.pos_x_pct = pos_x;
                    }
                    if let Some(pos_y) = new_pos_y {
                        layout.pos_y_pct = pos_y;
                    }
                } else {
                    state.webcam.config.width_pct = new_width_pct;
                    if let Some(pos_x) = new_pos_x {
                        state.webcam.config.pos_x_pct = pos_x;
                    }
                    if let Some(pos_y) = new_pos_y {
                        state.webcam.config.pos_y_pct = pos_y;
                    }
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

                let new_min_x = (start_min_x + delta.x).clamp(
                    viewport_rect.min.x,
                    (viewport_rect.max.x - total_w).max(viewport_rect.min.x),
                );
                let new_min_y = (start_min_y + delta.y).clamp(
                    viewport_rect.min.y,
                    (viewport_rect.max.y - total_h).max(viewport_rect.min.y),
                );

                let next_x = (new_min_x - viewport_rect.min.x) / viewport_w;
                let next_y = (new_min_y - viewport_rect.min.y) / viewport_h;
                if is_vtuber {
                    let layout = state.webcam.config.vtuber.active_layout_mut();
                    layout.pos_x_pct = next_x;
                    layout.pos_y_pct = next_y;
                } else {
                    state.webcam.config.pos_x_pct = next_x;
                    state.webcam.config.pos_y_pct = next_y;
                }
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
            if is_vtuber {
                state.webcam.config.vtuber.sync_active_profile();
            } else {
                state.webcam.config.sync_active_local_profile();
            }
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

    // 6. Draw on Order::Middle (captured by Twitch Broadcast & Replay Buffer)
    let layer_painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::new("webcam_overlay"),
        ))
        .with_clip_rect(viewport_rect);
    let painter = &layer_painter;
    let time = ui.input(|i| i.time);

    if !is_vtuber {
        paint_retro_window_frame(
            painter,
            outer_rect,
            inner_rect,
            time,
            hovered_corner,
            state.webcam.resizing_corner,
        );
        if let Some(texture) = &state.webcam.texture {
            painter.image(texture.id(), inner_rect, uv, Color32::WHITE);
        } else {
            paint_standby_screen(painter, inner_rect, STANDBY_LABEL, time);
        }
        theme::request_animation_repaint(ctx, time);
        return;
    }

    // VTuber Avatar path (`FramedWebcam` or `BorderlessCutout`)
    let has_avatar = state
        .webcam
        .vtuber
        .gpu
        .lock()
        .map(|g| g.has_avatar())
        .unwrap_or(false);

    let standby_label = if state.webcam.vtuber.loading_model_path.is_some() {
        AVATAR_STANDBY_LABEL
    } else {
        AVATAR_NO_MODEL_LABEL
    };

    if !is_cutout {
        // Framed Webcam VTuber: retro VN window frame + background + 3D avatar
        paint_retro_window_frame(
            painter,
            outer_rect,
            inner_rect,
            time,
            hovered_corner,
            state.webcam.resizing_corner,
        );
        if has_avatar {
            let raw_track = state
                .webcam
                .vtuber
                .shared_tracking
                .lock()
                .ok()
                .and_then(|g| g.latest_frame.clone());
            let mic_db = (state.mic.config.enabled && state.mic.active_stream.is_some())
                .then(|| f32::from_bits(state.mic.current_level_db.load(Ordering::Relaxed)));
            let pose = state.webcam.vtuber.rig_animator.evaluate(
                time,
                raw_track.as_ref(),
                &state.webcam.config.vtuber,
                mic_db,
            );
            let dt = state.webcam.vtuber.next_render_dt(time);
            let gpu = Arc::clone(&state.webcam.vtuber.gpu);
            let vtuber_cfg = state.webcam.config.vtuber.clone();
            let uv_arr = [uv.min.x, uv.min.y, uv.max.x, uv.max.y];
            let req_w = desired_tex_w.max(64);
            let req_h = desired_tex_h.max(64);

            painter.add(egui::PaintCallback {
                rect: inner_rect,
                callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |_info, glow_p| {
                    if let Ok(mut g) = gpu.lock() {
                        g.render_and_composite(
                            glow_p.gl(),
                            req_w,
                            req_h,
                            dt,
                            &pose,
                            &vtuber_cfg,
                            uv_arr,
                        );
                    }
                })),
            });
            ctx.request_repaint();
        } else {
            paint_standby_screen(painter, inner_rect, standby_label, time);
            theme::request_animation_repaint(ctx, time);
        }
    } else {
        // Borderless Cutout VTuber:
        // - Order::Middle gets ONLY the clean premultiplied character cutout (zero borders/chrome)
        // - Order::Debug (executed after Twitch/Replay PBO capture on Order::Tooltip) gets local-only
        //   streamer hover/resize guides!
        if has_avatar {
            let raw_track = state
                .webcam
                .vtuber
                .shared_tracking
                .lock()
                .ok()
                .and_then(|g| g.latest_frame.clone());
            let mic_db = (state.mic.config.enabled && state.mic.active_stream.is_some())
                .then(|| f32::from_bits(state.mic.current_level_db.load(Ordering::Relaxed)));
            let pose = state.webcam.vtuber.rig_animator.evaluate(
                time,
                raw_track.as_ref(),
                &state.webcam.config.vtuber,
                mic_db,
            );
            let dt = state.webcam.vtuber.next_render_dt(time);
            let gpu = Arc::clone(&state.webcam.vtuber.gpu);
            let vtuber_cfg = state.webcam.config.vtuber.clone();
            let uv_arr = [uv.min.x, uv.min.y, uv.max.x, uv.max.y];
            let req_w = desired_tex_w.max(64);
            let req_h = desired_tex_h.max(64);

            painter.add(egui::PaintCallback {
                rect: inner_rect,
                callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |_info, glow_p| {
                    if let Ok(mut g) = gpu.lock() {
                        g.render_and_composite(
                            glow_p.gl(),
                            req_w,
                            req_h,
                            dt,
                            &pose,
                            &vtuber_cfg,
                            uv_arr,
                        );
                    }
                })),
            });

            let show_local_handles = is_body_hovered
                || hovered_corner.is_some()
                || state.webcam.is_dragging
                || state.webcam.resizing_corner.is_some();
            if show_local_handles {
                let debug_painter = ctx
                    .layer_painter(egui::LayerId::new(
                        egui::Order::Debug,
                        egui::Id::new("vtuber_cutout_handles"),
                    ))
                    .with_clip_rect(viewport_rect);
                debug_painter.rect_stroke(
                    outer_rect.shrink(0.5),
                    egui::Rounding::ZERO,
                    Stroke::new(1.0, theme::RULE),
                );
                paint_corner_gems(
                    &debug_painter,
                    outer_rect.shrink(GEM_RADIUS_ACTIVE),
                    time,
                    hovered_corner,
                    state.webcam.resizing_corner,
                );
            }

            ctx.request_repaint();
        } else {
            let debug_painter = ctx
                .layer_painter(egui::LayerId::new(
                    egui::Order::Debug,
                    egui::Id::new("vtuber_cutout_handles"),
                ))
                .with_clip_rect(viewport_rect);
            debug_painter.rect_filled(
                outer_rect,
                egui::Rounding::ZERO,
                Color32::from_rgba_unmultiplied(18, 16, 30, 175),
            );
            debug_painter.rect_stroke(
                outer_rect.shrink(0.5),
                egui::Rounding::ZERO,
                Stroke::new(1.0, theme::RULE),
            );
            let font = egui::FontId::monospace(12.0);
            theme::paint_shadow_text(
                &debug_painter,
                outer_rect.center(),
                egui::Align2::CENTER_CENTER,
                standby_label,
                font,
                theme::MUTED,
                1.0,
            );
            paint_corner_gems(
                &debug_painter,
                outer_rect.shrink(GEM_RADIUS_ACTIVE),
                time,
                hovered_corner,
                state.webcam.resizing_corner,
            );
            theme::request_animation_repaint(ctx, time);
        }
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
    fn test_borderless_cutout_zero_padding_corner_resize_anchors_exact_corners() {
        let aspect_ratio = 3.0 / 4.0;
        let viewport = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(1920.0, 1080.0));
        let outer = Rect::from_min_size(Pos2::new(600.0, 300.0), Vec2::new(300.0, 400.0));

        // Top-left drag with zero padding: bottom-right corner must remain exact with 0px border
        let res = calculate_resized_geometry_with_padding(
            WebcamResizeCorner::TopLeft,
            Pos2::new(450.0, 100.0),
            outer,
            viewport,
            aspect_ratio,
            0.0,
        )
        .expect("resize within viewport should succeed");

        let (new_width_pct, new_x, new_y) = res;
        let new_w = new_width_pct * viewport.width();
        let new_h = new_w / aspect_ratio;
        let new_left = new_x.unwrap() * viewport.width();
        let new_top = new_y.unwrap() * viewport.height();
        assert!((new_w - 450.0).abs() < 1e-4);
        assert!((new_left + new_w - outer.right()).abs() < 1e-4);
        assert!((new_top + new_h - outer.bottom()).abs() < 1e-4);
    }

    #[test]
    fn test_no_emojis_in_webcam_labels_and_strings() {
        assert!(STANDBY_LABEL.chars().all(|c| c.is_ascii()));
        assert!(AVATAR_STANDBY_LABEL.chars().all(|c| c.is_ascii()));
        assert!(AVATAR_NO_MODEL_LABEL.chars().all(|c| c.is_ascii()));
    }
}
