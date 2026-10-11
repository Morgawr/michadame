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

    ui.add_space(4.0);

    // --- Microphone Device ---
    technical_group(ui, "Microphone Device:", |ui| {
        ui.horizontal_wrapped(|ui| {
            let mut mic_enabled = state.mic.config.enabled;
            if ui
                .checkbox(&mut mic_enabled, "Enable Microphone")
                .on_hover_text("Enables microphone audio input capture and recording into the replay buffer.")
                .changed()
            {
                state.mic.config.enabled = mic_enabled;
                if mic_enabled && state.mic.config.device_name.is_none() {
                    state.mic.config.device_name = state
                        .hardware
                        .audio_sources
                        .first()
                        .map(|(_, name)| name.clone());
                }
                crate::config::save_config(state);
                crate::config::save_global_hardware_config(state);
                state.sync_mic_stream();
                changed = true;
            }

            ui.add_space(8.0);

            ui.label(
                egui::RichText::new("Mic Input:")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            );

            let selected_mic_desc = state
                .hardware
                .audio_sources
                .iter()
                .find(|(_, name)| Some(name) == state.mic.config.device_name.as_ref())
                .map(|(desc, _)| desc.clone())
                .unwrap_or_else(|| "Select a Microphone".to_string());

            egui::ComboBox::from_id_source("mic_device_selector")
                .selected_text(selected_mic_desc)
                .show_ui(ui, |ui| {
                    let mut combo_changed = false;
                    for (desc, name) in &state.hardware.audio_sources {
                        combo_changed |= ui
                            .selectable_value(
                                &mut state.mic.config.device_name,
                                Some(name.clone()),
                                desc,
                            )
                            .changed();
                    }
                    if combo_changed {
                        crate::config::save_config(state);
                        crate::config::save_global_hardware_config(state);
                        state.sync_mic_stream();
                        changed = true;
                    }
                });
        });
    });

    ui.add_space(4.0);

    // --- Webcam Device ---
    technical_group(ui, "Webcam Device:", |ui| {
        ui.horizontal_wrapped(|ui| {
            let mut webcam_enabled = state.webcam.config.enabled;
            if ui
                .checkbox(&mut webcam_enabled, "Enable Webcam")
                .on_hover_text("Enables the webcam overlay feed on the video stream.")
                .changed()
            {
                state.webcam.config.enabled = webcam_enabled;
                if webcam_enabled && state.webcam.config.device.is_none() {
                    state.webcam.config.device = state.hardware.video_devices.first().cloned();
                    if let Some(dev) = &state.webcam.config.device {
                        if let Ok(formats) = crate::devices::video::find_video_formats(dev) {
                            state.webcam.supported_formats = formats;
                            if let Some(first_fmt) = state.webcam.supported_formats.first() {
                                state.webcam.config.format_fourcc = Some(first_fmt.fourcc.clone());
                                if let Some(first_res) = first_fmt.resolutions.first() {
                                    state.webcam.config.resolution = Some((first_res.width, first_res.height));
                                    state.webcam.config.framerate = first_res.framerates.first().cloned();
                                }
                            }
                        }
                    }
                }
                crate::config::save_config(state);
                crate::config::save_global_hardware_config(state);
                state.sync_webcam_stream(Some(ui.ctx()));
                changed = true;
            }

            ui.add_space(8.0);

            ui.label(
                egui::RichText::new("Webcam Device:")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            );

            let selected_dev_text = state
                .webcam
                .config
                .device
                .as_deref()
                .unwrap_or("None");

            egui::ComboBox::from_id_source("webcam_device_selector")
                .selected_text(selected_dev_text)
                .show_ui(ui, |ui| {
                    let mut dev_changed = ui
                        .selectable_value(&mut state.webcam.config.device, None, "None")
                        .changed();
                    for device in &state.hardware.video_devices {
                        dev_changed |= ui
                            .selectable_value(
                                &mut state.webcam.config.device,
                                Some(device.clone()),
                                device.as_str(),
                            )
                            .changed();
                    }
                    if dev_changed {
                        state.webcam.supported_formats.clear();
                        state.webcam.selected_format_index = 0;
                        if let Some(dev) = &state.webcam.config.device {
                            match crate::devices::video::find_video_formats(dev) {
                                Ok(formats) => {
                                    state.webcam.supported_formats = formats;
                                    if let Some(first_fmt) = state.webcam.supported_formats.first() {
                                        state.webcam.config.format_fourcc = Some(first_fmt.fourcc.clone());
                                        if let Some(first_res) = first_fmt.resolutions.first() {
                                            state.webcam.config.resolution = Some((first_res.width, first_res.height));
                                            state.webcam.config.framerate = first_res.framerates.first().cloned();
                                        }
                                    }
                                }
                                Err(e) => {
                                    state.error(format!("Failed to scan webcam formats: {}", e));
                                }
                            }
                        } else {
                            state.webcam.config.format_fourcc = None;
                            state.webcam.config.resolution = None;
                            state.webcam.config.framerate = None;
                        }
                        crate::config::save_config(state);
                        crate::config::save_global_hardware_config(state);
                        state.sync_webcam_stream(Some(ui.ctx()));
                        changed = true;
                    }
                });
        });

        // Ensure formats are queried if device is selected but formats are empty
        if state.webcam.supported_formats.is_empty() {
            if let Some(dev) = &state.webcam.config.device {
                if let Ok(formats) = crate::devices::video::find_video_formats(dev) {
                    state.webcam.supported_formats = formats;
                    if let Some(saved_fourcc) = &state.webcam.config.format_fourcc {
                        if let Some(idx) = state
                            .webcam
                            .supported_formats
                            .iter()
                            .position(|f| f.fourcc == *saved_fourcc)
                        {
                            state.webcam.selected_format_index = idx;
                        }
                    }
                }
            }
        }

        if !state.webcam.supported_formats.is_empty() {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let formats = state.webcam.supported_formats.clone();
                let fmt_idx = state
                    .webcam
                    .selected_format_index
                    .min(formats.len().saturating_sub(1));
                let selected_format_desc = formats[fmt_idx]
                    .description
                    .clone();
                let resolutions = formats[fmt_idx]
                    .resolutions
                    .clone();

                ui.label(
                    egui::RichText::new("Format:")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(170, 170, 170)),
                );

                egui::ComboBox::from_id_source("webcam_format_selector")
                    .selected_text(selected_format_desc)
                    .show_ui(ui, |ui| {
                        for (i, f) in formats.iter().enumerate() {
                            let is_selected = state.webcam.selected_format_index == i;
                            if ui
                                .selectable_label(is_selected, &f.description)
                                .clicked()
                            {
                                state.webcam.selected_format_index = i;
                                state.webcam.config.format_fourcc = Some(f.fourcc.clone());
                                if let Some(res) = f.resolutions.first() {
                                    state.webcam.config.resolution = Some((res.width, res.height));
                                    state.webcam.config.framerate = res.framerates.first().cloned();
                                }
                                crate::config::save_config(state);
                                crate::config::save_global_hardware_config(state);
                                state.sync_webcam_stream(Some(ui.ctx()));
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

                let current_res = state.webcam.config.resolution.unwrap_or((0, 0));
                let res_text = if current_res.0 > 0 {
                    format!("{}x{}", current_res.0, current_res.1)
                } else {
                    "Default".to_string()
                };

                egui::ComboBox::from_id_source("webcam_resolution_selector")
                    .selected_text(res_text)
                    .show_ui(ui, |ui| {
                        for res in &resolutions {
                            let is_selected = current_res == (res.width, res.height);
                            if ui
                                .selectable_label(is_selected, format!("{}x{}", res.width, res.height))
                                .clicked()
                            {
                                state.webcam.config.resolution = Some((res.width, res.height));
                                state.webcam.config.framerate = res.framerates.first().cloned();
                                crate::config::save_config(state);
                                crate::config::save_global_hardware_config(state);
                                state.sync_webcam_stream(Some(ui.ctx()));
                                changed = true;
                            }
                        }
                    });

                let selected_framerates = resolutions
                    .iter()
                    .find(|r| Some((r.width, r.height)) == state.webcam.config.resolution)
                    .map(|r| r.framerates.clone())
                    .unwrap_or_default();

                if !selected_framerates.is_empty() {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("Framerate:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );

                    let current_fps = state.webcam.config.framerate.unwrap_or(0);
                    let fps_text = if current_fps > 0 {
                        format!("{} fps", current_fps)
                    } else {
                        "Default".to_string()
                    };

                    egui::ComboBox::from_id_source("webcam_framerate_selector")
                        .selected_text(fps_text)
                        .show_ui(ui, |ui| {
                            for &fps in &selected_framerates {
                                let is_selected = current_fps == fps;
                                if ui
                                    .selectable_label(is_selected, format!("{} fps", fps))
                                    .clicked()
                                {
                                    state.webcam.config.framerate = Some(fps);
                                    crate::config::save_config(state);
                                    crate::config::save_global_hardware_config(state);
                                    state.sync_webcam_stream(Some(ui.ctx()));
                                    changed = true;
                                }
                            }
                        });
                }
            });
        }

        ui.add_space(4.0);
        crate::ui::controls::technical_separator(ui);
        crate::ui::controls::sub_heading(ui, "Webcam Cut Out (Pixels):");

        ui.horizontal_wrapped(|ui| {
            let (raw_w, raw_h) = state.webcam.config.resolution.unwrap_or((640, 480));
            let max_x = raw_w.saturating_sub(16);
            let max_y = raw_h.saturating_sub(16);

            ui.label(
                egui::RichText::new("Left:")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            );
            let max_l = max_x.saturating_sub(state.webcam.config.cutout.right);
            let mut cut_l = state.webcam.config.cutout.left;
            if ui
                .add(egui::DragValue::new(&mut cut_l).clamp_range(0..=max_l).speed(1.0))
                .changed()
            {
                state.webcam.config.cutout.left = cut_l;
                crate::config::save_config(state);
                crate::config::save_global_hardware_config(state);
                changed = true;
            }

            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Right:")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            );
            let max_r = max_x.saturating_sub(state.webcam.config.cutout.left);
            let mut cut_r = state.webcam.config.cutout.right;
            if ui
                .add(egui::DragValue::new(&mut cut_r).clamp_range(0..=max_r).speed(1.0))
                .changed()
            {
                state.webcam.config.cutout.right = cut_r;
                crate::config::save_config(state);
                crate::config::save_global_hardware_config(state);
                changed = true;
            }

            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Top:")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            );
            let max_t = max_y.saturating_sub(state.webcam.config.cutout.bottom);
            let mut cut_t = state.webcam.config.cutout.top;
            if ui
                .add(egui::DragValue::new(&mut cut_t).clamp_range(0..=max_t).speed(1.0))
                .changed()
            {
                state.webcam.config.cutout.top = cut_t;
                crate::config::save_config(state);
                crate::config::save_global_hardware_config(state);
                changed = true;
            }

            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Bottom:")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            );
            let max_b = max_y.saturating_sub(state.webcam.config.cutout.top);
            let mut cut_b = state.webcam.config.cutout.bottom;
            if ui
                .add(egui::DragValue::new(&mut cut_b).clamp_range(0..=max_b).speed(1.0))
                .changed()
            {
                state.webcam.config.cutout.bottom = cut_b;
                crate::config::save_config(state);
                crate::config::save_global_hardware_config(state);
                changed = true;
            }

            ui.add_space(8.0);
            if ui
                .button("Reset Cutout")
                .on_hover_text("Reset shaved pixels on all sides back to 0")
                .clicked()
            {
                state.webcam.config.cutout = crate::config::WebcamCutout::default();
                crate::config::save_config(state);
                crate::config::save_global_hardware_config(state);
                changed = true;
            }

            if ui
                .button("Reset Overlay Position")
                .on_hover_text("Reset webcam overlay screen position and dimensions to default")
                .clicked()
            {
                state.webcam.config.pos_x_pct = 0.72;
                state.webcam.config.pos_y_pct = 0.65;
                state.webcam.config.width_pct = 0.24;
                crate::config::save_config(state);
                crate::config::save_global_hardware_config(state);
                changed = true;
            }
        });
    });

    changed
}
