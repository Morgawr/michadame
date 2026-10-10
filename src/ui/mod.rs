use crate::app::AppState;
use eframe::egui;

pub mod bank;
pub mod controls;
pub mod debug;
pub mod devices;
pub mod dialogs;
pub mod fft_mask;
pub mod filters;
pub mod networking;
pub mod profiles;
pub mod tag_input;
pub mod twitch;
pub mod twitch_overlay;
pub mod niconico;
pub mod video_player;

pub use networking::send_ws_command;
pub use video_player::draw_video_player;

pub fn setup_style(ctx: &eframe::egui::Context) {
    let mut style = (*ctx.style()).clone();

    // 100% sharp rectangular corners across the entire application (no rounded pills)
    style.visuals.window_rounding = egui::Rounding::ZERO;
    style.visuals.menu_rounding = egui::Rounding::ZERO;
    style.visuals.widgets.noninteractive.rounding = egui::Rounding::ZERO;
    style.visuals.widgets.inactive.rounding = egui::Rounding::ZERO;
    style.visuals.widgets.hovered.rounding = egui::Rounding::ZERO;
    style.visuals.widgets.active.rounding = egui::Rounding::ZERO;
    style.visuals.widgets.open.rounding = egui::Rounding::ZERO;

    // Technical Linux old-school desktop UI (egui/GTK2/X11 style)
    style.visuals.dark_mode = true;
    style.visuals.panel_fill = egui::Color32::from_rgb(26, 26, 26); // #1a1a1a
    style.visuals.window_fill = egui::Color32::from_rgb(26, 26, 26);
    style.visuals.extreme_bg_color = egui::Color32::from_rgb(17, 17, 17); // #111111

    // Slider styling: blocky rectangular handle (aspect ratio 0.55), 8px rail height, 220px track, no trailing fill
    style.visuals.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.55 };
    style.visuals.slider_trailing_fill = false;
    style.spacing.slider_rail_height = 8.0;
    style.spacing.slider_width = 220.0;

    // Group / Noninteractive frame style: #161616 background, #353535 border
    style.visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(22, 22, 22);
    style.visuals.widgets.noninteractive.weak_bg_fill = egui::Color32::from_rgb(22, 22, 22);
    style.visuals.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(53, 53, 53));
    style.visuals.widgets.noninteractive.fg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(208, 208, 208));

    // Inactive button & slider track style:
    // Mandatory bg_fill (slider rail, checkbox): #111111
    style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(17, 17, 17);
    // Weak bg_fill (buttons): #2c2c2c
    style.visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(44, 44, 44);
    style.visuals.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(72, 72, 72));
    style.visuals.widgets.inactive.fg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(208, 208, 208)); // #d0d0d0 crisp white text

    // Hovered button style: #383838 background, #5c5c5c border, white text
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(24, 24, 24);
    style.visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(56, 56, 56);
    style.visuals.widgets.hovered.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(92, 92, 92));
    style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);

    // Active button style: #153c66 background, #21619f border, white text
    style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(18, 75, 128);
    style.visuals.widgets.active.weak_bg_fill = egui::Color32::from_rgb(21, 60, 102);
    style.visuals.widgets.active.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(33, 97, 159));
    style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);

    // Open/Combobox style
    style.visuals.widgets.open.bg_fill = egui::Color32::from_rgb(24, 24, 24);
    style.visuals.widgets.open.weak_bg_fill = egui::Color32::from_rgb(30, 30, 30);
    style.visuals.widgets.open.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 60, 60));
    style.visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);

    // Spacing
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(7.0, 2.0);

    ctx.set_style(style);
}

pub fn draw_main_ui(state: &mut AppState, ctx: &egui::Context) -> bool {
    state.replay.shortcuts(ctx);
    // Ignore Space while typing in a text field (e.g. the mining tag).
    if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Space)) {
        if ctx.input(|i| i.modifiers.shift) {
            state.clear_ocr();
        } else {
            println!("Spacebar pressed, sending command...");
            send_ws_command(serde_json::json!({"command": "manual_ocr"}));
        }
    }

    egui::CentralPanel::default()
        .frame(
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(26, 26, 26))
                .inner_margin(egui::Margin::same(8.0)),
        )
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
                            egui::Color32::from_rgb(208, 208, 208),
                            format!("Quarantined backup: {}", q.display()),
                        );
                    }
                });
                ui.add_space(6.0);
            }

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    repaint_requested |= controls::layout_top_ui(ui, state);
                });

            repaint_requested
        })
        .inner
}
