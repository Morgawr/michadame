use crate::{app::AppState, config};
use eframe::egui;

pub fn show_first_run_dialog(state: &mut AppState, ctx: &egui::Context, ui: &mut egui::Ui) {
    let screen_rect = ctx.screen_rect();
    ui.painter().rect_filled(screen_rect, 0.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 128));

    egui::Window::new("Heads Up!")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                if let Some(logo) = &state.logo_texture {
                    ui.add(egui::Image::new(logo).max_height(160.0));
                }
            });
            ui.add_space(10.0);
            ui.label("WARNING: Some capture cards require resetting the USB device after every stream. If yours is one of them, select your USB device from the drop down and make sure to reset it before or after you are done running the capture feed. This requires root.");
            ui.add_space(10.0);
            ui.label(egui::RichText::new("Also, DO NOT FALL IN LOVE WITH THE ANIME GIRL, SHE IS NOT REAL").strong().color(egui::Color32::RED));
            ui.add_space(15.0);
            ui.vertical_centered(|ui| {
                if ui.button("I Understand").clicked() {
                    state.show_first_run_dialog = false;
                    config::save_config(state);
                }
            });
        });
}