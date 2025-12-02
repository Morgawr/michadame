use crate::app::AppState;
use eframe::egui;
use eframe::egui_glow;
use crate::devices::filter_type::CrtFilter;
use crate::video;

pub mod controls;
pub mod dialogs;

pub fn draw_main_ui(state: &mut AppState, ctx: &egui::Context) -> bool {
    let panel_frame = if state.is_fullscreen {
        egui::Frame::none()
    } else {
        egui::Frame::central_panel(&ctx.style())
    };

    egui::CentralPanel::default()
        .frame(panel_frame)
        .show(ctx, |ui| {
            let mut repaint_requested = false;
            if state.show_first_run_dialog {
                repaint_requested |= dialogs::show_first_run_dialog(state, ctx, ui);
            } else if state.show_quit_dialog {
                dialogs::show_quit_dialog(state, ctx, ui);
                repaint_requested = true;
            }

            repaint_requested |= controls::layout_top_ui(ui, state);

            repaint_requested
        })
        .inner
}

pub fn draw_video_player(state: &mut AppState, ui: &mut egui::Ui, ctx: &egui::Context) {
    let response = ui.allocate_response(ui.available_size(), egui::Sense::click());

    let filter = CrtFilter::from_u8(state.crt_filter.load(std::sync::atomic::Ordering::Relaxed));

    if filter == CrtFilter::Lottes && state.crt_renderer.is_some() {
        let renderer = state.crt_renderer.as_ref().unwrap().clone();
        let video_texture_id = state.video_texture.as_ref().unwrap().id();
        let resolution = state.selected_resolution;
        let params = video::gpu_filter::ShaderParams {
            hard_scan: state.crt_hard_scan,
            warp_x: state.crt_warp_x,
            warp_y: state.crt_warp_y,
            shadow_mask: state.crt_shadow_mask,
            brightboost: state.crt_brightboost,
            hard_bloom_pix: state.crt_hard_bloom_pix,
            hard_bloom_scan: state.crt_hard_bloom_scan,
            bloom_amount: state.crt_bloom_amount,
            shape: state.crt_shape,
            hard_pix: state.crt_hard_pix,
        };

        let callback = egui::PaintCallback {
            rect: response.rect,
            callback: std::sync::Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                let output_size = (response.rect.width(), response.rect.height());
                renderer.lock().unwrap().paint(painter, video_texture_id, resolution, output_size, &params);
            })),
        };
        ui.painter().add(callback);
    } else {
        let image_widget = egui::Image::new(state.video_texture.as_ref().unwrap())
            .fit_to_exact_size(response.rect.size());
        ui.put(response.rect, image_widget);
    }
    if response.double_clicked() {
        let is_fullscreen = !ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(is_fullscreen));
    }
}