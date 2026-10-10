use crate::{
    app::AppState,
    devices,
    ui::controls::technical_group,
};
use eframe::egui;

pub fn draw_device_selectors(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;

    // --- USB Devices ---
    technical_group(ui, "USB Devices:", |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("USB Device to Reset:")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            );
            let selected_text = state
                .hardware
                .selected_usb_device
                .as_ref()
                .and_then(|selected_id| {
                    state
                        .hardware
                        .usb_devices
                        .iter()
                        .find(|(id, _)| id == selected_id)
                        .map(|(id, name)| format!("{} {}", id, name))
                })
                .unwrap_or_else(|| "None".to_string());
            egui::ComboBox::from_id_source("usb_device_selector")
                .selected_text(selected_text)
                .show_ui(ui, |ui| {
                    let mut combo_changed = ui
                        .selectable_value(&mut state.hardware.selected_usb_device, None, "None")
                        .changed();
                    for (id, name) in &state.hardware.usb_devices {
                        combo_changed |= ui
                            .selectable_value(
                                &mut state.hardware.selected_usb_device,
                                Some(id.clone()),
                                format!("{} {}", id, name),
                            )
                            .changed();
                    }
                    if combo_changed {
                        changed = true;
                    }
                });

            let has_usb = state.hardware.selected_usb_device.is_some();
            if ui
                .add_enabled(has_usb, egui::Button::new("Reset USB Device"))
                .clicked()
            {
                if let Some(selected_device) = &state.hardware.selected_usb_device {
                    let msg = match devices::usb::reset_usb_device(selected_device) {
                        Ok(_) => "USB device reset successfully.".to_string(),
                        Err(e) => format!("Failed to reset USB: {}", e),
                    };
                    state.info(msg);
                }
            }
            if ui
                .add_enabled(
                    has_usb,
                    egui::Checkbox::new(&mut state.ui.reset_usb_on_startup, "Reset on startup"),
                )
                .on_hover_text("Requires pkexec to be configured for usbreset without a password prompt for automatic startup reset.")
                .changed()
            {
                crate::config::save_global_hardware_config(state);
                changed = true;
            }
        });
    });

    ui.add_space(4.0);

    // --- Video Devices ---
    technical_group(ui, "Video Device:", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("Video Device:")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            );
            let _combo_box = egui::ComboBox::from_id_source("video_device_selector")
                .selected_text(state.hardware.selected_video_device.as_str())
                .show_ui(ui, |ui| {
                    let mut combo_changed = false;
                    for device in &state.hardware.video_devices {
                        combo_changed |= ui
                            .selectable_value(
                                &mut state.hardware.selected_video_device,
                                device.clone(),
                                device.as_str(),
                            )
                            .changed();
                    }
                    if combo_changed && !state.hardware.selected_video_device.is_empty() {
                        state.hardware.supported_formats.clear();
                        state.hardware.selected_format_index = 0;
                        state.hardware.selected_resolution = (0, 0);

                        match devices::video::find_video_formats(&state.hardware.selected_video_device)
                        {
                            Ok(formats) => {
                                state.info(format!(
                                    "Found {} formats for {}.",
                                    formats.len(),
                                    state.hardware.selected_video_device
                                ));
                                state.hardware.supported_formats = formats;
                                if let Some(res) = state
                                    .hardware
                                    .supported_formats
                                    .first()
                                    .and_then(|f| f.resolutions.first())
                                {
                                    state.hardware.selected_resolution = (res.width, res.height);
                                    state.hardware.selected_framerate =
                                        res.framerates.first().cloned().unwrap_or(0);
                                }
                                if let Ok(cfg) =
                                    confy::load::<crate::config::MichadameConfig>("michadame", None)
                                {
                                    crate::video::types::apply_saved_format_config(state, &cfg);
                                }
                            }
                            Err(e) => {
                                state.error(format!("Failed to scan formats: {}", e));
                            }
                        }
                        crate::config::save_global_hardware_config(state);
                        changed = true;
                    }
                });
        });

        if !state.hardware.supported_formats.is_empty() {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let selected_format_description = state.hardware.supported_formats
                    [state.hardware.selected_format_index]
                    .description
                    .clone();
                let resolutions = state.hardware.supported_formats
                    [state.hardware.selected_format_index]
                    .resolutions
                    .clone();

                let format_descriptions: Vec<(usize, String)> = state
                    .hardware
                    .supported_formats
                    .iter()
                    .enumerate()
                    .map(|(i, f)| (i, f.description.clone()))
                    .collect();

                ui.label(
                    egui::RichText::new("Format:")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(170, 170, 170)),
                );
                egui::ComboBox::from_id_source("format_selector")
                    .selected_text(selected_format_description)
                    .show_ui(ui, |ui| {
                        for (i, desc) in format_descriptions {
                            if ui
                                .selectable_value(
                                    &mut state.hardware.selected_format_index,
                                    i,
                                    desc,
                                )
                                .changed()
                            {
                                if let Some(res) =
                                    state.hardware.supported_formats[i].resolutions.first()
                                {
                                    state.hardware.selected_resolution = (res.width, res.height);
                                    state.hardware.selected_framerate =
                                        res.framerates.first().cloned().unwrap_or(0);
                                }
                                crate::config::save_config(state);
                                crate::config::save_global_hardware_config(state);
                                changed = true;
                            }
                        }
                    });

                ui.add_space(6.0);

                ui.label(
                    egui::RichText::new("Resolution:")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(170, 170, 170)),
                );
                egui::ComboBox::from_id_source("resolution_selector")
                    .selected_text(format!(
                        "{}x{}",
                        state.hardware.selected_resolution.0, state.hardware.selected_resolution.1
                    ))
                    .show_ui(ui, |ui| {
                        for res in &resolutions {
                            if ui
                                .selectable_value(
                                    &mut state.hardware.selected_resolution,
                                    (res.width, res.height),
                                    format!("{}x{}", res.width, res.height),
                                )
                                .changed()
                            {
                                state.hardware.selected_framerate =
                                    res.framerates.first().cloned().unwrap_or(0);
                                crate::config::save_config(state);
                                crate::config::save_global_hardware_config(state);
                                changed = true;
                            }
                        }
                    });

                if let Some(res_info) = resolutions.iter().find(|r| {
                    r.width == state.hardware.selected_resolution.0
                        && r.height == state.hardware.selected_resolution.1
                }) {
                    if !res_info.framerates.is_empty() {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new("Framerate:")
                                .monospace()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(170, 170, 170)),
                        );
                        egui::ComboBox::from_id_source("framerate_selector")
                            .selected_text(format!("{} fps", state.hardware.selected_framerate))
                            .show_ui(ui, |ui| {
                                for &fps in &res_info.framerates {
                                    if ui
                                        .selectable_value(
                                            &mut state.hardware.selected_framerate,
                                            fps,
                                            format!("{} fps", fps),
                                        )
                                        .changed()
                                    {
                                        crate::config::save_config(state);
                                        crate::config::save_global_hardware_config(state);
                                        changed = true;
                                    }
                                }
                            });
                    }
                }
            });
        }
    });

    ui.add_space(4.0);

    // --- Audio Devices ---
    technical_group(ui, "", |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Audio Configuration:")
                    .monospace()
                    .size(11.0)
                    .strong()
                    .color(egui::Color32::from_rgb(224, 224, 224)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Refresh").clicked() {
                    state.info("Refresh clicked. Please restart the app to re-scan devices.");
                    changed = true;
                }
            });
        });
        ui.add_space(4.0);

        let selected_source_desc = state
                .hardware
                .audio_sources
                .iter()
                .find(|(_, name)| Some(name) == state.hardware.selected_audio_source_name.as_ref())
                .map(|(desc, _)| desc.clone())
                .unwrap_or_else(|| "Select an Input".to_string());

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Input (Source):")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(170, 170, 170)),
                );
                egui::ComboBox::from_id_source("audio_source_selector")
                    .selected_text(selected_source_desc)
                    .show_ui(ui, |ui| {
                        let mut combo_changed = false;
                        for (desc, name) in &state.hardware.audio_sources {
                            combo_changed |= ui
                                .selectable_value(
                                    &mut state.hardware.selected_audio_source_name,
                                    Some(name.clone()),
                                    desc,
                                )
                                .changed();
                        }
                        if combo_changed {
                            crate::config::save_config(state);
                            crate::config::save_global_hardware_config(state);
                            if state.hardware.active_audio_stream.is_some() {
                                state.restart_audio_stream(ui.ctx());
                            }
                            changed = true;
                        }
                    });
            });

            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Buffer Size (Samples):")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(170, 170, 170)),
                );
                let buffer_sizes = [32, 64, 128, 256, 512, 1024, 2048, 4096];
                let current_size = state.hardware.audio_buffer_size;

                egui::ComboBox::from_id_source("audio_buffer_size")
                    .selected_text(format!("{}", current_size))
                    .show_ui(ui, |ui| {
                        let mut combo_changed = false;
                        for &size in &buffer_sizes {
                            combo_changed |= ui
                                .selectable_value(
                                    &mut state.hardware.audio_buffer_size,
                                    size,
                                    format!("{}", size),
                                )
                                .changed();
                        }
                        if combo_changed {
                            crate::config::save_config(state);
                            crate::config::save_global_hardware_config(state);
                            if state.hardware.active_audio_stream.is_some() {
                                state.restart_audio_stream(ui.ctx());
                            }
                            changed = true;
                        }
                    });

                ui.add_space(6.0);

                ui.label(
                    egui::RichText::new("Sample Rate:")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(170, 170, 170)),
                );
                let sample_rates = [44100, 48000];
                let current_rate = state.hardware.audio_sample_rate;

                egui::ComboBox::from_id_source("audio_sample_rate")
                    .selected_text(format!("{} Hz", current_rate))
                    .show_ui(ui, |ui| {
                        let mut combo_changed = false;
                        for &rate in &sample_rates {
                            combo_changed |= ui
                                .selectable_value(
                                    &mut state.hardware.audio_sample_rate,
                                    rate,
                                    format!("{} Hz", rate),
                                )
                                .changed();
                        }
                        if combo_changed {
                            crate::config::save_config(state);
                            crate::config::save_global_hardware_config(state);
                            if state.hardware.active_audio_stream.is_some() {
                                state.restart_audio_stream(ui.ctx());
                            }
                            changed = true;
                        }
                    });

                ui.add_space(6.0);

                ui.label(
                    egui::RichText::new("Format:")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(170, 170, 170)),
                );
                let formats = ["S16LE", "S32LE", "F32LE"];
                let current_format = &state.hardware.audio_sample_format;

                egui::ComboBox::from_id_source("audio_sample_format")
                    .selected_text(current_format)
                    .show_ui(ui, |ui| {
                        let mut combo_changed = false;
                        for &fmt in &formats {
                            combo_changed |= ui
                                .selectable_value(
                                    &mut state.hardware.audio_sample_format,
                                    fmt.to_string(),
                                    fmt,
                                )
                                .changed();
                        }
                        if combo_changed {
                            crate::config::save_config(state);
                            crate::config::save_global_hardware_config(state);
                            if state.hardware.active_audio_stream.is_some() {
                                state.restart_audio_stream(ui.ctx());
                            }
                            changed = true;
                        }
                    });
            });
    });

    changed
}
