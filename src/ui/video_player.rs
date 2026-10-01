use super::networking::send_ws_command;
use crate::app::AppState;
use crate::devices::filter_type::CrtFilter;
use crate::video;
use eframe::egui;
use eframe::egui_glow;
use std::sync::atomic::Ordering;

fn capture_frame_pixels(
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

    // Flip vertically (OpenGL coordinate origin is bottom-left; image origin is top-left)
    let row_bytes = (area.width * 4) as usize;
    let mut flipped = vec![0u8; raw_pixels.len()];
    for y in 0..area.height as usize {
        let src_row = area.height as usize - 1 - y;
        flipped[y * row_bytes..(y + 1) * row_bytes]
            .copy_from_slice(&raw_pixels[src_row * row_bytes..(src_row + 1) * row_bytes]);
    }

    Some((flipped, area.width, area.height))
}

fn dispatch_ocr_worker(
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    sender: crossbeam_channel::Sender<Result<Vec<crate::ocr::ParsedLine>, String>>,
    is_processing: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    is_processing.store(true, Ordering::Release);
    let _ = std::thread::Builder::new()
        .name("google-lens-ocr".into())
        .spawn(move || {
            use image::{
                codecs::png::{CompressionType, FilterType, PngEncoder},
                imageops::FilterType as ResizeFilter,
                ExtendedColorType, ImageEncoder, RgbaImage,
            };

            let Some(rgba_img) = RgbaImage::from_raw(width, height, pixels) else {
                is_processing.store(false, Ordering::Release);
                let _ = sender.send(Err("Invalid raw pixel buffer for OCR".into()));
                return;
            };

            // Scale to 720p equivalent if larger, preserving aspect ratio
            let (target_w, target_h) =
                crate::ocr::lens::calculate_720p_target_dimensions(width, height);

            let (final_img, final_w, final_h) = if target_w != width || target_h != height {
                let resized = image::imageops::resize(
                    &rgba_img,
                    target_w,
                    target_h,
                    ResizeFilter::Triangle,
                );
                (resized, target_w, target_h)
            } else {
                (rgba_img, width, height)
            };

            // Convert to RGB8 (strip alpha channel to reduce payload size by 25%)
            let rgb_img = image::RgbImage::from_fn(final_w, final_h, |x, y| {
                let pixel = final_img.get_pixel(x, y);
                image::Rgb([pixel[0], pixel[1], pixel[2]])
            });

            let mut png_bytes = Vec::new();
            let encoder = PngEncoder::new_with_quality(
                &mut png_bytes,
                CompressionType::Fast,
                FilterType::NoFilter,
            );
            if let Err(e) = encoder.write_image(
                rgb_img.as_raw(),
                final_w,
                final_h,
                ExtendedColorType::Rgb8,
            ) {
                is_processing.store(false, Ordering::Release);
                let _ = sender.send(Err(format!("PNG encode failed: {e}")));
                return;
            }

            let result = crate::ocr::lens::execute_lens_ocr(png_bytes, final_w, final_h);
            let _ = sender.send(result);
        });
}

pub fn draw_video_player(state: &mut AppState, ui: &mut egui::Ui, ctx: &egui::Context) {
    // Space to capture OCR; Shift+Space to clear OCR boxes
    if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
        if ctx.input(|i| i.modifiers.shift) {
            if !state.ocr.boxes.is_empty() {
                state.ocr.boxes.clear();
                state.ocr.raw_lines.clear();
                state.ocr.last_copied_index = None;
            }
        } else {
            if !state.ocr.is_processing.load(Ordering::Relaxed) {
                state.ocr.capture_requested.store(true, Ordering::Release);
                state.info("Capturing screen for Google Lens OCR...");
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
        let ocr_processing = state.ocr.is_processing.clone();
        let ocr_sender = state
            .ocr
            .ocr_sender
            .as_ref()
            .expect("OCR sender channel not initialized")
            .clone();

        let ppp = ctx.pixels_per_point();
        let output_size = (response.rect.width() * ppp, response.rect.height() * ppp);
        let latest_frame_dim = state.latest_frame.as_ref().map(|f| (f.width, f.height));
        let res = latest_frame_dim.unwrap_or((texture_size.x as u32, texture_size.y as u32));
        let rendered_area_geom = crate::video::gpu::geometry::RenderedArea::fit(
            res,
            output_size,
            state.video.horizontal_stretch,
        );

        if state.video.pixelate_filter_enabled || filter == CrtFilter::Lottes {
            if let Some(renderer_arc) = &state.crt_renderer {
                let renderer_clone = renderer_arc.clone();
                let params = video::gpu::ShaderParams::from_state(state);
                let pixelate = state.video.pixelate_filter_enabled;
                let run_lottes = filter == CrtFilter::Lottes;
                let rect = response.rect;
                let latest_frame = state.latest_frame.clone();
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
                let ocr_processing_cb = ocr_processing.clone();
                let ocr_sender_cb = ocr_sender.clone();

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
                                .unwrap_or((texture_size.x as u32, texture_size.y as u32));

                            let rendered_area = renderer.paint(
                                painter.gl(),
                                latest_frame.as_deref(),
                                fallback_tex,
                                res,
                                output_size,
                                &params,
                                pixelate,
                                run_lottes,
                                fft_clone.as_ref(),
                                fft_threshold,
                                fft_black,
                            );
                            let (at, rate) = latest_frame
                                .as_ref()
                                .map(|f| (f.captured_at, f.rate))
                                .unwrap_or((0, crate::replay::config::Rate::new(60, 1)));
                            replay_gpu.lock().unwrap().capture(
                                painter.gl(),
                                replay.as_ref(),
                                rendered_area,
                                at,
                                rate,
                            );

                            if ocr_capture_cb.swap(false, Ordering::AcqRel) {
                                if let Some((pixels, w, h)) =
                                    capture_frame_pixels(painter.gl(), rendered_area)
                                {
                                    dispatch_ocr_worker(
                                        pixels,
                                        w,
                                        h,
                                        ocr_sender_cb.clone(),
                                        ocr_processing_cb.clone(),
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
            let background_color = if state.video.use_magenta_background {
                [1.0, 0.0, 1.0]
            } else {
                [0.0, 0.0, 0.0]
            };
            let horizontal_stretch = state.video.horizontal_stretch;
            let median_filter_enabled = state.video.median_filter_enabled;
            let median_mix = state.video.median_mix;
            let vibrance = state.video.vibrance;
            let overscan_x = state.video.overscan_x;
            let overscan_y = state.video.overscan_y;
            let scaler_filter = state
                .scaler_filter
                .load(Ordering::Relaxed);
            let latest_frame = state.latest_frame.clone();
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
            let ocr_processing_cb = ocr_processing.clone();
            let ocr_sender_cb = ocr_sender.clone();

            let callback = egui::PaintCallback {
                rect,
                callback: std::sync::Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                    let fallback_tex = video_texture_id.and_then(|id| painter.texture(id));
                    let res = latest_frame
                        .as_ref()
                        .map(|f| (f.width, f.height))
                        .unwrap_or((texture_size.x as u32, texture_size.y as u32));
                    let rendered_area = renderer_clone.lock().unwrap().draw_passthrough(
                        painter.gl(),
                        latest_frame.as_deref(),
                        fallback_tex,
                        res,
                        (rect.width() * ppp, rect.height() * ppp),
                        background_color,
                        horizontal_stretch,
                        median_filter_enabled,
                        median_mix,
                        vibrance,
                        scaler_filter,
                        overscan_x,
                        overscan_y,
                        fft_clone.as_ref(),
                        fft_threshold,
                        fft_black,
                    );
                    let (at, rate) = latest_frame
                        .as_ref()
                        .map(|f| (f.captured_at, f.rate))
                        .unwrap_or((0, crate::replay::config::Rate::new(60, 1)));
                    replay_gpu.lock().unwrap().capture(
                        painter.gl(),
                        replay.as_ref(),
                        rendered_area,
                        at,
                        rate,
                    );

                    if ocr_capture_cb.swap(false, Ordering::AcqRel) {
                        if let Some((pixels, w, h)) =
                            capture_frame_pixels(painter.gl(), rendered_area)
                        {
                            dispatch_ocr_worker(
                                pixels,
                                w,
                                h,
                                ocr_sender_cb.clone(),
                                ocr_processing_cb.clone(),
                            );
                        }
                    }
                })),
            };
            ui.painter().add(callback);
        }

        // Draw interactive OCR bounding box overlay on top of the video image
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

        crate::ocr::overlay::draw_ocr_overlay(ui, state, video_rect);

        if response.double_clicked() {
            let is_fullscreen = !ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(is_fullscreen));
        }
    }
}
