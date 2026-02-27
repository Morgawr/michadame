use crate::app::AppState;
use crate::devices::filter_type::CrtFilter;
use crate::video;
use eframe::egui;
use eframe::egui_glow;
use super::networking::send_ws_command;

pub fn draw_video_player(state: &mut AppState, ui: &mut egui::Ui, ctx: &egui::Context) {
    if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
        println!("Spacebar pressed, sending command...");
        send_ws_command(serde_json::json!({"command": "manual_ocr"}));
    }

    if state.ui.video_window_open {
        let response = ui.allocate_response(ui.available_size(), egui::Sense::click());
        let video_texture = state.video_texture.as_ref().expect("Video texture not initialized");
        let texture_size = video_texture.size_vec2();

        let filter = CrtFilter::from_u8(state.crt_filter.load(std::sync::atomic::Ordering::Relaxed));
        let fft_filter_ref = if state.video.fft_filter_enabled {
            state.fft_filter.clone()
        } else {
            None
        };

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

                let callback = egui::PaintCallback {
                    rect: response.rect,
                    callback: std::sync::Arc::new(egui_glow::CallbackFn::new(
                        move |_info, painter| {
                            let mut renderer = renderer_clone.lock().unwrap();
                            let output_size = (rect.width(), rect.height());
                            let fallback_tex = video_texture_id.and_then(|id| painter.texture(id));

                            let res = latest_frame
                                .as_ref()
                                .map(|f| (f.width, f.height))
                                .unwrap_or((texture_size.x as u32, texture_size.y as u32));

                            renderer.paint(
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
                            )
                        },
                    )),
                };
                ui.painter().add(callback);
            }
        } else {
            let renderer_clone = state.crt_renderer.as_ref().expect("Renderer not initialized").clone();
            let rect = response.rect;
            let background_color = if state.video.use_magenta_background {
                [1.0, 0.0, 1.0]
            } else {
                [0.0, 0.0, 0.0]
            };
            let horizontal_stretch = state.video.horizontal_stretch;
            let median_filter_enabled = state.video.median_filter_enabled;
            let vibrance = state.video.vibrance;
            let overscan_x = state.video.overscan_x;
            let overscan_y = state.video.overscan_y;
            let scaler_filter = state.scaler_filter.load(std::sync::atomic::Ordering::Relaxed);
            let latest_frame = state.latest_frame.clone();
            let video_texture_id = state.video_texture.as_ref().map(|t| t.id());
            let fft_clone = fft_filter_ref.clone();
            let fft_threshold = state.fft_mask_threshold;
            let fft_black = state.fft_black_threshold;

            let callback = egui::PaintCallback {
                rect,
                callback: std::sync::Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                    let fallback_tex = video_texture_id.and_then(|id| painter.texture(id));
                    let res = latest_frame
                        .as_ref()
                        .map(|f| (f.width, f.height))
                        .unwrap_or((texture_size.x as u32, texture_size.y as u32));
                    renderer_clone.lock().unwrap().draw_passthrough(
                        painter.gl(),
                        latest_frame.as_deref(),
                        fallback_tex,
                        res,
                        (rect.width(), rect.height()),
                        background_color,
                        horizontal_stretch,
                        median_filter_enabled,
                        vibrance,
                        scaler_filter,
                        overscan_x,
                        overscan_y,
                        fft_clone.as_ref(),
                        fft_threshold,
                        fft_black,
                    );
                })),
            };
            ui.painter().add(callback);
        }
        if response.double_clicked() {
            let is_fullscreen = !ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(is_fullscreen));
        }
    }
}
