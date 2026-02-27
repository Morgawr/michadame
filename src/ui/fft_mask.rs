use crate::app::AppState;
use crate::config;
use eframe::egui;
use eframe::egui_glow;

/// Draw the FFT mask editor window.
/// Returns true if the mask was modified this frame.
pub fn draw_fft_mask_editor(state: &mut AppState, ctx: &egui::Context) -> bool {
    // Auto-hide when FFT filter is disabled
    if !state.video.fft_filter_enabled {
        return false;
    }

    if !state.video.fft_mask_window_open {
        return false;
    }

    let mut mask_changed = false;
    let mut window_open = state.video.fft_mask_window_open;

    egui::Window::new("FFT Mask Editor")
        .open(&mut window_open)
        .resizable(true)
        .default_size([512.0, 600.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Brush size:");
                ui.add(egui::Slider::new(&mut state.fft_brush_radius, 1.0..=64.0).logarithmic(true));
            });

            ui.horizontal(|ui| {
                if ui.button("Clear Mask (Block All)").clicked() {
                    let (w, h) = state.fft_mask_resolution;
                    if w > 0 && h > 0 {
                        state.fft_mask_data = vec![0u8; (w * h) as usize];
                        mask_changed = true;
                    }
                }
                if ui.button("Fill Mask (Pass All)").clicked() {
                    let (w, h) = state.fft_mask_resolution;
                    if w > 0 && h > 0 {
                        state.fft_mask_data = vec![255u8; (w * h) as usize];
                        mask_changed = true;
                    }
                }
            });

            ui.horizontal(|ui| {
                ui.label("Threshold:");
                let mut pct = state.fft_mask_threshold * 100.0;
                if ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%")).changed() {
                    state.fft_mask_threshold = pct / 100.0;
                }
            }).response.on_hover_text("Only block frequencies brighter than this threshold.\n0% = block all masked, 100% = block only the brightest peaks.");

            ui.horizontal(|ui| {
                ui.label("Black skip:");
                let mut pct = state.fft_black_threshold * 100.0;
                if ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%")).changed() {
                    state.fft_black_threshold = pct / 100.0;
                }
            }).response.on_hover_text("Skip FFT filtering for dark areas.\nPixels whose 9×9 neighborhood average brightness\nis below this level use the original unfiltered image.\n0% = apply everywhere, higher = skip darker regions.");

            ui.separator();

            // === Save/Load masks ===
            let (fft_w, fft_h) = state.fft_mask_resolution;
            let has_frame = fft_w > 0 && fft_h > 0;
            let stream_res = state.latest_frame.as_ref()
                .map(|f| (f.width, f.height))
                .unwrap_or((0, 0));

            if has_frame && stream_res.0 > 0 {
                ui.horizontal(|ui| {
                    ui.label("Mask name:");
                    ui.text_edit_singleline(&mut state.fft_mask_save_name);
                    if ui.button("💾 Save").clicked() && !state.fft_mask_save_name.is_empty() {
                        match config::fft_masks::save_mask(
                            &state.fft_mask_save_name,
                            stream_res,
                            (fft_w, fft_h),
                            &state.fft_mask_data,
                            state.fft_mask_threshold,
                            state.fft_black_threshold,
                        ) {
                            Ok(()) => {
                                state.info(format!("Saved FFT mask '{}'", state.fft_mask_save_name));
                                // Refresh available masks list
                                state.fft_available_masks = config::fft_masks::list_masks_for_resolution(stream_res);
                            }
                            Err(e) => {
                                state.error(format!("Failed to save mask: {}", e));
                            }
                        }
                    }
                });

                // Refresh available masks when resolution changes
                if state.fft_available_masks.is_empty() {
                    state.fft_available_masks = config::fft_masks::list_masks_for_resolution(stream_res);
                }

                if !state.fft_available_masks.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label("Load mask:");
                        for mask_name in state.fft_available_masks.clone() {
                            if ui.button(&mask_name).clicked() {
                                match config::fft_masks::load_mask(&mask_name, stream_res) {
                                    Ok((data, fft_res, mask_thresh, black_thresh)) => {
                                        if fft_res == (fft_w, fft_h) {
                                            state.fft_mask_data = data;
                                            state.fft_mask_threshold = mask_thresh;
                                            state.fft_black_threshold = black_thresh;
                                            state.fft_mask_save_name = mask_name.clone();
                                            mask_changed = true;
                                            state.info(format!("Loaded FFT mask '{}'", mask_name));
                                        } else {
                                            state.error(format!(
                                                "FFT size mismatch: mask is {}x{} but current is {}x{}",
                                                fft_res.0, fft_res.1, fft_w, fft_h
                                            ));
                                        }
                                    }
                                    Err(e) => {
                                        state.error(format!("Failed to load mask: {}", e));
                                    }
                                }
                            }
                        }
                    });
                }
            }

            ui.separator();
            ui.label("Left-drag: block frequencies | Right-drag: restore | Scroll: brush size");

            // Render the spectrum+mask preview using GL texture
            let available = ui.available_size();

            if has_frame {
                // Maintain aspect ratio while filling available space
                let aspect = fft_w as f32 / fft_h as f32;
                let display_w = available.x.min(available.y * aspect);
                let display_h = display_w / aspect;
                let display_size = egui::Vec2::new(display_w, display_h);

                let (response, painter) = ui.allocate_painter(display_size, egui::Sense::click_and_drag());
                let rect = response.rect;

                // Render the spectrum texture using a GL PaintCallback
                if let Some(fft_arc) = &state.fft_filter {
                    let fft_clone = fft_arc.clone();

                    let callback = egui::PaintCallback {
                        rect,
                        callback: std::sync::Arc::new(egui_glow::CallbackFn::new(
                            move |_info, painter| {
                                let gl = painter.gl();
                                let fft = fft_clone.lock().unwrap();
                                let spectrum_tex = fft.spectrum_texture();
                                fft.blit_texture(gl, spectrum_tex);
                            },
                        )),
                    };
                    painter.add(callback);
                }

                // Draw brush cursor
                if let Some(hover_pos) = response.hover_pos() {
                    let brush_screen_radius = state.fft_brush_radius * display_w / fft_w as f32;
                    painter.circle_stroke(
                        hover_pos,
                        brush_screen_radius,
                        egui::Stroke::new(2.0, egui::Color32::RED),
                    );
                }

                // Handle mouse interaction for painting
                // Support both single clicks and drags
                let is_painting = response.dragged_by(egui::PointerButton::Primary)
                    || response.clicked_by(egui::PointerButton::Primary);
                let is_erasing = response.dragged_by(egui::PointerButton::Secondary)
                    || response.clicked_by(egui::PointerButton::Secondary);

                if is_painting || is_erasing {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let rel_x = (pos.x - rect.min.x) / rect.width();
                        let rel_y = (pos.y - rect.min.y) / rect.height();

                        // Flip Y to match GL texture orientation
                        let mask_x = (rel_x * fft_w as f32) as i32;
                        let mask_y = ((1.0 - rel_y) * fft_h as f32) as i32;

                        let radius = state.fft_brush_radius as i32;
                        let value = if is_painting { 0u8 } else { 255u8 };

                        let (w, h) = (fft_w as i32, fft_h as i32);
                        for dy in -radius..=radius {
                            for dx in -radius..=radius {
                                if dx * dx + dy * dy <= radius * radius {
                                    let px = mask_x + dx;
                                    let py = mask_y + dy;
                                    if px >= 0 && px < w && py >= 0 && py < h {
                                        state.fft_mask_data[(py * w + px) as usize] = value;
                                    }
                                }
                            }
                        }
                        mask_changed = true;
                    }
                }

                // Handle scroll wheel for brush size — one step per discrete scroll event
                if response.hovered() {
                    let scroll_ticks: i32 = ctx.input(|i| {
                        i.events.iter().filter_map(|e| {
                            if let egui::Event::MouseWheel { delta, .. } = e {
                                if delta.y > 0.0 { Some(1) }
                                else if delta.y < 0.0 { Some(-1) }
                                else { None }
                            } else { None }
                        }).sum()
                    });
                    if scroll_ticks != 0 {
                        state.fft_brush_radius = (state.fft_brush_radius + scroll_ticks as f32).clamp(1.0, 64.0);
                    }
                }
            } else {
                ui.label("No video frame available. Start a stream to see the FFT spectrum.");
            }
        });

    state.video.fft_mask_window_open = window_open;
    mask_changed
}
