use super::networking::send_ws_command;
use crate::app::AppState;
use crate::devices::filter_type::CrtFilter;
use crate::video;
use eframe::egui;
use eframe::egui_glow;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::Ordering;

fn capture_frame_pixels_raw(
    gl: &eframe::glow::Context,
    area: crate::video::gpu::geometry::RenderedArea,
) -> Option<(Vec<u8>, u32, u32)> {
    crate::video::gpu::renderer::read_framebuffer_pixels_raw(gl, area)
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

    let is_gamescope = state.gamescope_hook.as_ref().map_or(false, |h| h.is_active);
    let is_pc98 = state.pc98.is_active;
    let is_pc88 = state.pc88.is_active;
    let mut passthrough_active = false;
    if is_gamescope {
        let toggle_passthrough =
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab));
        if toggle_passthrough {
            if let Some(hook) = &mut state.gamescope_hook {
                let _now_enabled = hook.toggle_passthrough();
                ctx.memory_mut(|m| {
                    m.stop_text_input();
                    if let Some(id) = m.focused() {
                        m.surrender_focus(id);
                    }
                });
                ctx.request_repaint();
            }
        }
        passthrough_active = state
            .gamescope_hook
            .as_ref()
            .map_or(false, |h| h.passthrough_enabled);
    } else if is_pc98 {
        let toggle_passthrough =
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab));
        if toggle_passthrough {
            let _now_enabled = state.pc98.toggle_passthrough();
            ctx.memory_mut(|m| {
                m.stop_text_input();
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            ctx.request_repaint();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F5)) {
            let slot = state.pc98.active_slot;
            state
                .pc98
                .send_command(crate::pc98::Pc98Command::SaveState { slot });
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F9)) {
            let slot = state.pc98.active_slot;
            state
                .pc98
                .send_command(crate::pc98::Pc98Command::LoadState { slot });
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F6)) {
            let slot = state.pc98.cycle_slot(-1);
            state.info(format!(
                "Selected PC-98 Save Slot {slot} (Shift+F5 Save / Shift+F9 Load)"
            ));
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F7)) {
            let slot = state.pc98.cycle_slot(1);
            state.info(format!(
                "Selected PC-98 Save Slot {slot} (Shift+F5 Save / Shift+F9 Load)"
            ));
        }
        passthrough_active = state.pc98.passthrough_enabled;
    } else if is_pc88 {
        let toggle_passthrough =
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab));
        if toggle_passthrough {
            let _now_enabled = state.pc88.toggle_passthrough();
            ctx.memory_mut(|m| {
                m.stop_text_input();
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            ctx.request_repaint();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F5)) {
            let slot = state.pc88.active_slot;
            state
                .pc88
                .send_command(crate::pc88::Pc88Command::SaveState { slot });
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F9)) {
            let slot = state.pc88.active_slot;
            state
                .pc88
                .send_command(crate::pc88::Pc88Command::LoadState { slot });
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F6)) {
            let slot = state.pc88.cycle_slot(-1);
            state.info(format!(
                "Selected PC-88 Save Slot {slot} (Shift+F5 Save / Shift+F9 Load)"
            ));
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F7)) {
            let slot = state.pc88.cycle_slot(1);
            state.info(format!(
                "Selected PC-88 Save Slot {slot} (Shift+F5 Save / Shift+F9 Load)"
            ));
        }
        passthrough_active = state.pc88.passthrough_enabled;
    }

    let mut new_dma_frame_received = false;
    state.poll_gamescope_dma_retry(ctx);
    if let Some(hook) = &mut state.gamescope_hook {
        if hook.is_active {
            hook.poll_late_connections();
            if let Some(dma) = &hook.dma_handle {
                while let Ok(frame) = dma.rx.try_recv() {
                    hook.current_dmabuf_frame = Some(std::sync::Arc::new(frame));
                    state.video_frames_since_last_check += 1;
                    new_dma_frame_received = true;
                }
            }
        }
    }

    // Space to capture OCR; Shift+Space to clear OCR boxes (suppressed when passthrough or constant scanning is active)
    if !passthrough_active
        && !state.ocr.is_constant_scanning_active()
        && !ctx.wants_keyboard_input()
        && ctx.input(|i| i.key_pressed(egui::Key::Space))
    {
        if ctx.input(|i| i.modifiers.shift) {
            if !state.ocr.boxes.is_empty() {
                state.clear_ocr();
            }
        } else {
            if !state.ocr.is_processing.load(Ordering::Relaxed) {
                state
                    .ocr
                    .backend_shared
                    .store(state.ocr.backend.as_u8(), Ordering::Release);
                state.ocr.capture_requested.store(true, Ordering::Release);
                state.info(format!(
                    "Capturing screen for {} OCR...",
                    state.ocr.backend.short_name()
                ));
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
        let non_focus_click = egui::Sense {
            click: true,
            drag: false,
            focusable: false,
        };
        let response = ui.allocate_response(ui.available_size(), non_focus_click);
        let video_texture = state
            .video_texture
            .as_ref()
            .expect("Video texture not initialized");
        let texture_size = video_texture.size_vec2();

        let filter = CrtFilter::from_u8(state.crt_filter.load(Ordering::Relaxed));
        let fft_filter_ref = if state.video.fft_filter_enabled {
            state.fft_filter.clone()
        } else {
            None
        };

        state
            .ocr
            .backend_shared
            .store(state.ocr.backend.as_u8(), Ordering::Release);
        let ocr_dispatch = state.ocr.dispatch_handle();
        let mining_capture_this_frame = state.bank.has_pending();
        if mining_capture_this_frame {
            ctx.request_repaint();
        }
        let bank_capture = state.bank.capture_handle();

        let is_preroll_active = state.twitch.preroll.is_some();
        let is_outro_active = state.twitch.outro.is_some();
        let is_roll_active = is_preroll_active || is_outro_active;
        let preroll_fade_in = state.twitch.is_preroll_in_fade_in();
        let outro_fade_out = state.twitch.is_outro_in_fade_out();
        let roll_fade_alpha = state.twitch.roll_fade_alpha();
        let preroll_remaining_secs = state.twitch.preroll_remaining_secs();
        let outro_remaining_secs = state.twitch.outro_remaining_secs();
        let is_showing_preroll = is_preroll_active && !preroll_fade_in;
        let is_showing_outro = is_outro_active && !outro_fade_out;
        let is_showing_roll = is_showing_preroll || is_showing_outro;
        let active_frame = if is_showing_preroll {
            state.twitch.preroll_frame()
        } else if is_showing_outro {
            state.twitch.outro_frame()
        } else {
            state.latest_frame.clone()
        };

        if is_roll_active {
            if let Some(ref r) = state.crt_renderer {
                let mut renderer = r.lock().unwrap();
                renderer.popup_primitives.clear();
                renderer.comment_primitives.clear();
                renderer.alert_primitives.clear();
            }
        }

        let ppp = ctx.pixels_per_point();
        let output_size = (response.rect.width() * ppp, response.rect.height() * ppp);
        let latest_frame_dim = active_frame
            .as_ref()
            .map(|f| (f.width, f.height))
            .or_else(|| {
                state
                    .gamescope_hook
                    .as_ref()
                    .and_then(|h| h.current_dmabuf_frame.as_ref().map(|f| (f.width, f.height)))
            });
        let res = latest_frame_dim
            .or_else(|| {
                if let Some(hook) = &state.gamescope_hook {
                    if hook.is_active {
                        return Some(hook.resolution());
                    }
                }
                if state.hardware.selected_resolution.0 > 0 {
                    Some(state.hardware.selected_resolution)
                } else {
                    None
                }
            })
            .unwrap_or((texture_size.x as u32, texture_size.y as u32));
        let h_stretch = if is_showing_roll {
            1.0
        } else {
            state.video.horizontal_stretch
        };
        let rendered_area_geom =
            crate::video::gpu::geometry::RenderedArea::fit(res, output_size, h_stretch);
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

        let is_window_focused = ctx.input(|i| i.focused);
        if is_gamescope && passthrough_active {
            let is_cursor_hidden = state
                .gamescope_hook
                .as_ref()
                .map_or(false, |h| h.is_cursor_hidden());
            if is_window_focused && is_cursor_hidden {
                ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::Locked));
                ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::None));
                ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
            }

            if let Some(ref mut hook) = state.gamescope_hook {
                if !is_window_focused {
                    // When window loses focus (e.g. Alt+Tab), release inputs to avoid stuck keys
                    hook.release_all_inputs();
                } else {
                    let target_res = hook.resolution();
                    forward_gamescope_inputs(hook, ctx, video_rect, target_res);
                }
            }
            // Consume key and text events so game inputs don't leak into egui widgets while passthrough is on
            ctx.input_mut(|i| {
                i.events
                    .retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_)));
            });
        } else if is_gamescope {
            ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::None));
            ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
        } else if is_pc98 && passthrough_active {
            let has_system_dialog = state.ui.show_quit_dialog
                || state.ui.show_stop_stream_dialog
                || state.ui.show_first_run_dialog
                || (state.config_load_error.is_some() && !state.ui.dismissed_config_error)
                || state.video.fft_mask_window_open
                || state.ui.debug_open;

            if is_window_focused && !has_system_dialog {
                ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::Locked));
                ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                let target_res = state.pc98.resolution;
                crate::pc98::input::forward_pc98_inputs(
                    &mut state.pc98,
                    ctx,
                    video_rect,
                    target_res,
                );
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::None));
                ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
                state.pc98.release_all_inputs();
            }
            ctx.input_mut(|i| {
                i.events
                    .retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_)));
            });
        } else if is_pc98 {
            ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::None));
            ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
        } else if is_pc88 && passthrough_active {
            let has_system_dialog = state.ui.show_quit_dialog
                || state.ui.show_stop_stream_dialog
                || state.ui.show_first_run_dialog
                || (state.config_load_error.is_some() && !state.ui.dismissed_config_error)
                || state.video.fft_mask_window_open
                || state.ui.debug_open;

            if is_window_focused && !has_system_dialog {
                ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::Locked));
                ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                let target_res = state.pc88.resolution;
                crate::pc98::input::forward_pc88_inputs(
                    &mut state.pc88,
                    ctx,
                    video_rect,
                    target_res,
                );
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::None));
                ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
                state.pc88.release_all_inputs();
            }
            ctx.input_mut(|i| {
                i.events
                    .retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_)));
            });
        } else if is_pc88 {
            ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::None));
            ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
        }

        // Keep UI surface rendering strictly tied to Gamescope's frame delivery in both active
        // and input-paused modes. This prevents 144Hz presentation spikes when hovering/interacting
        // with UI controls or moving the mouse while input is paused.
        if is_gamescope && !new_dma_frame_received {
            if let Some(ref mut hook) = state.gamescope_hook {
                if hook.is_active {
                    if let Some(dma) = &hook.dma_handle {
                        // Use a 25ms safety timeout (1.5 frame periods):
                        // Under active 60 FPS streaming, this unblocks immediately when Gamescope delivers
                        // the frame (~16.67ms cadence).
                        if let Ok(frame) = dma.rx.recv_timeout(std::time::Duration::from_millis(25))
                        {
                            hook.current_dmabuf_frame = Some(std::sync::Arc::new(frame));
                            state.video_frames_since_last_check += 1;
                            while let Ok(extra) = dma.rx.try_recv() {
                                hook.current_dmabuf_frame = Some(std::sync::Arc::new(extra));
                                state.video_frames_since_last_check += 1;
                            }
                        }
                    }
                }
            }
        }

        let (latest_dmabuf_frame, egl_importer_cb) = if let Some(hook) = &state.gamescope_hook {
            (hook.current_dmabuf_frame.clone(), hook.egl_importer.clone())
        } else {
            (None, None)
        };

        let cathode_params = video::gpu::CathodeInterferenceShaderParams::from_state(state);
        let time = ui.input(|i| i.time) as f32;
        let is_crt_on = !is_showing_roll && filter != CrtFilter::Off;
        let light_switch_flicker_active = state.video.advance_light_switch_flicker();
        let night_mode_active = is_crt_on && state.video.effective_lights_off_night_mode();
        let retro_pc_frame = is_crt_on && state.video.retro_pc_frame;
        let effective_dark_mode =
            is_crt_on && (state.video.retro_pc_frame_dark_mode || night_mode_active);
        let retro_pc_ambient_glow = if night_mode_active {
            (state.video.retro_pc_ambient_glow * 1.50).min(1.0)
        } else if is_showing_roll {
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

        let pointer_pos = ui.input(|i| {
            i.pointer.hover_pos().or_else(|| {
                if i.pointer.any_down() {
                    i.pointer.latest_pos()
                } else {
                    None
                }
            })
        });
        let is_on_crt_surface = pointer_pos.map_or(false, |pos| {
            crt_surface.contains(pos)
                && !state.twitch.overlay_contains(pos)
                && !state.whiteboard.contains_pointer(Some(pos), response.rect)
        });
        let is_hovering_controls = is_hovering_video_controls(state, pointer_pos, response.rect);
        let software_mouse_active =
            is_software_mouse_active(state, is_on_crt_surface) && !is_hovering_controls;
        let real_mouse_active = !software_mouse_active
            && is_window_focused
            && is_real_game_cursor_active(state, pointer_pos, video_rect, is_hovering_controls);
        let passthrough_hide_host_cursor =
            is_passthrough_host_cursor_hidden(state, pointer_pos, video_rect, is_hovering_controls);

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

        if software_mouse_active || real_mouse_active || passthrough_hide_host_cursor {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        }

        if state.cathode_interference.enabled
            || state.audio_reactive.enabled
            || retro_pc_frame
            || (night_mode_glow_intensity > 0.001)
            || light_switch_flicker_active
            || is_roll_active
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_nanos(1_000_000_000 / 60));
        }

        if !is_showing_roll
            && (state.video.pixelate_filter_enabled
                || filter != CrtFilter::Off
                || state.cathode_interference.enabled
                || state.audio_reactive.enabled)
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
                let audio_reactive_params = video::gpu::AudioReactiveShaderParams::from_state(
                    state,
                    warp,
                    corner_size,
                    filter_type,
                );
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

                let ocr_dispatch_cb = ocr_dispatch.clone();
                let bank_capture_cb = bank_capture.clone();

                let latest_dmabuf_frame_cb = latest_dmabuf_frame.clone();
                let egl_importer_cb = egl_importer_cb.clone();

                let capture_overlays = state.replay.config.capture_overlays;
                let webcam_enabled = state.webcam.config.enabled;
                let whiteboard_visible = !is_roll_active && state.whiteboard.is_visible();
                let stream_res = if let Some(hook) = &state.gamescope_hook {
                    if hook.is_active {
                        hook.resolution()
                    } else {
                        state.hardware.selected_resolution
                    }
                } else {
                    state.hardware.selected_resolution
                };

                let callback = egui::PaintCallback {
                    rect: response.rect,
                    callback: std::sync::Arc::new(egui_glow::CallbackFn::new(
                        move |_info, painter| {
                            let mut renderer = renderer_clone.lock().unwrap();
                            unsafe {
                                crate::gamescope::egl::drain_orphaned_dma_textures(painter.gl());
                            }
                            let output_size = (rect.width() * ppp, rect.height() * ppp);
                            let mut fallback_tex =
                                video_texture_id.and_then(|id| painter.texture(id));
                            if let (Some(importer_arc), Some(dma_frame)) =
                                (&egl_importer_cb, &latest_dmabuf_frame_cb)
                            {
                                if let Ok(mut importer) = importer_arc.lock() {
                                    unsafe {
                                        if let Some(imported_tex) = importer.import_dmabuf(
                                            painter.gl(),
                                            dma_frame.buffer_id,
                                            dma_frame.fd.as_raw_fd(),
                                            dma_frame.width,
                                            dma_frame.height,
                                            dma_frame.stride,
                                            dma_frame.drm_fourcc,
                                            dma_frame.modifier,
                                            dma_frame.captured_at,
                                        ) {
                                            fallback_tex = Some(imported_tex);
                                        }
                                    }
                                }
                            }

                            let res = latest_frame
                                .as_ref()
                                .map(|f| (f.width, f.height))
                                .or_else(|| {
                                    latest_dmabuf_frame_cb.as_ref().map(|f| (f.width, f.height))
                                })
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

                            renderer.capture_ocr_next_paint =
                                ocr_dispatch_cb.take_capture_requested();
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
                                Some(&audio_reactive_params),
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
                                .or_else(|| latest_dmabuf_frame_cb.as_ref().map(|f| f.captured_at))
                                .unwrap_or(0);
                            if let Some((pixels, w, h)) = renderer.last_ocr_capture.take() {
                                ocr_dispatch_cb.dispatch(pixels, w, h);
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

                            if !whiteboard_visible {
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
            let background_color = if is_showing_roll {
                [0.0, 0.0, 0.0]
            } else if state.video.use_magenta_background {
                [1.0, 0.0, 1.0]
            } else {
                [0.0, 0.0, 0.0]
            };
            let horizontal_stretch = if is_showing_roll {
                1.0
            } else {
                state.video.horizontal_stretch
            };
            let median_filter_enabled = !is_showing_roll && state.video.median_filter_enabled;
            let median_mix = if is_showing_roll {
                0.0
            } else {
                state.video.median_mix
            };
            let deinterlace_filter_enabled =
                !is_showing_roll && state.video.deinterlace_filter_enabled;
            let deinterlace_mode = if is_showing_roll {
                0
            } else {
                state.video.deinterlace_mode
            };
            let deinterlace_blend = if is_showing_roll {
                0.0
            } else {
                state.video.deinterlace_blend
            };
            let deinterlace_motion_threshold = if is_showing_roll {
                0.0
            } else {
                state.video.deinterlace_motion_threshold
            };
            let deinterlace_line_spacing = if is_showing_roll {
                1.0
            } else {
                state.video.deinterlace_line_spacing
            };
            let deinterlace_spatial_mix = if is_showing_roll {
                0.0
            } else {
                state.video.deinterlace_spatial_mix
            };
            let vibrance = if is_showing_roll {
                0.0
            } else {
                state.video.vibrance
            };
            let overscan_x = if is_showing_roll {
                0.0
            } else {
                state.video.overscan_x
            };
            let overscan_y = if is_showing_roll {
                0.0
            } else {
                state.video.overscan_y
            };
            let underscan_x = if is_showing_roll {
                0.0
            } else {
                state.video.underscan_x
            };
            let underscan_y = if is_showing_roll {
                0.0
            } else {
                state.video.underscan_y
            };
            let border_crop = if is_showing_roll {
                [0.0, 0.0, 0.0, 0.0]
            } else {
                [
                    state.video.border_crop_left,
                    state.video.border_crop_right,
                    state.video.border_crop_top,
                    state.video.border_crop_bottom,
                ]
            };
            let retro_pc_ambient_glow = if is_showing_roll {
                0.0
            } else {
                retro_pc_ambient_glow
            };
            let scaler_filter = if is_showing_roll {
                0
            } else {
                state.scaler_filter.load(Ordering::Relaxed)
            };
            let latest_frame = active_frame.clone();
            let video_texture_id = state.video_texture.as_ref().map(|t| t.id());
            let fft_clone = if is_showing_roll {
                None
            } else {
                fft_filter_ref.clone()
            };
            let fft_threshold = if is_showing_roll {
                0.0
            } else {
                state.fft_mask_threshold
            };
            let fft_black = if is_showing_roll {
                0.0
            } else {
                state.fft_black_threshold
            };
            let glass_params = if is_showing_roll {
                None
            } else {
                Some(video::gpu::GlassShaderParams::from_state(
                    state,
                    [0.0, 0.0],
                    0.0,
                    0,
                ))
            };

            let replay_gpu = state.replay.gpu.clone();
            let replay = state
                .replay
                .runtime
                .as_ref()
                .map(crate::replay::gpu::RuntimeView::new);

            let ocr_dispatch_cb = ocr_dispatch.clone();
            let bank_capture_cb = bank_capture.clone();

            let latest_dmabuf_frame_cb = latest_dmabuf_frame.clone();
            let egl_importer_cb = egl_importer_cb.clone();

            let capture_overlays = state.replay.config.capture_overlays;
            let webcam_enabled = state.webcam.config.enabled;
            let whiteboard_visible = !is_roll_active && state.whiteboard.is_visible();
            let stream_res = if let Some(hook) = &state.gamescope_hook {
                if hook.is_active {
                    hook.resolution()
                } else {
                    state.hardware.selected_resolution
                }
            } else {
                state.hardware.selected_resolution
            };

            let callback = egui::PaintCallback {
                rect,
                callback: std::sync::Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                    unsafe {
                        crate::gamescope::egl::drain_orphaned_dma_textures(painter.gl());
                    }
                    let mut fallback_tex = video_texture_id.and_then(|id| painter.texture(id));
                    if let (Some(importer_arc), Some(dma_frame)) =
                        (&egl_importer_cb, &latest_dmabuf_frame_cb)
                    {
                        if let Ok(mut importer) = importer_arc.lock() {
                            unsafe {
                                if let Some(imported_tex) = importer.import_dmabuf(
                                    painter.gl(),
                                    dma_frame.buffer_id,
                                    dma_frame.fd.as_raw_fd(),
                                    dma_frame.width,
                                    dma_frame.height,
                                    dma_frame.stride,
                                    dma_frame.drm_fourcc,
                                    dma_frame.modifier,
                                    dma_frame.captured_at,
                                ) {
                                    fallback_tex = Some(imported_tex);
                                }
                            }
                        }
                    }

                    let res = latest_frame
                        .as_ref()
                        .map(|f| (f.width, f.height))
                        .or_else(|| latest_dmabuf_frame_cb.as_ref().map(|f| (f.width, f.height)))
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
                    let (rendered_area, ocr_captured) = {
                        let mut renderer = renderer_clone.lock().unwrap();
                        renderer.capture_ocr_next_paint = ocr_dispatch_cb.take_capture_requested();
                        let area = renderer.draw_passthrough(
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
                            None,
                            glass_params.as_ref(),
                            0.0,
                            current_software_mouse_pos,
                            current_software_mouse_clip,
                            is_mining_capture,
                        );
                        (area, renderer.last_ocr_capture.take())
                    };
                    let at = latest_frame
                        .as_ref()
                        .map(|f| f.captured_at)
                        .or_else(|| latest_dmabuf_frame_cb.as_ref().map(|f| f.captured_at))
                        .unwrap_or(0);
                    if let Some((pixels, w, h)) = ocr_captured {
                        ocr_dispatch_cb.dispatch(pixels, w, h);
                    }

                    if retro_pc_frame && !is_showing_roll {
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

                    if !whiteboard_visible {
                        if let Some(request) = bank_capture_cb.take_pending() {
                            let shot = capture_frame_pixels(painter.gl(), full_or_rendered_area);
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
                    }
                })),
            };
            ui.painter().add(callback);
        }

        // Draw interactive OCR bounding box overlay on top of the video image
        if !is_roll_active && !passthrough_active {
            crate::ocr::overlay::draw_ocr_overlay(ui, state, video_rect);
        } else {
            state.ocr.is_box_hovered = false;
        }

        // Niconico chat comments, drawn by the renderer under the CRT effects.
        if !is_roll_active {
            crate::ui::niconico::update(state, ctx, video_rect);
        }

        // Video alert notices, drawn by the renderer under the CRT effects.
        if !is_roll_active {
            crate::ui::alerts::update(state, ctx, video_rect);
        }

        // Extra widget for local video player mode (scrub bar, play/pause/stop)
        if state.video_player.is_some() {
            draw_video_file_controls(state, ui, ctx, response.rect);
        }

        if (software_mouse_active || real_mouse_active || passthrough_hide_host_cursor)
            && !is_roll_active
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        }

        // Draw resizable whiteboard on Order::PanelResizeLine (above CRT borders, Niconico comments,
        // OCR boxes, and dictionary popups; below Webcam overlay, Debug window, and Twitch chat).
        if !is_roll_active {
            crate::ui::whiteboard::draw_whiteboard(state, ctx, response.rect);
            if state.whiteboard.is_visible() && mining_capture_this_frame {
                let bank_capture_cb = bank_capture.clone();
                let repaint_ctx = ctx.clone();
                let full_window_effects = retro_pc_frame || night_mode_glow_intensity > 0.001;
                let capture_area = if full_window_effects {
                    crate::video::gpu::geometry::RenderedArea::full(output_size)
                } else {
                    rendered_area_geom
                };
                let wb_capture_cb = egui::PaintCallback {
                    rect: response.rect,
                    callback: std::sync::Arc::new(egui_glow::CallbackFn::new(
                        move |_info, painter| {
                            if let Some(request) = bank_capture_cb.take_pending() {
                                let shot = capture_frame_pixels(painter.gl(), capture_area);
                                bank_capture_cb.save(request, shot);
                                repaint_ctx.request_repaint();
                            }
                        },
                    )),
                };
                ctx.layer_painter(egui::LayerId::new(
                    egui::Order::PanelResizeLine,
                    egui::Id::new("whiteboard_overlay"),
                ))
                .add(wb_capture_cb);
            } else if state.whiteboard.is_visible() && state.bank.has_pending() {
                ctx.request_repaint();
            }
        }

        // Draw webcam overlay on top of everything (suppressed during pre-roll and outro)
        if !is_roll_active {
            crate::ui::webcam_overlay::draw_webcam_overlay(state, ui, ctx, response.rect);
        }

        // Draw real Gamescope cursor if active
        if real_mouse_active && !is_roll_active {
            if let Some(hook) = &mut state.gamescope_hook {
                if let Some(img) = hook.cursor_image() {
                    let needs_new_texture = match &hook.cursor_texture {
                        Some((serial, _)) => *serial != img.cursor_serial,
                        None => true,
                    };
                    if needs_new_texture {
                        let color_image = egui::ColorImage::from_rgba_premultiplied(
                            [img.width as usize, img.height as usize],
                            &img.rgba_pixels,
                        );
                        let texture = ctx.load_texture(
                            "gamescope_game_cursor",
                            color_image,
                            egui::TextureOptions::LINEAR,
                        );
                        hook.cursor_texture = Some((img.cursor_serial, texture));
                    }

                    if let Some(pos) = pointer_pos {
                        if let Some((_, texture)) = &hook.cursor_texture {
                            let target_res = hook.resolution();
                            let scale_x = if target_res.0 > 0 {
                                video_rect.width() / target_res.0 as f32
                            } else {
                                1.0
                            };
                            let scale_y = if target_res.1 > 0 {
                                video_rect.height() / target_res.1 as f32
                            } else {
                                1.0
                            };
                            let cursor_w = img.width as f32 * scale_x;
                            let cursor_h = img.height as f32 * scale_y;
                            let hot_x = img.xhot as f32 * scale_x;
                            let hot_y = img.yhot as f32 * scale_y;
                            let top_left = egui::pos2(pos.x - hot_x, pos.y - hot_y);
                            let cursor_rect =
                                egui::Rect::from_min_size(top_left, egui::vec2(cursor_w, cursor_h));

                            ui.painter().with_clip_rect(response.rect).image(
                                texture.id(),
                                cursor_rect,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );
                            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                        }
                    }
                }
            }
        }

        // During pre-roll / outro fade out / fade in transition, render a black overlay across the video
        // surface. This is rendered on the UI painter so it will be captured into the livestream
        // broadcast by the replay_gpu.capture callback below.
        if is_roll_active && roll_fade_alpha > 0.001 {
            let black_alpha = (roll_fade_alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
            ui.painter().rect_filled(
                response.rect,
                0.0,
                egui::Color32::from_black_alpha(black_alpha),
            );
        }

        // When capturing overlays in the replay buffer, when webcam or whiteboard is visible, or when broadcasting to Twitch,
        // capture the frame after egui has rendered the interactive OCR boxes, dictionary popup, whiteboard, and webcam overlay.
        let is_broadcasting = state.twitch.is_broadcasting() || state.twitch.broadcast.is_some();
        let whiteboard_visible = !is_roll_active && state.whiteboard.is_visible();
        let capture_replay_here = state.replay.config.capture_overlays
            || state.webcam.config.enabled
            || whiteboard_visible;
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
            let broadcast_sink =
                state
                    .twitch
                    .broadcast
                    .as_ref()
                    .map(|b| crate::replay::gpu::BroadcastSink {
                        tx: b.video_tx.clone(),
                        free_rx: b.video_free_rx.clone(),
                        dropped: b.metrics.video_dropped.clone(),
                    });
            let at = active_frame
                .as_ref()
                .map(|f| f.captured_at)
                .unwrap_or_else(crate::replay::now_us);
            let full_window_effects = retro_pc_frame || night_mode_glow_intensity > 0.001;
            let rendered_area = if full_window_effects {
                crate::video::gpu::geometry::RenderedArea::full(output_size)
            } else {
                rendered_area_geom
            };

            let callback = egui::PaintCallback {
                rect: response.rect,
                callback: std::sync::Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                    replay_gpu.lock().unwrap().capture(
                        painter.gl(),
                        replay.as_ref(),
                        broadcast_sink.as_ref(),
                        rendered_area,
                        at,
                    );
                })),
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
                draw_roll_counter_preview(ctx, response.rect, rem_secs, "PRE-ROLL (PREVIEW)");
            }
        } else if is_outro_active {
            if let Some(rem_secs) = outro_remaining_secs {
                draw_roll_counter_preview(ctx, response.rect, rem_secs, "OUTRO (PREVIEW)");
            }
        }

        if !passthrough_active
            && response.double_clicked()
            && !state
                .whiteboard
                .contains_pointer(pointer_pos, response.rect)
        {
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
    if state.twitch.is_roll_active() {
        return false;
    }
    if state
        .gamescope_hook
        .as_ref()
        .map_or(false, |h| h.is_active && h.is_cursor_hidden())
    {
        return false;
    }
    if (state.pc98.is_active && state.pc98.passthrough_enabled)
        || (state.pc88.is_active && state.pc88.passthrough_enabled)
    {
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
        && !state.whiteboard.is_interacting()
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

/// Determines if the host OS hardware cursor should be hidden over the video surface
/// while Gamescope passthrough is active (e.g. when the game renders its own software cursor).
pub fn is_passthrough_host_cursor_hidden(
    state: &AppState,
    pointer_pos: Option<egui::Pos2>,
    video_rect: egui::Rect,
    is_hovering_controls: bool,
) -> bool {
    if state.twitch.is_roll_active() {
        return false;
    }
    let retro_pc_passthrough = (state.pc98.is_active && state.pc98.passthrough_enabled)
        || (state.pc88.is_active && state.pc88.passthrough_enabled);
    let passthrough_on = state
        .gamescope_hook
        .as_ref()
        .map_or(false, |h| h.is_active && h.passthrough_enabled)
        || retro_pc_passthrough;
    if !passthrough_on || is_hovering_controls {
        return false;
    }
    if !retro_pc_passthrough {
        let Some(pos) = pointer_pos else {
            return false;
        };
        if !video_rect.contains(pos) {
            return false;
        }
    }

    let has_system_dialog = state.ui.show_quit_dialog
        || state.ui.show_stop_stream_dialog
        || state.ui.show_first_run_dialog
        || (state.config_load_error.is_some() && !state.ui.dismissed_config_error)
        || state.video.fft_mask_window_open
        || state.ui.debug_open;

    if has_system_dialog {
        return false;
    }

    let popup_active = state.dict.popup.is_some();
    let popup_under_crt = state.dict.popup_under_crt;
    if popup_active && !popup_under_crt {
        return false;
    }

    if state.webcam.config.enabled
        && (state.webcam.is_dragging || state.webcam.resizing_corner.is_some())
    {
        return false;
    }

    if state.whiteboard.is_interacting()
        || state.whiteboard.contains_pointer(pointer_pos, video_rect)
    {
        return false;
    }

    true
}

/// Determines if the real game mouse cursor extracted from Gamescope/X11 should be active.
/// Returns false if Gamescope is inactive, passthrough is off, game cursor is hidden,
/// no cursor image is available, mouse is outside video rect, hovering controls,
/// or if system dialogs are open.
pub fn is_real_game_cursor_active(
    state: &AppState,
    pointer_pos: Option<egui::Pos2>,
    video_rect: egui::Rect,
    is_hovering_controls: bool,
) -> bool {
    let Some(hook) = &state.gamescope_hook else {
        return false;
    };
    if hook.is_cursor_hidden() || hook.cursor_image().is_none() {
        return false;
    }
    is_passthrough_host_cursor_hidden(state, pointer_pos, video_rect, is_hovering_controls)
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
                        let seek_width =
                            (remaining_width - exit_btn_width - item_spacing).max(60.0);

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
                                let fraction = ((mouse_pos.x - seek_rect.min.x)
                                    / seek_rect.width())
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
                                let fraction = ((mouse_pos.x - seek_rect.min.x)
                                    / seek_rect.width())
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
                                let fraction = ((mouse_pos.x - seek_rect.min.x)
                                    / seek_rect.width())
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
                            let hover_fraction = ((hover_pos.x - seek_rect.min.x)
                                / seek_rect.width())
                            .clamp(0.0, 1.0);
                            let hover_secs = hover_fraction as f64 * duration;
                            seek_resp.clone().on_hover_ui_at_pointer(|ui| {
                                ui.label(
                                    egui::RichText::new(
                                        crate::video::player::format_time_with_duration(
                                            hover_secs, duration,
                                        ),
                                    )
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
fn draw_roll_counter_preview(
    ctx: &egui::Context,
    player_rect: egui::Rect,
    rem_secs: f64,
    label_text: &str,
) {
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
        label_text,
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

/// Forwards pointer, button, scroll, keyboard, and modifier inputs to Gamescope virtual input via libei.
pub fn forward_gamescope_inputs(
    hook: &mut crate::gamescope::GamescopeHookState,
    ctx: &egui::Context,
    video_rect: egui::Rect,
    target_res: (u32, u32),
) {
    if !hook.is_active || !hook.passthrough_enabled {
        return;
    }

    let is_cursor_hidden = hook.is_cursor_hidden();

    let Some(client) = &mut hook.input_client else {
        return;
    };

    // Ensure incoming EIS events (seat added, device resumed, etc.) are processed
    client.dispatch();

    // Forward pointer motion:
    if is_cursor_hidden {
        // When game hides cursor (camera control / mouselook): send relative motion deltas.
        // Under CursorGrab::Locked, egui's pointer position is frozen so pointer.delta() is (0,0).
        // The raw movement is reported via pointer.motion() (fed by DeviceEvent::MouseMotion).
        let raw_motion = ctx.input(|i| {
            if let Some(m) = i.pointer.motion() {
                if m.x != 0.0 || m.y != 0.0 {
                    return m;
                }
            }
            i.pointer.delta()
        });
        if raw_motion.x != 0.0 || raw_motion.y != 0.0 {
            let scale_x = target_res.0 as f64 / video_rect.width().max(1.0) as f64;
            let scale_y = target_res.1 as f64 / video_rect.height().max(1.0) as f64;
            client
                .send_pointer_motion(raw_motion.x as f64 * scale_x, raw_motion.y as f64 * scale_y);
        }
    } else {
        // In standard mode (Slay the Spire 2, RPGs, strategy):
        // Stream absolute coordinates on hover and drag
        let is_any_button_down = ctx.input(|i| i.pointer.any_down());
        if let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) {
            if video_rect.contains(pos) || is_any_button_down {
                let rel_x = ((pos.x - video_rect.min.x) / video_rect.width()).clamp(0.0, 1.0);
                let rel_y = ((pos.y - video_rect.min.y) / video_rect.height()).clamp(0.0, 1.0);
                let gs_x = rel_x as f64 * target_res.0 as f64;
                let gs_y = rel_y as f64 * target_res.1 as f64;
                client.send_pointer_motion_absolute(gs_x, gs_y);
            }
        }
    }

    // Forward modifiers transition
    let current_modifiers = ctx.input(|i| i.modifiers);
    if current_modifiers.shift != hook.prev_modifiers.shift {
        client.send_key(42, current_modifiers.shift);
    }
    if current_modifiers.ctrl != hook.prev_modifiers.ctrl {
        client.send_key(29, current_modifiers.ctrl);
    }
    if current_modifiers.alt != hook.prev_modifiers.alt {
        client.send_key(56, current_modifiers.alt);
    }
    if current_modifiers.mac_cmd != hook.prev_modifiers.mac_cmd {
        client.send_key(125, current_modifiers.mac_cmd);
    }
    hook.prev_modifiers = current_modifiers;

    // Forward discrete events
    let events = ctx.input(|i| i.events.clone());
    for event in events {
        match event {
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                ..
            } => {
                let btn_code = match button {
                    egui::PointerButton::Primary => crate::gamescope::input::BTN_LEFT,
                    egui::PointerButton::Secondary => crate::gamescope::input::BTN_RIGHT,
                    egui::PointerButton::Middle => crate::gamescope::input::BTN_MIDDLE,
                    _ => crate::gamescope::input::BTN_LEFT,
                };
                if is_cursor_hidden {
                    // When game hides cursor for camera control, cursor is locked and hidden.
                    // Forward buttons directly without warping to stale screen coordinates or dropping clicks.
                    client.send_button(btn_code, pressed);
                } else if pressed {
                    if video_rect.contains(pos) {
                        let rel_x =
                            ((pos.x - video_rect.min.x) / video_rect.width()).clamp(0.0, 1.0);
                        let rel_y =
                            ((pos.y - video_rect.min.y) / video_rect.height()).clamp(0.0, 1.0);
                        let gs_x = rel_x as f64 * target_res.0 as f64;
                        let gs_y = rel_y as f64 * target_res.1 as f64;
                        client.send_pointer_motion_absolute(gs_x, gs_y);
                        client.send_button(btn_code, true);
                    }
                } else {
                    client.send_button(btn_code, false);
                }
            }
            egui::Event::MouseWheel { unit, delta, .. } => {
                let is_hovered = ctx
                    .input(|i| i.pointer.latest_pos())
                    .map_or(false, |pos| video_rect.contains(pos));
                if is_cursor_hidden || is_hovered {
                    // Gamescope passes (dy * 120.0) as discrete units to wlr_seat_pointer_notify_axis.
                    // In Wayland/libei, positive dy is scroll DOWN, negative dy is scroll UP.
                    // In egui, delta.y > 0 is scroll UP, so we invert delta.y.
                    let (scroll_x, scroll_y) = match unit {
                        egui::MouseWheelUnit::Line => (delta.x as f64, -delta.y as f64),
                        egui::MouseWheelUnit::Point => {
                            // Touchpad / smooth scroll: 50 points per notch
                            (delta.x as f64 / 50.0, -delta.y as f64 / 50.0)
                        }
                        egui::MouseWheelUnit::Page => (delta.x as f64 * 5.0, -delta.y as f64 * 5.0),
                    };
                    client.send_scroll(scroll_x, scroll_y);
                }
            }
            egui::Event::Key {
                key,
                pressed,
                modifiers,
                ..
            } => {
                // Ignore Shift+Tab (handled as passthrough toggle)
                if key == egui::Key::Tab && modifiers.shift {
                    continue;
                }
                if let Some(code) = crate::gamescope::input::egui_key_to_evdev(key) {
                    client.send_key(code, pressed);
                }
            }
            _ => {}
        }
    }

    client.dispatch();
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

        // Gamescope hook interactions:
        state.gamescope_hook = Some(crate::gamescope::GamescopeHookState {
            is_active: true,
            passthrough_enabled: true,
            ..Default::default()
        });
        // Active in standard passthrough mode
        assert!(is_software_mouse_active(&state, true));

        // Active in input-paused mode (passthrough disabled)
        if let Some(h) = &mut state.gamescope_hook {
            h.passthrough_enabled = false;
        }
        assert!(is_software_mouse_active(&state, true));
    }

    #[test]
    fn test_is_hovering_video_controls() {
        let mut state = AppState::default();
        let window_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(640.0, 480.0));

        // When no video player active, always false
        assert!(!is_hovering_video_controls(
            &state,
            Some(egui::pos2(320.0, 460.0)),
            window_rect
        ));

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
        assert!(!is_hovering_video_controls(
            &state,
            Some(egui::pos2(320.0, 240.0)),
            window_rect
        ));
        assert!(!is_hovering_video_controls(
            &state,
            Some(egui::pos2(320.0, 100.0)),
            window_rect
        ));

        // Near bottom of screen within hover zone -> true
        assert!(is_hovering_video_controls(
            &state,
            Some(egui::pos2(320.0, 460.0)),
            window_rect
        ));
        assert!(is_hovering_video_controls(
            &state,
            Some(egui::pos2(320.0, 440.0)),
            window_rect
        ));

        // When actively seeking/dragging, remains true even if cursor moves away
        if let Some(ref mut p) = state.video_player {
            p.is_seeking = true;
        }
        assert!(is_hovering_video_controls(
            &state,
            Some(egui::pos2(320.0, 100.0)),
            window_rect
        ));
        assert!(is_hovering_video_controls(&state, None, window_rect));
    }

    #[test]
    fn test_real_game_cursor_and_precedence() {
        let mut state = AppState::default();
        let video_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1080.0));
        let cursor_pos = Some(egui::pos2(960.0, 540.0));

        let dummy_cursor = crate::gamescope::x11_cursor::GameCursorImage {
            width: 32,
            height: 32,
            xhot: 0,
            yhot: 0,
            cursor_serial: 1,
            rgba_pixels: vec![255; 32 * 32 * 4],
        };

        // 1. Without Gamescope hook -> real cursor inactive
        assert!(!is_real_game_cursor_active(
            &state, cursor_pos, video_rect, false
        ));

        // 2. Set up Gamescope hook with visible cursor image
        let monitor = crate::gamescope::x11_cursor::GamescopeX11CursorMonitor::dummy_for_test(
            false,
            Some(dummy_cursor.clone()),
        );
        state.gamescope_hook = Some(crate::gamescope::GamescopeHookState {
            is_active: true,
            passthrough_enabled: true,
            x11_cursor_monitor: Some(monitor),
            ..Default::default()
        });

        // Real cursor should be active in normal playback
        assert!(is_real_game_cursor_active(
            &state, cursor_pos, video_rect, false
        ));

        // When mouse is hovering controls -> inactive
        assert!(!is_real_game_cursor_active(
            &state, cursor_pos, video_rect, true
        ));

        // When mouse is outside video rect -> inactive
        assert!(!is_real_game_cursor_active(
            &state,
            Some(egui::pos2(2000.0, 500.0)),
            video_rect,
            false
        ));
        assert!(!is_real_game_cursor_active(&state, None, video_rect, false));

        // When passthrough is disabled -> inactive
        if let Some(h) = &mut state.gamescope_hook {
            h.passthrough_enabled = false;
        }
        assert!(!is_real_game_cursor_active(
            &state, cursor_pos, video_rect, false
        ));
        if let Some(h) = &mut state.gamescope_hook {
            h.passthrough_enabled = true;
        }

        // When cursor is hidden by game (camera rotation / mouselook) -> inactive
        if let Some(h) = &state.gamescope_hook {
            if let Some(m) = &h.x11_cursor_monitor {
                m.set_hidden_for_test(true);
            }
        }
        assert!(!is_real_game_cursor_active(
            &state, cursor_pos, video_rect, false
        ));
        if let Some(h) = &state.gamescope_hook {
            if let Some(m) = &h.x11_cursor_monitor {
                m.set_hidden_for_test(false);
            }
        }
        assert!(is_real_game_cursor_active(
            &state, cursor_pos, video_rect, false
        ));

        // When system dialog is open -> inactive
        state.ui.show_quit_dialog = true;
        assert!(!is_real_game_cursor_active(
            &state, cursor_pos, video_rect, false
        ));
        state.ui.show_quit_dialog = false;

        // PRECEDENCE TESTS:
        // Rule: If retro mouse is enabled AND CRT filters are enabled, retro mouse takes precedence.
        // Otherwise, render the real mouse.

        let is_on_crt_surface = true;
        let is_hovering_controls = false;
        let is_window_focused = true;

        // Case A: Retro mouse enabled + CRT enabled (Lottes)
        state.video.retro_software_mouse = true;
        state.crt_filter.store(1, Ordering::Relaxed);
        let software_mouse_active =
            is_software_mouse_active(&state, is_on_crt_surface) && !is_hovering_controls;
        let real_mouse_active = !software_mouse_active
            && is_window_focused
            && is_real_game_cursor_active(&state, cursor_pos, video_rect, is_hovering_controls);
        assert!(software_mouse_active, "Retro mouse must be active");
        assert!(
            !real_mouse_active,
            "Retro mouse must take precedence over real mouse"
        );

        // Case B: Retro mouse disabled + CRT enabled (Lottes)
        state.video.retro_software_mouse = false;
        state.crt_filter.store(1, Ordering::Relaxed);
        let software_mouse_active =
            is_software_mouse_active(&state, is_on_crt_surface) && !is_hovering_controls;
        let real_mouse_active = !software_mouse_active
            && is_window_focused
            && is_real_game_cursor_active(&state, cursor_pos, video_rect, is_hovering_controls);
        assert!(!software_mouse_active, "Retro mouse must be inactive");
        assert!(
            real_mouse_active,
            "Real mouse must be active when retro mouse is disabled"
        );

        // Case C: Retro mouse enabled + CRT filter OFF
        state.video.retro_software_mouse = true;
        state.crt_filter.store(0, Ordering::Relaxed);
        let software_mouse_active =
            is_software_mouse_active(&state, is_on_crt_surface) && !is_hovering_controls;
        let real_mouse_active = !software_mouse_active
            && is_window_focused
            && is_real_game_cursor_active(&state, cursor_pos, video_rect, is_hovering_controls);
        assert!(
            !software_mouse_active,
            "Retro mouse must be inactive when CRT is off"
        );
        assert!(
            real_mouse_active,
            "Real mouse must be active when CRT is off"
        );

        // Case D: Retro mouse disabled + CRT filter OFF
        state.video.retro_software_mouse = false;
        state.crt_filter.store(0, Ordering::Relaxed);
        let software_mouse_active =
            is_software_mouse_active(&state, is_on_crt_surface) && !is_hovering_controls;
        let real_mouse_active = !software_mouse_active
            && is_window_focused
            && is_real_game_cursor_active(&state, cursor_pos, video_rect, is_hovering_controls);
        assert!(!software_mouse_active, "Retro mouse must be inactive");
        assert!(real_mouse_active, "Real mouse must be active");

        // Case E: Camera rotation mode (cursor hidden)
        if let Some(h) = &state.gamescope_hook {
            if let Some(m) = &h.x11_cursor_monitor {
                m.set_hidden_for_test(true);
            }
        }
        state.video.retro_software_mouse = true;
        state.crt_filter.store(1, Ordering::Relaxed);
        let software_mouse_active =
            is_software_mouse_active(&state, is_on_crt_surface) && !is_hovering_controls;
        let real_mouse_active = !software_mouse_active
            && is_window_focused
            && is_real_game_cursor_active(&state, cursor_pos, video_rect, is_hovering_controls);
        assert!(
            !software_mouse_active,
            "Retro mouse must NOT render during camera rotation"
        );
        assert!(
            !real_mouse_active,
            "Real mouse must NOT render during camera rotation"
        );
        assert!(
            is_passthrough_host_cursor_hidden(&state, cursor_pos, video_rect, is_hovering_controls),
            "Host hardware cursor must be hidden over video_rect during active passthrough"
        );

        // Case F: Game renders its own software cursor without an X11 cursor monitor
        if let Some(h) = &mut state.gamescope_hook {
            h.x11_cursor_monitor = None;
            h.passthrough_enabled = true;
        }
        assert!(
            is_passthrough_host_cursor_hidden(&state, cursor_pos, video_rect, false),
            "Host hardware cursor must be hidden in passthrough mode even without X11 cursor image"
        );
        if let Some(h) = &mut state.gamescope_hook {
            h.passthrough_enabled = false;
        }
        assert!(
            !is_passthrough_host_cursor_hidden(&state, cursor_pos, video_rect, false),
            "Host hardware cursor must be restored when passthrough is paused"
        );
    }
}
