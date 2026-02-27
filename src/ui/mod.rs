use crate::app::AppState;
use eframe::egui;

pub mod controls;
pub mod devices;
pub mod dialogs;
pub mod filters;
pub mod profiles;
pub mod networking;
pub mod video_player;
pub mod fft_mask;

pub use networking::send_ws_command;
pub use video_player::draw_video_player;

pub fn setup_style(ctx: &eframe::egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals.window_rounding = eframe::egui::Rounding::ZERO;
    ctx.set_style(style);
}

pub fn draw_main_ui(state: &mut AppState, ctx: &egui::Context) -> bool {
    if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
        println!("Spacebar pressed, sending command...");
        send_ws_command(serde_json::json!({"command": "manual_ocr"}));
    }

    let panel_frame = if state.ui.is_fullscreen {
        egui::Frame::none()
    } else {
        egui::Frame::central_panel(&ctx.style())
    };

    egui::CentralPanel::default()
        .frame(panel_frame)
        .show(ctx, |ui| {
            let mut repaint_requested = false;
            if state.ui.show_first_run_dialog {
                repaint_requested |= dialogs::show_first_run_dialog(state, ctx, ui);
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                repaint_requested |= controls::layout_top_ui(ui, state);
            });

            repaint_requested
        })
        .inner
}
