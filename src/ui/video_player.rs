use super::networking::send_ws_command;
use crate::app::AppState;
use crate::devices::filter_type::CrtFilter;
use crate::video;
use eframe::egui;
use eframe::egui_glow;
use std::sync::atomic::Ordering;

fn capture_frame_pixels_raw(
    gl: &eframe::glow::Context,
    area: crate::video::gpu::geometry::RenderedArea,
) -> Option<(Vec<u8>, u32, u32)> {
    if area.width == 0 || area.height == 0 {
        return None;
    }
    use eframe::glow::{self, HasContext};
    let mut raw_pixels = vec![0u8; (area.width * area.height * 4) as usize];
    unsafe {
        let old_pbo = gl.get_parameter_i32(glow::PIXEL_PACK_BUFFER_BINDING);
        let alignment = gl.get_parameter_i32(glow::PACK_ALIGNMENT);
        let row_length = gl.get_parameter_i32(glow::PACK_ROW_LENGTH);
        let skip_rows = gl.get_parameter_i32(glow::PACK_SKIP_ROWS);
        let skip_pixels = gl.get_parameter_i32(glow::PACK_SKIP_PIXELS);

        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
        gl.pixel_store_i32(glow::PACK_ROW_LENGTH, 0);
        gl.pixel_store_i32(glow::PACK_SKIP_ROWS, 0);
        gl.pixel_store_i32(glow::PACK_SKIP_PIXELS, 0);
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);

        gl.read_pixels(
            area.x as i32,
            area.y as i32,
            area.width as i32,
            area.height as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(&mut raw_pixels),
        );

        gl.bind_buffer(
            glow::PIXEL_PACK_BUFFER,
            std::num::NonZeroU32::new(old_pbo as u32).map(glow::NativeBuffer),
        );
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, alignment);
        gl.pixel_store_i32(glow::PACK_ROW_LENGTH, row_length);
        gl.pixel_store_i32(glow::PACK_SKIP_ROWS, skip_rows);
        gl.pixel_store_i32(glow::PACK_SKIP_PIXELS, skip_pixels);
    }

    Some((raw_pixels, area.width, area.height))
}

fn capture_frame_pixels(
    gl: &eframe::glow::Context,
    area: crate::video::gpu::geometry::RenderedArea,
) -> Option<(Vec<u8>, u32, u32)> {
    let (mut raw_pixels, w, h) = capture_frame_pixels_raw(gl, area)?;
    let row_bytes = (w * 4) as usize;
    let mut temp_row = vec![0u8; row_bytes];
    let half_h = (h / 2) as usize;
    for y in 0..half_h {
        let opp_y = (h as usize) - 1 - y;
        let r1_start = y * row_bytes;
        let r2_start = opp_y * row_bytes;
        temp_row.copy_from_slice(&raw_pixels[r1_start..r1_start + row_bytes]);
        raw_pixels.copy_within(r2_start..r2_start + row_bytes, r1_start);
        raw_pixels[r2_start..r2_start + row_bytes].copy_from_slice(&temp_row);
    }
    Some((raw_pixels, w, h))
}

pub fn draw_video_player(state: &mut AppState, ui: &mut egui::Ui, ctx: &egui::Context) {
    state.ocr.start_worker(Some(ctx.clone()));

    // Space to capture OCR; Shift+Space to clear OCR boxes
    if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Space)) {
        if ctx.input(|i| i.modifiers.shift) {
            if !state.ocr.boxes.is_empty() {
                state.clear_ocr();
            }
        } else {
            if !state.ocr.is_processing.load(Ordering::Relaxed) {
                state.ocr.capture_requested.store(true, Ordering::Release);
                state.info("Capturing screen for Google Lens OCR...");
                ctx.request_repaint();
            }
            send_ws_command(serde_json::json!({"command": "manual_ocr"}));
        }
    }

    if !state.ui.video_window_open {
        let gpu = state.replay.gpu.clone();
        ui.painter().add(egui::PaintCallback {
            rect: ui.available_rect_before_wrap(),
            callback: std::sync::Arc::new(egui_glow::CallbackFn::new(move |_, painter| {
                gpu.lock().unwrap().destroy(painter.gl());
            })),
        });
    }
    if state.ui.video_window_open {
        let response = ui.allocate_response(ui.available_size(), egui::Sense::click());
        let video_texture = state
            .video_texture
            .as_ref()
            .expect("Video texture not initialized");
        let texture_size = video_texture.size_vec2();

        let filter =
            CrtFilter::from_u8(state.crt_filter.load(Ordering::Relaxed));
        let fft_filter_ref = if state.video.fft_filter_enabled {
            state.fft_filter.clone()
        } else {
            None
        };

        let ocr_capture = state.ocr.capture_requested.clone();
        let ocr_req_sender = state.ocr.ocr_request_sender.clone();
        if state.bank.has_pending() {
            ctx.request_repaint();
        }
        let bank_capture = state.bank.capture_handle();

        let is_preroll_active = state.twitch.preroll.is_some();
        let preroll_fade_in = state.twitch.is_preroll_in_fade_in();
        let preroll_fade_alpha = state.twitch.preroll_fade_alpha();
        let preroll_remaining_secs = state.twitch.preroll_remaining_secs();
        let is_showing_preroll = is_preroll_active && !preroll_fade_in;
        let active_frame = if is_showing_preroll {
            state.twitch.preroll_frame()
        } else {
            state.latest_frame.clone()
        };

        let ppp = ctx.pixels_per_point();
        let output_size = (response.rect.width() * ppp, response.rect.height() * ppp);
        let latest_frame_dim = active_frame.as_ref().map(|f| (f.width, f.height));
        let res = latest_frame_dim
            .or_else(|| {
                if state.hardware.selected_resolution.0 > 0 {
                    Some(state.hardware.selected_resolution)
                } else {
                    None
                }
            })
            .unwrap_or((texture_size.x as u32, texture_size.y as u32));
        let h_stretch = if is_showing_preroll {
            1.0
        } else {
            state.video.horizontal_stretch
        };
        let rendered_area_geom = crate::video::gpu::geometry::RenderedArea::fit(
            res,
            output_size,
            h_stretch,
        );
        let video_rect_min = response.rect.min
            + egui::vec2(
                rendered_area_geom.x as f32 / ppp,
                rendered_area_geom.y as f32 / ppp,
            );
        let video_rect_size = egui::vec2(
            rendered_area_geom.width as f32 / ppp,
            rendered_area_geom.height as f32 / ppp,
        );
        let video_rect = egui::Rect::from_min_size(video_rect_min, video_rect_size);

        let cathode_params = video::gpu::CathodeInterferenceShaderParams::from_state(state);
        let time = ui.input(|i| i.time) as f32;
        let is_crt_on = !is_showing_preroll && filter != CrtFilter::Off;
        let night_mode_active = is_crt_on && state.video.lights_off_night_mode;
        let retro_pc_frame = is_crt_on && state.video.retro_pc_frame;
        let effective_dark_mode =
            is_crt_on && (state.video.retro_pc_frame_dark_mode || night_mode_active);
        let retro_pc_ambient_glow = if night_mode_active {
            (state.video.retro_pc_ambient_glow * 1.50).min(1.0)
        } else if is_showing_preroll {
            0.0
        } else {
            state.video.retro_pc_ambient_glow
        };
        let night_mode_glow_intensity = if night_mode_active {
            state.video.night_mode_glow_intensity
        } else {
            0.0
        };

        let border_crop = [
            state.video.border_crop_left,
            state.video.border_crop_right,
            state.video.border_crop_top,
            state.video.border_crop_bottom,
        ];
        let underscan = [state.video.underscan_x, state.video.underscan_y];
        let overscan = [state.video.overscan_x, state.video.overscan_y];
        let crt_surface = crate::dict::popup::calculate_crt_raster_surface(
            video_rect,
            border_crop,
            underscan,
            overscan,
        );

        let pointer_pos = ui.input(|i| i.pointer.hover_pos());
        let is_on_crt_surface = pointer_pos.map_or(false, |pos| {
            crt_surface.contains(pos) && !state.twitch.overlay_contains(pos)
        });
        let is_hovering_controls = is_hovering_video_controls(state, pointer_pos, response.rect);
        let software_mouse_active =
            is_software_mouse_active(state, is_on_crt_surface) && !is_hovering_controls;

        let (software_mouse_pos, software_mouse_clip) = if software_mouse_active {
            let pos = pointer_pos.map(|pos| {
                let rel_x = pos.x - response.rect.min.x;
                let rel_y = pos.y - response.rect.min.y;
                (rel_x * ppp, rel_y * ppp)
            });
            let clip_min_x = ((crt_surface.min.x - response.rect.min.x) * ppp).round() as i32;
            let clip_max_x = ((crt_surface.max.x - response.rect.min.x) * ppp).round() as i32;
            let clip_min_y = ((crt_surface.min.y - response.rect.min.y) * ppp).round() as i32;
            let clip_max_y = ((crt_surface.max.y - response.rect.min.y) * ppp).round() as i32;
            let clip_w = (clip_max_x - clip_min_x).max(1);
            let clip_h = (clip_max_y - clip_min_y).max(1);
            let clip_y = (output_size.1 as i32 - clip_max_y).max(0);
            (pos, Some([clip_min_x, clip_y, clip_w, clip_h]))
        } else {
            (None, None)
        };

        if software_mouse_active {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        }

        if state.cathode_interference.enabled
            || retro_pc_frame
            || (night_mode_glow_intensity > 0.001)
            || is_preroll_active
        {
            ui.ctx().request_repaint();
        }

        if !is_showing_preroll
            && (state.video.pixelate_filter_enabled
                || filter != CrtFilter::Off
                || state.cathode_interference.enabled)
        {
            if let Some(renderer_arc) = &state.crt_renderer {
                let renderer_clone = renderer_arc.clone();
                let params = video::gpu::ShaderParams::from_state(state);
                let halo_params = video::gpu::HaloShaderParams::from_state(state);
                let cathode_params_cb = cathode_params.clone();
                let pixelate = state.video.pixelate_filter_enabled;
                let run_lottes = filter == CrtFilter::Lottes;
                let run_halo = filter == CrtFilter::Halo;
                let (warp, corner_size, filter_type) = if run_lottes {
                    ([params.warp_x, params.warp_y], 0.0, 1)
                } else if run_halo {
                    (
                        if halo_params.curvature {
                            [0.031, 0.041]
                        } else {
                            [0.0, 0.0]
                        },
                        halo_params.corner_size,
                        2,
                    )
                } else {
                    ([0.0, 0.0], 0.0, 0)
                };
                let glass_params = video::gpu::GlassShaderParams::from_state(
                    state,
                    warp,
                    corner_size,
                    filter_type,
                );
                let rect = response.rect;
                let latest_frame = active_frame.clone();
                let video_texture_id = state.video_texture.as_ref().map(|t| t.id());
                let fft_clone = fft_filter_ref.clone();
                let fft_threshold = state.fft_mask_threshold;
                let fft_black = state.fft_black_threshold;

                let replay_gpu = state.replay.gpu.clone();
                let replay = state
                    .replay
                    .runtime
                    .as_ref()
                    .map(crate::replay::gpu::RuntimeView::new);

                let ocr_capture_cb = ocr_capture.clone();
                let ocr_req_sender_cb = ocr_req_sender.clone();
                let bank_capture_cb = bank_capture.clone();

                let capture_overlays = state.replay.config.capture_overlays;
                let webcam_enabled = state.webcam.config.enabled;
                let stream_res = state.hardware.selected_resolution;

                let callback = egui::PaintCallback {
                    rect: response.rect,
                    callback: std::sync::Arc::new(egui_glow::CallbackFn::new(
                        move |_info, painter| {
                            let mut renderer = renderer_clone.lock().unwrap();
                            let output_size = (rect.width() * ppp, rect.height() * ppp);
                            let fallback_tex = video_texture_id.and_then(|id| painter.texture(id));

                            let res = latest_frame
                                .as_ref()
                                .map(|f| (f.width, f.height))
                                .or_else(|| {
                                    if stream_res.0 > 0 {
                                        Some(stream_res)
                                    } else {
                                        None
                                    }
                                })
                                .unwrap_or((texture_size.x as u32, texture_size.y as u32));

                            let is_mining_capture = bank_capture_cb.has_pending();
                            let current_software_mouse_pos = if is_mining_capture {
                                None
                            } else {
                                software_mouse_pos
                            };
                            let current_software_mouse_clip = if is_mining_capture {
                                None
                            } else {
                                software_mouse_clip
                            };

                            let rendered_area = renderer.paint(
                                painter.gl(),
                                painter,
                                rect,
                                latest_frame.as_deref(),
                                fallback_tex,
                                res,
                                output_size,
                                &params,
                                &halo_params,
                                &cathode_params_cb,
                                Some(&glass_params),
                                time,
                                pixelate,
                                run_lottes,
                                run_halo,
                                fft_clone.as_ref(),
                                fft_threshold,
                                fft_black,
                                current_software_mouse_pos,
                                current_software_mouse_clip,
                                is_mining_capture,
                            );
                            let at = latest_frame
                                .as_ref()
                                .map(|f| f.captured_at)
                                .unwrap_or(0);
                            if ocr_capture_cb.swap(false, Ordering::AcqRel) {
                                if let Some((pixels, w, h)) =
                                    capture_frame_pixels_raw(painter.gl(), rendered_area)
                                {
                                    if let Some(req_tx) = &ocr_req_sender_cb {
                                        let _ = req_tx.send(crate::ocr::models::OcrCaptureRequest {
                                            raw_rgba: pixels,
                                            width: w,
                                            height: h,
                                        });
                                    }
                                }
                            }

                            if retro_pc_frame {
                                let (warp, corner_size, filter_type) = if run_lottes {
                                    ([params.warp_x, params.warp_y], 0.0, 1)
                                } else if run_halo {
                                    (
                                        if halo_params.curvature {
                                            [0.031, 0.041]
                                        } else {
                                            [0.0, 0.0]
                                        },
                                        halo_params.corner_size,
                                        2,
                                    )
                                } else {
                                    ([0.0, 0.0], 0.0, 0)
                                };
                                renderer.draw_retro_frame(
                                    painter.gl(),
                                    res,
                                    output_size,
                                    params.horizontal_stretch,
                                    params.border_crop,
                                    warp,
                                    corner_size,
                                    filter_type,
                                    time,
                                    retro_pc_ambient_glow,
                                    effective_dark_mode,
                                );
                            }

                            if night_mode_glow_intensity > 0.001 {
                                let (warp, corner_size, filter_type) = if run_lottes {
                                    ([params.warp_x, params.warp_y], 0.0, 1)
                                } else if run_halo {
                                    (
                                        if halo_params.curvature {
                                            [0.031, 0.041]
                                        } else {
                                            [0.0, 0.0]
                                        },
                                        halo_params.corner_size,
                                        2,
                                    )
                                } else {
                                    ([0.0, 0.0], 0.0, 0)
                                };
                                renderer.draw_night_mode_glow(
                                    painter.gl(),
                                    res,
                                    output_size,
                                    params.horizontal_stretch,
                                    params.border_crop,
                                    warp,
                                    corner_size,
                                    filter_type,
                                    night_mode_glow_intensity,
                                );
                            }

                            let full_window_effects =
                                retro_pc_frame || night_mode_glow_intensity > 0.001;
                            let full_or_rendered_area = if full_window_effects {
                                crate::video::gpu::geometry::RenderedArea::full(output_size)
                            } else {
                                rendered_area
                            };

                            if let Some(request) = bank_capture_cb.take_pending() {
                                let shot =
                                    capture_frame_pixels(painter.gl(), full_or_rendered_area);
                                bank_capture_cb.save(request, shot);
                            }

                            if !capture_overlays && !webcam_enabled {
                                replay_gpu.lock().unwrap().capture(
                                    painter.gl(),
                                    replay.as_ref(),
                                    None,
                                    full_or_rendered_area,
                                    at,
                                );
                            }
                        },
                    )),
                };
                ui.painter().add(callback);
            }
        } else {
            let renderer_clone = state
                .crt_renderer
                .as_ref()
                .expect("Renderer not initialized")
                .clone();
            let rect = response.rect;
            let background_color = if is_showing_preroll {
                [0.0, 0.0, 0.0]
            } else if state.video.use_magenta_background {
                [1.0, 0.0, 1.0]
            } else {
                [0.0, 0.0, 0.0]
            };
            let horizontal_stretch = if is_showing_preroll {
                1.0
            } else {
                state.video.horizontal_stretch
            };
            let median_filter_enabled = !is_showing_preroll && state.video.median_filter_enabled;
            let median_mix = if is_showing_preroll { 0.0 } else { state.video.median_mix };
            let deinterlace_filter_enabled = !is_showing_preroll && state.video.deinterlace_filter_enabled;
            let deinterlace_mode = if is_showing_preroll { 0 } else { state.video.deinterlace_mode };
            let deinterlace_blend = if is_showing_preroll { 0.0 } else { state.video.deinterlace_blend };
            let deinterlace_motion_threshold = if is_showing_preroll { 0.0 } else { state.video.deinterlace_motion_threshold };
            let deinterlace_line_spacing = if is_showing_preroll { 1.0 } else { state.video.deinterlace_line_spacing };
            let deinterlace_spatial_mix = if is_showing_preroll { 0.0 } else { state.video.deinterlace_spatial_mix };
            let vibrance = if is_showing_preroll { 0.0 } else { state.video.vibrance };
            let overscan_x = if is_showing_preroll { 0.0 } else { state.video.overscan_x };
            let overscan_y = if is_showing_preroll { 0.0 } else { state.video.overscan_y };
            let underscan_x = if is_showing_preroll { 0.0 } else { state.video.underscan_x };
            let underscan_y = if is_showing_preroll { 0.0 } else { state.video.underscan_y };
            let border_crop = if is_showing_preroll {
                [0.0, 0.0, 0.0, 0.0]
            } else {
                [
                    state.video.border_crop_left,
                    state.video.border_crop_right,
                    state.video.border_crop_top,
                    state.video.border_crop_bottom,
                ]
            };
            let retro_pc_ambient_glow = if is_showing_preroll { 0.0 } else { retro_pc_ambient_glow };
            let scaler_filter = if is_showing_preroll {
                0
            } else {
                state.scaler_filter.load(Ordering::Relaxed)
            };
            let latest_frame = active_frame.clone();
            let video_texture_id = state.video_texture.as_ref().map(|t| t.id());
            let fft_clone = if is_showing_preroll { None } else { fft_filter_ref.clone() };
            let fft_threshold = if is_showing_preroll { 0.0 } else { state.fft_mask_threshold };
            let fft_black = if is_showing_preroll { 0.0 } else { state.fft_black_threshold };
            let glass_params = if is_showing_preroll {
                None
            } else {
                Some(video::gpu::GlassShaderParams::from_state(state, [0.0, 0.0], 0.0, 0))
            };

            let replay_gpu = state.replay.gpu.clone();
            let replay = state
                .replay
                .runtime
                .as_ref()
                .map(crate::replay::gpu::RuntimeView::new);

            let ocr_capture_cb = ocr_capture.clone();
            let ocr_req_sender_cb = ocr_req_sender.clone();
            let bank_capture_cb = bank_capture.clone();
            let capture_overlays = state.replay.config.capture_overlays;
            let webcam_enabled = state.webcam.config.enabled;
            let stream_res = state.hardware.selected_resolution;

            let callback = egui::PaintCallback {
                rect,
                callback: std::sync::Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                    let fallback_tex = video_texture_id.and_then(|id| painter.texture(id));
                    let res = latest_frame
                        .as_ref()
                        .map(|f| (f.width, f.height))
                        .or_else(|| {
                            if stream_res.0 > 0 {
                                Some(stream_res)
                            } else {
                                None
                            }
                        })
                        .unwrap_or((texture_size.x as u32, texture_size.y as u32));
                    let rendered_area = renderer_clone.lock().unwrap().draw_passthrough(
                        painter.gl(),
                        painter,
                        rect,
                        latest_frame.as_deref(),
                        fallback_tex,
                        res,
                        (rect.width() * ppp, rect.height() * ppp),
                        background_color,
                        horizontal_stretch,
                        median_filter_enabled,
                        median_mix,
                        deinterlace_filter_enabled,
                        deinterlace_mode,
                        deinterlace_blend,
                        deinterlace_motion_threshold,
                        deinterlace_line_spacing,
                        deinterlace_spatial_mix,
                        vibrance,
                        scaler_filter,
                        overscan_x,
                        overscan_y,
                        underscan_x,
                        underscan_y,
                        border_crop,
                        fft_clone.as_ref(),
                        fft_threshold,
                        fft_black,
                        None,
                        glass_params.as_ref(),
                        0.0,
                    );
                    let at = latest_frame
                        .as_ref()
                        .map(|f| f.captured_at)
                        .unwrap_or(0);
                    if ocr_capture_cb.swap(false, Ordering::AcqRel) {
                        if let Some((pixels, w, h)) =
                            capture_frame_pixels_raw(painter.gl(), rendered_area)
                        {
                            if let Some(req_tx) = &ocr_req_sender_cb {
                                let _ = req_tx.send(crate::ocr::models::OcrCaptureRequest {
                                    raw_rgba: pixels,
                                    width: w,
                                    height: h,
                                });
                            }
                        }
                    }

                    if retro_pc_frame && !is_showing_preroll {
                        renderer_clone.lock().unwrap().draw_retro_frame(
                            painter.gl(),
                            res,
                            (rect.width() * ppp, rect.height() * ppp),
                            horizontal_stretch,
                            border_crop,
                            [0.0, 0.0],
                            0.0,
                            0,
                            time,
                            retro_pc_ambient_glow,
                            false,
                        );
                    }

                    let full_or_rendered_area = if retro_pc_frame {
                        crate::video::gpu::geometry::RenderedArea::full((
                            rect.width() * ppp,
                            rect.height() * ppp,
                        ))
                    } else {
                        rendered_area
                    };

                    if let Some(request) = bank_capture_cb.take_pending() {
                        let shot =
                            capture_frame_pixels(painter.gl(), full_or_rendered_area);
                        bank_capture_cb.save(request, shot);
                    }

                    if !capture_overlays && !webcam_enabled {
                        replay_gpu.lock().unwrap().capture(
                            painter.gl(),
                            replay.as_ref(),
                            None,
                            full_or_rendered_area,
                            at,
                        );
                    }
                })),
            };
            ui.painter().add(callback);
        }

        // Draw interactive OCR bounding box overlay on top of the video image
        if !is_preroll_active {
            crate::ocr::overlay::draw_ocr_overlay(ui, state, video_rect);
        }

        // Niconico chat comments, drawn by the renderer under the CRT effects.
        if !is_preroll_active {
            crate::ui::niconico::update(state, ctx, video_rect);
        }

        // Video alert notices, drawn by the renderer under the CRT effects.
        if !is_preroll_active {
            crate::ui::alerts::update(state, ctx, video_rect);
        }

        // Extra widget for local video player mode (scrub bar, play/pause/stop)
        if state.video_player.is_some() {
            draw_video_file_controls(state, ui, ctx, response.rect);
        }

        if software_mouse_active && !is_preroll_active {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        }

        // Draw webcam overlay on top of everything (suppressed during pre-roll)
        if !is_preroll_active {
            crate::ui::webcam_overlay::draw_webcam_overlay(state, ui, ctx, response.rect);
        }

        // During pre-roll fade out / fade in transition, render a black overlay across the video
        // surface. This is rendered on the UI painter so it will be captured into the livestream
        // broadcast by the replay_gpu.capture callback below.
        if is_preroll_active && preroll_fade_alpha > 0.001 {
            let black_alpha = (preroll_fade_alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
            ui.painter().rect_filled(
                response.rect,
                0.0,
                egui::Color32::from_black_alpha(black_alpha),
            );
        }

        // When capturing overlays in the replay buffer, when webcam is enabled, or when broadcasting to Twitch,
        // capture the frame after egui has rendered the interactive OCR boxes, dictionary popup, and webcam overlay.
        let is_broadcasting = state.twitch.is_broadcasting() || state.twitch.broadcast.is_some();
        let capture_replay_here = state.replay.config.capture_overlays || state.webcam.config.enabled;
        if capture_replay_here || is_broadcasting {
            let replay_gpu = state.replay.gpu.clone();
            let replay = if capture_replay_here {
                state
                    .replay
                    .runtime
                    .as_ref()
                    .map(crate::replay::gpu::RuntimeView::new)
            } else {
                None
            };
            let broadcast_sink = state.twitch.broadcast.as_ref().map(|b| {
                crate::replay::gpu::BroadcastSink {
                    tx: b.video_tx.clone(),
                    free_rx: b.video_free_rx.clone(),
                    dropped: b.metrics.video_dropped.clone(),
                }
            });
            let at = active_frame
                .as_ref()
                .map(|f| f.captured_at)
                .unwrap_or_else(crate::replay::now_us);
            let full_window_effects =
                retro_pc_frame || night_mode_glow_intensity > 0.001;
            let rendered_area = if full_window_effects {
                crate::video::gpu::geometry::RenderedArea::full(output_size)
            } else {
                rendered_area_geom
            };

            let callback = egui::PaintCallback {
                rect: response.rect,
                callback: std::sync::Arc::new(egui_glow::CallbackFn::new(
                    move |_info, painter| {
                        replay_gpu.lock().unwrap().capture(
                            painter.gl(),
                            replay.as_ref(),
                            broadcast_sink.as_ref(),
                            rendered_area,
                            at,
                        );
                    },
                )),
            };
            ctx.layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::new("replay_capture_overlay"),
            ))
            .add(callback);
        }

        // Draw local streamer pre-roll countdown overlay.
        // NOTE: This is intentionally rendered on egui::Order::Debug (which executes after egui::Order::Tooltip).
        // That way, it is visible in the local preview window, but replay_gpu.capture (which runs in Order::Tooltip)
        // never captures it into the livestream broadcast.
        if is_preroll_active {
            if let Some(rem_secs) = preroll_remaining_secs {
                draw_preroll_counter_preview(ctx, response.rect, rem_secs);
            }
        }

        if response.double_clicked() {
            let is_fullscreen = !ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(is_fullscreen));
        }
    }
}

/// Determines if the retro software mouse pointer should be active.
/// Returns false if CRT filter is off, feature is disabled, mouse is outside
/// the CRT video surface, any system dialog/overlay is open, or if the
/// dictionary popup is active and rendered on top of the CRT filter.
pub fn is_software_mouse_active(state: &AppState, is_on_crt_surface: bool) -> bool {
    if state.twitch.preroll.is_some() {
        return false;
    }
    let filter = CrtFilter::from_u8(state.crt_filter.load(Ordering::Relaxed));
    let is_crt_on = filter != CrtFilter::Off;
    let popup_active = state.dict.popup.is_some();
    let popup_under_crt = state.dict.popup_under_crt;

    let has_system_dialog = state.ui.show_quit_dialog
        || state.ui.show_stop_stream_dialog
        || state.ui.show_first_run_dialog
        || (state.config_load_error.is_some() && !state.ui.dismissed_config_error)
        || state.video.fft_mask_window_open
        || state.ui.debug_open;

    is_crt_on
        && state.video.retro_software_mouse
        && is_on_crt_surface
        && !has_system_dialog
        && !(popup_active && !popup_under_crt)
        && !(state.webcam.config.enabled
            && (state.webcam.is_dragging || state.webcam.resizing_corner.is_some()))
}

/// Checks if the mouse cursor is hovering over or near the bottom video controls widget.
pub fn is_hovering_video_controls(
    state: &AppState,
    pointer_pos: Option<egui::Pos2>,
    window_rect: egui::Rect,
) -> bool {
    if state.video_player.is_none() {
        return false;
    }
    let bar_height = 46.0;
    let hover_zone = egui::Rect::from_min_max(
        egui::pos2(
            window_rect.min.x,
            (window_rect.max.y - bar_height - 35.0).max(window_rect.min.y),
        ),
        window_rect.max,
    );
    pointer_pos.is_some_and(|pos| hover_zone.contains(pos))
        || state.video_player.as_ref().is_some_and(|p| p.is_seeking)
}

/// Draws the auto-hiding playback control widget with scrub bar, play, pause, and stop buttons.
pub fn draw_video_file_controls(
    state: &mut AppState,
    _ui: &mut egui::Ui,
    ctx: &egui::Context,
    screen_rect: egui::Rect,
) {
    let pointer_pos = ctx.input(|i| i.pointer.hover_pos());
    if !is_hovering_video_controls(state, pointer_pos, screen_rect) {
        return;
    }

    let Some(ref mut player) = state.video_player else {
        return;
    };

    let bar_height = 46.0;
    let bar_rect = egui::Rect::from_min_max(
        egui::pos2(screen_rect.min.x, screen_rect.max.y - bar_height),
        egui::pos2(screen_rect.max.x, screen_rect.max.y),
    );

    let mut should_stop = false;

    egui::Area::new(egui::Id::new("video_file_bottom_bar"))
        .order(egui::Order::Foreground)
        .fixed_pos(bar_rect.min)
        .show(ctx, |ui| {
            ui.set_min_width(bar_rect.width());
            ui.set_max_width(bar_rect.width());
            egui::Frame::none()
                .fill(egui::Color32::from_rgba_unmultiplied(16, 16, 16, 235))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(50, 50, 50)))
                .inner_margin(egui::Margin::symmetric(14.0, 8.0))
                .show(ui, |ui| {
                    ui.set_min_width((bar_rect.width() - 28.0).max(0.0));
                    ui.set_max_width((bar_rect.width() - 28.0).max(0.0));
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(10.0, 0.0);

                        // Play / Pause button
                        let is_paused = player.is_paused;
                        let play_pause_icon = if is_paused { "▶" } else { "⏸" };
                        let play_pause_tip = if is_paused { "Play (P)" } else { "Pause (P)" };
                        let play_btn = egui::Button::new(
                            egui::RichText::new(play_pause_icon)
                                .size(13.0)
                                .strong()
                                .color(egui::Color32::WHITE),
                        )
                        .fill(egui::Color32::from_rgb(34, 34, 34))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(68, 68, 68)))
                        .rounding(egui::Rounding::same(2.0));

                        if ui.add(play_btn).on_hover_text(play_pause_tip).clicked() {
                            let cmd = if is_paused {
                                crate::video::player::PlayerCommand::Play
                            } else {
                                crate::video::player::PlayerCommand::Pause
                            };
                            let _ = player.command_sender.send(cmd);
                            ctx.request_repaint();
                        }

                        // Stop button (resets to 0 and pauses)
                        let stop_btn = egui::Button::new(
                            egui::RichText::new("⏹")
                                .size(12.0)
                                .strong()
                                .color(egui::Color32::WHITE),
                        )
                        .fill(egui::Color32::from_rgb(34, 34, 34))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(68, 68, 68)))
                        .rounding(egui::Rounding::same(2.0));

                        if ui
                            .add(stop_btn)
                            .on_hover_text("Stop (Reset to beginning)")
                            .clicked()
                        {
                            let _ = player
                                .command_sender
                                .send(crate::video::player::PlayerCommand::Stop);
                            ctx.request_repaint();
                        }

                        // Current time / Total duration label
                        let cur_str = crate::video::player::format_time_with_duration(
                            player.current_position_secs,
                            player.duration_secs,
                        );
                        let dur_str = crate::video::player::format_time(player.duration_secs);
                        ui.label(
                            egui::RichText::new(format!("{} / {}", cur_str, dur_str))
                                .monospace()
                                .size(11.5)
                                .strong()
                                .color(egui::Color32::from_rgb(220, 220, 220)),
                        );

                        // Horizontal seek / scrub bar filling the entire middle width
                        let exit_btn_width = 85.0;
                        let item_spacing = ui.spacing().item_spacing.x;
                        let remaining_width = ui.available_width();
                        let seek_width = (remaining_width - exit_btn_width - item_spacing).max(60.0);

                        let duration = player.duration_secs.max(0.1);
                        let current_pos = player.current_position_secs.clamp(0.0, duration);

                        let (seek_rect, seek_resp) = ui.allocate_exact_size(
                            egui::vec2(seek_width, 22.0),
                            egui::Sense::click_and_drag(),
                        );

                        if seek_resp.drag_started() {
                            player.is_seeking = true;
                            player.was_playing_before_scrub = !player.is_paused;
                            if player.was_playing_before_scrub {
                                let _ = player
                                    .command_sender
                                    .send(crate::video::player::PlayerCommand::Pause);
                            }
                        }

                        if seek_resp.dragged() {
                            if let Some(mouse_pos) = seek_resp.interact_pointer_pos() {
                                let fraction = ((mouse_pos.x - seek_rect.min.x) / seek_rect.width())
                                    .clamp(0.0, 1.0);
                                let seek_target = fraction as f64 * duration;
                                player.current_position_secs = seek_target;
                                let _ = player
                                    .command_sender
                                    .send(crate::video::player::PlayerCommand::Seek(seek_target));
                                ctx.request_repaint();
                            }
                        }

                        if seek_resp.drag_stopped() {
                            player.is_seeking = false;
                            if let Some(mouse_pos) = seek_resp.interact_pointer_pos() {
                                let fraction = ((mouse_pos.x - seek_rect.min.x) / seek_rect.width())
                                    .clamp(0.0, 1.0);
                                let seek_target = fraction as f64 * duration;
                                player.current_position_secs = seek_target;
                                let _ = player
                                    .command_sender
                                    .send(crate::video::player::PlayerCommand::Seek(seek_target));
                            }
                            if player.was_playing_before_scrub {
                                let _ = player
                                    .command_sender
                                    .send(crate::video::player::PlayerCommand::Play);
                            }
                            player.was_playing_before_scrub = false;
                            ctx.request_repaint();
                        }

                        if seek_resp.clicked() && !seek_resp.dragged() {
                            if let Some(mouse_pos) = seek_resp.interact_pointer_pos() {
                                let fraction = ((mouse_pos.x - seek_rect.min.x) / seek_rect.width())
                                    .clamp(0.0, 1.0);
                                let seek_target = fraction as f64 * duration;
                                player.current_position_secs = seek_target;
                                let _ = player
                                    .command_sender
                                    .send(crate::video::player::PlayerCommand::Seek(seek_target));
                                ctx.request_repaint();
                            }
                        }

                        // Hover timestamp tooltip
                        if let Some(hover_pos) = seek_resp.hover_pos() {
                            let hover_fraction = ((hover_pos.x - seek_rect.min.x) / seek_rect.width())
                                .clamp(0.0, 1.0);
                            let hover_secs = hover_fraction as f64 * duration;
                            seek_resp.clone().on_hover_ui_at_pointer(|ui| {
                                ui.label(
                                    egui::RichText::new(crate::video::player::format_time_with_duration(
                                        hover_secs,
                                        duration,
                                    ))
                                    .monospace()
                                    .size(11.0)
                                    .color(egui::Color32::WHITE),
                                );
                            });
                        }

                        // Paint seek track and thumb handle
                        let painter = ui.painter();
                        let track_center_y = seek_rect.center().y;
                        let track_h = if seek_resp.hovered() || seek_resp.dragged() {
                            6.0
                        } else {
                            4.0
                        };
                        let track_rect = egui::Rect::from_min_max(
                            egui::pos2(seek_rect.min.x, track_center_y - track_h / 2.0),
                            egui::pos2(seek_rect.max.x, track_center_y + track_h / 2.0),
                        );
                        painter.rect_filled(
                            track_rect,
                            egui::Rounding::same(track_h / 2.0),
                            egui::Color32::from_rgb(55, 55, 55),
                        );

                        let progress_fraction = (current_pos / duration).clamp(0.0, 1.0) as f32;
                        let progress_x = seek_rect.min.x + progress_fraction * seek_rect.width();
                        let progress_rect = egui::Rect::from_min_max(
                            track_rect.min,
                            egui::pos2(progress_x, track_rect.max.y),
                        );
                        painter.rect_filled(
                            progress_rect,
                            egui::Rounding::same(track_h / 2.0),
                            egui::Color32::from_rgb(45, 140, 255),
                        );

                        let pip_radius = if seek_resp.dragged() {
                            7.0
                        } else if seek_resp.hovered() {
                            6.0
                        } else {
                            4.5
                        };
                        painter.circle_filled(
                            egui::pos2(progress_x, track_center_y),
                            pip_radius,
                            egui::Color32::WHITE,
                        );

                        // Stop & Exit video mode button
                        let exit_btn = egui::Button::new(
                            egui::RichText::new("Stop & Exit")
                                .size(10.5)
                                .color(egui::Color32::from_rgb(255, 180, 180)),
                        )
                        .fill(egui::Color32::from_rgb(60, 20, 20))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 30, 30)))
                        .rounding(egui::Rounding::same(2.0));

                        if ui
                            .add_sized([exit_btn_width, 22.0], exit_btn)
                            .on_hover_text("Exit video player and return to main controls")
                            .clicked()
                        {
                            should_stop = true;
                        }
                    });
                });
        });

    if should_stop {
        state.stop_stream(ctx);
    }
}

/// Renders a local-only countdown badge over the video preview on Order::Debug.
/// This executes after the broadcast capture layer (Order::Tooltip), ensuring that the
/// countdown timer is visible to the streamer in preview but NEVER broadcast to Twitch.
fn draw_preroll_counter_preview(ctx: &egui::Context, player_rect: egui::Rect, rem_secs: f64) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Debug,
        egui::Id::new("preroll_countdown_preview"),
    ));

    let mins = (rem_secs / 60.0).floor() as u64;
    let secs = (rem_secs % 60.0).floor() as u64;
    let time_str = format!("{:02}:{:02}", mins, secs);

    let badge_width = 140.0;
    let badge_height = 42.0;
    let margin = 16.0;
    let badge_rect = egui::Rect::from_min_size(
        egui::pos2(
            player_rect.right() - badge_width - margin,
            player_rect.top() + margin,
        ),
        egui::vec2(badge_width, badge_height),
    );

    // Subtle dark translucent background with amber/orange border
    painter.rect_filled(
        badge_rect,
        egui::Rounding::same(6.0),
        egui::Color32::from_rgba_premultiplied(16, 18, 24, 230),
    );
    painter.rect_stroke(
        badge_rect,
        egui::Rounding::same(6.0),
        egui::Stroke::new(1.0, egui::Color32::from_rgb(240, 160, 32)),
    );

    // Indicator dot
    let dot_center = egui::pos2(badge_rect.left() + 14.0, badge_rect.top() + 13.0);
    painter.circle_filled(dot_center, 3.5, egui::Color32::from_rgb(240, 160, 32));

    // Label
    painter.text(
        egui::pos2(badge_rect.left() + 24.0, badge_rect.top() + 13.0),
        egui::Align2::LEFT_CENTER,
        "PRE-ROLL (PREVIEW)",
        egui::FontId::monospace(9.0),
        egui::Color32::from_gray(190),
    );

    // Digital countdown
    painter.text(
        egui::pos2(badge_rect.center().x, badge_rect.bottom() - 13.0),
        egui::Align2::CENTER_CENTER,
        time_str,
        egui::FontId::monospace(16.0),
        egui::Color32::from_rgb(255, 255, 255),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    #[test]
    fn test_is_software_mouse_active_conditions() {
        let mut state = AppState::default();
        state.video.retro_software_mouse = true;
        state.crt_filter.store(1, Ordering::Relaxed); // Lottes CRT

        // Normal case on CRT surface
        assert!(is_software_mouse_active(&state, true));
        // Off CRT surface
        assert!(!is_software_mouse_active(&state, false));

        // CRT filter turned off
        state.crt_filter.store(0, Ordering::Relaxed);
        assert!(!is_software_mouse_active(&state, true));
        state.crt_filter.store(1, Ordering::Relaxed);

        // Feature toggle disabled
        state.video.retro_software_mouse = false;
        assert!(!is_software_mouse_active(&state, true));
        state.video.retro_software_mouse = true;

        // Modal / system dialogs restore hardware cursor
        state.ui.show_quit_dialog = true;
        assert!(!is_software_mouse_active(&state, true));
        state.ui.show_quit_dialog = false;

        state.ui.show_stop_stream_dialog = true;
        assert!(!is_software_mouse_active(&state, true));
        state.ui.show_stop_stream_dialog = false;

        state.ui.show_first_run_dialog = true;
        assert!(!is_software_mouse_active(&state, true));
        state.ui.show_first_run_dialog = false;

        state.config_load_error = Some("corrupt config".into());
        state.ui.dismissed_config_error = false;
        assert!(!is_software_mouse_active(&state, true));
        state.ui.dismissed_config_error = true;
        assert!(is_software_mouse_active(&state, true));

        state.video.fft_mask_window_open = true;
        assert!(!is_software_mouse_active(&state, true));
        state.video.fft_mask_window_open = false;

        state.ui.debug_open = true;
        assert!(!is_software_mouse_active(&state, true));
        state.ui.debug_open = false;

        // Dictionary popup interactions
        state.dict.popup = Some(crate::dict::DictPopupState {
            matched_term: "test".into(),
            source_text: "test".into(),
            char_range: (0, 4),
            box_rect: egui::Rect::ZERO,
            word_rect: egui::Rect::ZERO,
            extra_word_rects: Vec::new(),
            entries: Vec::new(),
            popup_rect: None,
            is_popup_hovered: false,
            last_hover_time: std::time::Instant::now(),
            last_word_hover_time: std::time::Instant::now(),
        });

        // Popup active ON TOP of CRT -> software mouse disabled (hardware cursor shown)
        state.dict.popup_under_crt = false;
        assert!(!is_software_mouse_active(&state, true));

        // Popup active UNDER CRT -> software mouse remains active
        state.dict.popup_under_crt = true;
        assert!(is_software_mouse_active(&state, true));
    }

    #[test]
    fn test_is_hovering_video_controls() {
        let mut state = AppState::default();
        let window_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(640.0, 480.0));

        // When no video player active, always false
        assert!(!is_hovering_video_controls(&state, Some(egui::pos2(320.0, 460.0)), window_rect));

        // When video player is active
        let (cmd_tx, _cmd_rx) = crossbeam_channel::unbounded();
        state.video_player = Some(crate::app::models::VideoPlayerState {
            file_path: std::path::PathBuf::from("test.mp4"),
            duration_secs: 100.0,
            current_position_secs: 10.0,
            is_paused: false,
            is_eof: false,
            is_seeking: false,
            was_playing_before_scrub: false,
            command_sender: cmd_tx,
            position_us: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            is_paused_shared: Arc::new(AtomicBool::new(false)),
            is_eof_shared: Arc::new(AtomicBool::new(false)),
            stop_requested: Arc::new(AtomicBool::new(false)),
            file_width: 640,
            file_height: 480,
            player_thread: None,
        });

        // Near top/middle of screen -> false
        assert!(!is_hovering_video_controls(&state, Some(egui::pos2(320.0, 240.0)), window_rect));
        assert!(!is_hovering_video_controls(&state, Some(egui::pos2(320.0, 100.0)), window_rect));

        // Near bottom of screen within hover zone -> true
        assert!(is_hovering_video_controls(&state, Some(egui::pos2(320.0, 460.0)), window_rect));
        assert!(is_hovering_video_controls(&state, Some(egui::pos2(320.0, 440.0)), window_rect));

        // When actively seeking/dragging, remains true even if cursor moves away
        if let Some(ref mut p) = state.video_player {
            p.is_seeking = true;
        }
        assert!(is_hovering_video_controls(&state, Some(egui::pos2(320.0, 100.0)), window_rect));
        assert!(is_hovering_video_controls(&state, None, window_rect));
    }
}
