use crate::{app::AppState, config};
use eframe::egui;

pub fn show_first_run_dialog(state: &mut AppState, ctx: &egui::Context, ui: &mut egui::Ui) -> bool {
    let screen_rect = ctx.screen_rect();
    ui.painter().rect_filled(
        screen_rect,
        0.0,
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 128),
    );

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
                    state.ui.show_first_run_dialog = false;
                    config::save_config(state);
                    true
                } else {
                    false
                }
            }).inner
        })
        .and_then(|inner| inner.inner)
        .unwrap_or(false)
}

pub fn show_quit_dialog(state: &mut AppState, ctx: &egui::Context, ui: &mut egui::Ui) {
    let screen_rect = ctx.screen_rect();
    ui.painter().rect_filled(
        screen_rect,
        0.0,
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 128),
    );

    egui::Window::new("Quit?")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("A video stream is active. Are you sure you want to quit the application?");
            ui.add_space(15.0);
            ui.horizontal(|ui| {
                if ui.button("Yes, quit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if ui.button("Cancel").clicked() {
                    state.ui.show_quit_dialog = false;
                }
            });
        });
}

pub fn show_stop_stream_dialog(
    state: &mut AppState,
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    main_ctx: &egui::Context,
) {
    let screen_rect = ctx.screen_rect();
    ui.painter().rect_filled(
        screen_rect,
        0.0,
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 128),
    );

    egui::Window::new("Stop Stream?")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("Are you sure you want to stop the video stream?");
            ui.add_space(15.0);
            ui.horizontal(|ui| {
                if ui.button("Yes, stop stream").clicked() {
                    state.stop_stream(main_ctx);
                }
                if ui.button("Cancel").clicked() {
                    state.ui.show_stop_stream_dialog = false;
                }
            });
        });
}

pub fn show_config_error_dialog(
    state: &mut AppState,
    ctx: &egui::Context,
    ui: &mut egui::Ui,
) -> bool {
    let screen_rect = ctx.screen_rect();
    ui.painter().rect_filled(
        screen_rect,
        0.0,
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 160),
    );

    let mut dismissed = false;
    egui::Window::new("Configuration Warning")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("Failed to load existing configuration")
                        .size(17.0)
                        .strong()
                        .color(egui::Color32::from_rgb(255, 100, 100)),
                );
            });
            ui.add_space(8.0);

            ui.label(
                "An error occurred while loading your configuration file. To protect your existing profiles and settings from being overwritten or lost, config auto-saving has been DISABLED for this session.",
            );
            ui.add_space(8.0);

            if let Some(err) = &state.config_load_error {
                ui.group(|ui| {
                    ui.label(egui::RichText::new("Error detail:").strong());
                    ui.label(
                        egui::RichText::new(err)
                            .monospace()
                            .color(egui::Color32::from_rgb(230, 190, 120)),
                    );
                });
                ui.add_space(8.0);
            }

            if let Some(quarantine) = &state.config_quarantine_path {
                ui.group(|ui| {
                    ui.label(
                        egui::RichText::new("A backup copy of your previous file was saved to:")
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(quarantine.to_string_lossy())
                            .monospace()
                            .color(egui::Color32::LIGHT_BLUE),
                    );
                });
                ui.add_space(10.0);
            }

            ui.vertical_centered(|ui| {
                if ui
                    .button(egui::RichText::new("Dismiss (Continue with Default Settings)").strong())
                    .clicked()
                {
                    state.ui.dismissed_config_error = true;
                    dismissed = true;
                }
            });
        });

    dismissed
}
