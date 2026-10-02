use crate::app::AppState;
use eframe::egui;

pub mod controls;
pub mod debug;
pub mod devices;
pub mod dialogs;
pub mod fft_mask;
pub mod filters;
pub mod networking;
pub mod profiles;
pub mod video_player;

pub use networking::send_ws_command;
pub use video_player::draw_video_player;

pub fn setup_style(ctx: &eframe::egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals.window_rounding = eframe::egui::Rounding::ZERO;
    ctx.set_style(style);
}

pub fn draw_main_ui(state: &mut AppState, ctx: &egui::Context) -> bool {
    state.replay.shortcuts(ctx);
    if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
        if ctx.input(|i| i.modifiers.shift) {
            state.clear_ocr();
        } else {
            println!("Spacebar pressed, sending command...");
            send_ws_command(serde_json::json!({"command": "manual_ocr"}));
        }
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
            if state.config_load_error.is_some() && !state.ui.dismissed_config_error {
                repaint_requested |= dialogs::show_config_error_dialog(state, ctx, ui);
            }
            if state.ui.show_first_run_dialog {
                repaint_requested |= dialogs::show_first_run_dialog(state, ctx, ui);
            }

            if let Some(err) = &state.config_load_error {
                ui.group(|ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(255, 100, 100),
                        format!("⚠ CONFIG LOAD ERROR: {}. Saving is disabled to prevent overwriting settings.", err),
                    );
                    if let Some(q) = &state.config_quarantine_path {
                        ui.colored_label(
                            egui::Color32::LIGHT_BLUE,
                            format!("Quarantined backup: {}", q.display()),
                        );
                    }
                });
                ui.add_space(6.0);
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                repaint_requested |= controls::layout_top_ui(ui, state);
            });

            repaint_requested
        })
        .inner
}
