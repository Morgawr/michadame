use crate::app::AppState;
use eframe::egui;

use crate::ui::{devices, filters, profiles};

pub fn layout_top_ui(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;

    ui.horizontal(|ui| {
        if let Some(logo) = &state.logo_texture {
            ui.add(egui::Image::new(logo).max_height(160.0));
        }
        ui.heading("Michadame Viewer");
    });
    ui.separator();

    changed |= profiles::draw_profile_management(ui, state);
    changed |= devices::draw_device_selectors(ui, state);
    changed |= filters::draw_filters(ui, state);

    if changed {
        // config::save_config(state); is handled explicitly by the Save button now
        // except when drawing specific triggers like resetting defaults. We'll do it
        // there explicitly if needed.
    }

    ui.separator();

    ui.horizontal(|ui| {
        if state.ui.video_window_open {
            if ui.button("🛑 Stop Stream").clicked() {
                state.stop_stream(ui.ctx());
                state.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default(), text: "Stream stopped.".to_string().into() });
            }
        } else {
            let can_stream =
                !state.hardware.selected_video_device.is_empty() && state.hardware.selected_resolution.0 > 0;
            if ui
                .add_enabled(can_stream, egui::Button::new("▶ Start Stream"))
                .clicked()
            {
                state.start_stream(ui.ctx());
                state.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default(), text: "Stream starting...".to_string().into() });
            }
            if !can_stream {
                ui.label("Select Video Format/Resolution first.");
            }
        }
    });

    changed
}
