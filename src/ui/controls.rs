use crate::{app::AppState, config, devices};
use eframe::egui;

pub fn layout_top_ui(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    if state.is_fullscreen {
        return false;
    }
    layout_top_ui_content(ui, state)
}

fn layout_top_ui_content(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        if let Some(logo) = &state.logo_texture {
            ui.add(egui::Image::new(logo).max_height(160.0));
        }
        ui.heading("Michadame Viewer");
    });
    ui.separator();

    ui.horizontal(|ui| {
        ui.label("USB Device to Reset:");
        let selected_text = state.selected_usb_device.as_ref()
            .and_then(|selected_id| {
                state.usb_devices.iter().find(|(id, _)| id == selected_id)
                    .map(|(id, name)| format!("{} {}", id, name))
            })
            .unwrap_or_else(|| "None".to_string());
        egui::ComboBox::from_id_source("usb_device_selector")
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                let mut combo_changed = ui.selectable_value(&mut state.selected_usb_device, None, "None").changed();
                for (id, name) in &state.usb_devices {
                    combo_changed |= ui.selectable_value(&mut state.selected_usb_device, Some(id.clone()), format!("{} {}", id, name)).changed();
                }
                if combo_changed {
                    config::save_config(state);
                    changed = true;
                }
            });

        if let Some(selected_device) = &state.selected_usb_device {
            if ui.button("Reset USB Device").clicked() {
                state.status_message = match devices::usb::reset_usb_device(selected_device) {
                    Ok(_) => "USB device reset successfully.".to_string(),
                    Err(e) => format!("Failed to reset USB: {}", e),
                };
            }
            if ui.checkbox(&mut state.reset_usb_on_startup, "Reset on startup").on_hover_text("Requires pkexec to be configured for usbreset without a password prompt for automatic startup reset.").changed() {
                config::save_config(state);
                changed = true;
            }
        }
    });

    ui.separator();

    ui.horizontal(|ui| {
        ui.label("Video Device:");
        let _combo_box = egui::ComboBox::from_id_source("video_device_selector")
            .selected_text(state.selected_video_device.as_str())
            .show_ui(ui, |ui| {
                let mut combo_changed = false;
                for device in &state.video_devices {
                    combo_changed |= ui.selectable_value(&mut state.selected_video_device, device.clone(), device.as_str()).changed();
                }
                if combo_changed && !state.selected_video_device.is_empty() {
                    config::save_config(state);
                    state.supported_formats.clear();
                    state.selected_format_index = 0;
                    state.selected_resolution = (0, 0);

                    match devices::video::find_video_formats(&state.selected_video_device) {
                        Ok(formats) => {
                            state.status_message = format!("Found {} formats for {}.", formats.len(), state.selected_video_device);
                            state.supported_formats = formats;
                            if let Some(res) = state.supported_formats.first().and_then(|f| f.resolutions.first()) {
                                state.selected_resolution = (res.width, res.height);
                                state.selected_framerate = res.framerates.first().cloned().unwrap_or(0);
                            }
                        }
                        Err(e) => {
                            state.status_message = format!("Failed to scan formats: {}", e);
                        }
                    }
                    changed = true;
                }
            });
    });

    if !state.supported_formats.is_empty() {
        ui.horizontal(|ui| {
            let selected_format_description = state.supported_formats[state.selected_format_index].description.clone();
            let resolutions = state.supported_formats[state.selected_format_index].resolutions.clone();

            ui.label("Format:");
            egui::ComboBox::from_id_source("format_selector")
                .selected_text(selected_format_description)
                .show_ui(ui, |ui| {
                    for (i, format) in state.supported_formats.iter().enumerate() {
                        if ui.selectable_value(&mut state.selected_format_index, i, &format.description).changed() {
                            if let Some(res) = state.supported_formats[i].resolutions.first() {
                                state.selected_resolution = (res.width, res.height);
                                state.selected_framerate = res.framerates.first().cloned().unwrap_or(0);
                            }
                            config::save_config(state);
                            changed = true;
                        }
                    }
                });

            ui.label("Resolution:");
            egui::ComboBox::from_id_source("resolution_selector")
                .selected_text(format!("{}x{}", state.selected_resolution.0, state.selected_resolution.1))
                .show_ui(ui, |ui| {
                    for res in &resolutions {
                        if ui.selectable_value(&mut state.selected_resolution, (res.width, res.height), format!("{}x{}", res.width, res.height)).changed() {
                            state.selected_framerate = res.framerates.first().cloned().unwrap_or(0);
                            config::save_config(state);
                            changed = true;
                        }
                    }
                });

            if let Some(res_info) = resolutions.iter().find(|r| r.width == state.selected_resolution.0 && r.height == state.selected_resolution.1) {
                if !res_info.framerates.is_empty() {
                    ui.label("Framerate:");
                    egui::ComboBox::from_id_source("framerate_selector")
                        .selected_text(format!("{} fps", state.selected_framerate))
                        .show_ui(ui, |ui| {
                            for &fps in &res_info.framerates {
                                if ui.selectable_value(&mut state.selected_framerate, fps, format!("{} fps", fps)).changed() {
                                    config::save_config(state);
                                    changed = true;
                                }
                            }
                        });
                    }
            }
        });
    }
    ui.separator();

    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label("PulseAudio Configuration:");
            if ui.button("🔄 Refresh").clicked() {
                state.status_message = "Refresh clicked. Please restart the app to re-scan devices.".to_string();
                changed = true;
            }
        });

        let selected_source_desc = state.pulse_sources.iter()
            .find(|(_, name)| Some(name) == state.selected_pulse_source_name.as_ref())
            .map(|(desc, _)| desc.as_str())
            .unwrap_or("Select an Input");

        egui::ComboBox::from_label("Input (Source)")
            .selected_text(selected_source_desc)
            .show_ui(ui, |ui| {
                let mut combo_changed = false;
                for (desc, name) in &state.pulse_sources {
                    combo_changed |= ui.selectable_value(&mut state.selected_pulse_source_name, Some(name.clone()), desc).changed();
                }
                if combo_changed {
                    config::save_config(state);
                    changed = true;
                }
            });

        let selected_sink_desc = state.pulse_sinks.iter()
            .find(|(_, name)| Some(name) == state.selected_pulse_sink_name.as_ref())
            .map(|(desc, _)| desc.as_str())
            .unwrap_or("Select an Output");

        egui::ComboBox::from_label("Output (Sink)")
            .selected_text(selected_sink_desc)
            .show_ui(ui, |ui| {
                let mut combo_changed = false;
                for (desc, name) in &state.pulse_sinks {
                    combo_changed |= ui.selectable_value(&mut state.selected_pulse_sink_name, Some(name.clone()), desc).changed();
                }
                if combo_changed {
                    config::save_config(state);
                    changed = true;
                }
            });
    });
    ui.separator();

    ui.horizontal(|ui| {
        let is_running = state.video_thread.is_some();
        let start_button = ui.add_enabled(!is_running && state.selected_resolution.0 > 0, egui::Button::new("▶ Start Stream"));
        if start_button.clicked() {
            state.start_stream(ui.ctx());
            changed = true;
        }
        let stop_button = ui.add_enabled(is_running, egui::Button::new("⏹ Stop Stream"));
        if stop_button.clicked() {
            state.stop_stream(ui.ctx());
            changed = true;
        }
    });

    ui.separator();
    ui.label(&state.status_message);
    changed
}