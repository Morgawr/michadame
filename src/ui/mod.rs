use crate::app::AppState;
use eframe::egui;

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
            }

            repaint_requested |= controls::layout_top_ui(ui, state);

            if state.video_thread.is_some() {
                if !state.is_fullscreen {
                    ui.separator();
                }
                repaint_requested |= draw_video_player(state, ui, ctx);
            } else {
                if !state.is_fullscreen {
                    ui.allocate_space(egui::vec2(640.0, 360.0));
                }
            }
            repaint_requested
        })
        .inner
}

fn draw_video_player(state: &mut AppState, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
    let image_widget = egui::Image::new(state.video_texture.as_ref().unwrap())
        .fit_to_exact_size(ui.available_size());

    let response = ui.add(image_widget.sense(egui::Sense::click()));
    if response.double_clicked() {
        state.is_fullscreen = !state.is_fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(state.is_fullscreen));
        return true;
    }
    false
}