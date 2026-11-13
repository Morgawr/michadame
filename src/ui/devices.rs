use crate::{
    app::AppState,
    ui::controls::{technical_group, technical_separator},
};
use eframe::egui;

pub fn draw_capture_and_webcam_selectors(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;

    // --- HDMI Capture Card & USB Reset ---
    technical_group(ui, "HDMI Capture Card & USB Reset:", |ui| {
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
                        state.request_capture_formats_probe(Some(ui.ctx().clone()), true);
                        crate::config::save_global_hardware_config(state);
                        changed = true;
                    }
                });

            if state.capture_format_probing {
                ui.add_space(4.0);
                ui.spinner();
            }

            if !state.hardware.supported_formats.is_empty() {
                ui.add_space(6.0);
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
            }
        });

        ui.horizontal_wrapped(|ui| {
            if ui
                .checkbox(
                    &mut state.hardware.video_hwdec,
                    "Hardware Video Decoding (VAAPI / NVDEC)",
                )
                .on_hover_text(
                    "Accelerates compressed video decoding (H.264, HEVC, AV1, VP9, MJPEG) on the GPU via VAAPI/NVDEC/DRM with automatic fallback to CPU software decoding.",
                )
                .changed()
            {
                crate::video::player::set_default_video_hwdec(state.hardware.video_hwdec);
                crate::config::save_config(state);
                crate::config::save_global_hardware_config(state);
                changed = true;
            }
        });

        technical_separator(ui);

        ui.horizontal_wrapped(|ui| {
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
                        crate::config::save_global_hardware_config(state);
                        changed = true;
                    }
                });

            let has_usb = state.hardware.selected_usb_device.is_some();
            let reset_label = if state.usb_reset_in_progress {
                "Resetting USB..."
            } else {
                "Reset USB Device"
            };
            if ui
                .add_enabled(
                    has_usb && !state.usb_reset_in_progress,
                    egui::Button::new(reset_label),
                )
                .clicked()
            {
                state.start_async_usb_reset(Some(ui.ctx().clone()), false);
            }
            if state.usb_reset_in_progress {
                ui.spinner();
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

    // --- Local Game Launcher ---
    technical_group(ui, "Local Game Launcher:", |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Launcher Script:")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            );
            let avail = (ui.available_width() - 135.0).max(120.0);
            if ui
                .add_sized(
                    [avail, 20.0],
                    egui::TextEdit::singleline(&mut state.gamescope_launcher_script)
                        .hint_text("/path/to/michadame-gamescope-steam.sh"),
                )
                .changed()
            {
                crate::config::save_config(state);
                crate::config::save_global_hardware_config(state);
                changed = true;
            }
            if ui
                .add_enabled(
                    !state.ui.is_file_dialog_open(),
                    egui::Button::new("Browse..."),
                )
                .clicked()
            {
                state.open_gamescope_script_dialog(ui.ctx());
            }
            let has_path = !state.gamescope_launcher_script.is_empty();
            if ui
                .add_enabled(has_path, egui::Button::new("Clear"))
                .clicked()
            {
                state.gamescope_launcher_script.clear();
                crate::config::save_config(state);
                crate::config::save_global_hardware_config(state);
                changed = true;
            }
        });
    });

    ui.add_space(4.0);

    // --- Webcam Overlay ---
    technical_group(ui, "Webcam Overlay:", |ui| {
        state.webcam.config.ensure_profiles();
        let is_vtuber_mode =
            state.webcam.config.source_mode == crate::config::WebcamSourceMode::VtuberAvatar;

        ui.horizontal_wrapped(|ui| {
            let mut webcam_enabled = state.webcam.config.enabled;
            if ui
                .checkbox(&mut webcam_enabled, "Enable Webcam")
                .on_hover_text("Enables the webcam or VTuber avatar overlay on the video stream.")
                .changed()
            {
                state.webcam.config.enabled = webcam_enabled;
                if webcam_enabled && !is_vtuber_mode && state.webcam.config.device.is_none() {
                    state.webcam.config.device = state.hardware.video_devices.first().cloned();
                    state.webcam.config.sync_active_local_profile();
                    if state.webcam.config.device.is_some() {
                        state.request_webcam_formats_probe(Some(ui.ctx().clone()), true, false);
                    }
                }
                crate::config::save_config(state);
                crate::config::save_global_hardware_config(state);
                state.sync_webcam_stream(Some(ui.ctx()));
                changed = true;
            }

            ui.add_space(6.0);

            ui.label(
                egui::RichText::new("Source:")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            );
            let source_label = match state.webcam.config.source_mode {
                crate::config::WebcamSourceMode::LocalCamera => "Local Webcam",
                crate::config::WebcamSourceMode::VtuberAvatar => "VTuber Avatar (.vrm)",
            };
            egui::ComboBox::from_id_source("webcam_source_mode_selector")
                .selected_text(source_label)
                .show_ui(ui, |ui| {
                    let prev_mode = state.webcam.config.source_mode;
                    let mut mode_changed = ui
                        .selectable_value(
                            &mut state.webcam.config.source_mode,
                            crate::config::WebcamSourceMode::LocalCamera,
                            "Local Webcam",
                        )
                        .changed();
                    mode_changed |= ui
                        .selectable_value(
                            &mut state.webcam.config.source_mode,
                            crate::config::WebcamSourceMode::VtuberAvatar,
                            "VTuber Avatar (.vrm)",
                        )
                        .changed();
                    if mode_changed && state.webcam.config.source_mode != prev_mode {
                        state.webcam.config.sync_active_local_profile();
                        state.webcam.config.vtuber.sync_active_profile();
                        if state.webcam.config.enabled
                            && state.webcam.config.source_mode
                                == crate::config::WebcamSourceMode::LocalCamera
                            && state.webcam.config.device.is_none()
                        {
                            state.webcam.config.device =
                                state.hardware.video_devices.first().cloned();
                            state.webcam.config.sync_active_local_profile();
                            if state.webcam.config.device.is_some() {
                                state.request_webcam_formats_probe(
                                    Some(ui.ctx().clone()),
                                    true,
                                    false,
                                );
                            }
                        }
                        crate::config::save_config(state);
                        crate::config::save_global_hardware_config(state);
                        state.sync_webcam_stream(Some(ui.ctx()));
                        changed = true;
                    }
                });
        });

        let is_vtuber_now =
            state.webcam.config.source_mode == crate::config::WebcamSourceMode::VtuberAvatar;

        technical_separator(ui);

        if !is_vtuber_now {
            ui.push_id("local_webcam_controls", |ui| {
                // Row L1: Local Webcam Profiles & Presets
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("Webcam Profile:")
                            .monospace()
                            .size(11.0)
                            .strong()
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    );

                    let local_names: Vec<String> =
                        state.webcam.config.local_profiles.keys().cloned().collect();
                    let prev_dev = state.webcam.config.device.clone();
                    let mut switched_local = false;

                    egui::ComboBox::from_id_source("local_webcam_profile_selector")
                        .selected_text(state.webcam.config.local_active_profile.as_str())
                        .show_ui(ui, |ui| {
                            for name in &local_names {
                                let is_sel = &state.webcam.config.local_active_profile == name;
                                if ui.selectable_label(is_sel, name).clicked() && !is_sel {
                                    switched_local |=
                                        state.webcam.config.switch_to_local_profile(name);
                                }
                            }
                        });

                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("New:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    let enter_pressed = ui
                        .add(
                            egui::TextEdit::singleline(&mut state.webcam.new_local_profile_name)
                                .desired_width(96.0)
                                .hint_text("Profile name"),
                        )
                        .lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter));

                    let trimmed_new = state.webcam.new_local_profile_name.trim().to_string();
                    if (ui
                        .add_enabled(!trimmed_new.is_empty(), egui::Button::new("Add"))
                        .on_hover_text(
                            "Save current local webcam settings as a new profile and switch to it",
                        )
                        .clicked()
                        || (enter_pressed && !trimmed_new.is_empty()))
                        && !trimmed_new.is_empty()
                    {
                        state.webcam.config.sync_active_local_profile();
                        let snap = state.webcam.config.snapshot_local_profile();
                        state
                            .webcam
                            .config
                            .local_profiles
                            .insert(trimmed_new.clone(), snap);
                        state.webcam.config.local_active_profile = trimmed_new;
                        state.webcam.new_local_profile_name.clear();
                        crate::config::save_config(state);
                        crate::config::save_global_hardware_config(state);
                        changed = true;
                    }

                    if ui
                        .button("Reset")
                        .on_hover_text(
                            "Reset this local webcam profile's cutout, position, and size to defaults",
                        )
                        .clicked()
                    {
                        let def = crate::config::LocalWebcamProfile::default();
                        state.webcam.config.pos_x_pct = def.pos_x_pct;
                        state.webcam.config.pos_y_pct = def.pos_y_pct;
                        state.webcam.config.width_pct = def.width_pct;
                        state.webcam.config.cutout = def.cutout;
                        state.webcam.config.sync_active_local_profile();
                        crate::config::save_config(state);
                        crate::config::save_global_hardware_config(state);
                        changed = true;
                    }

                    if ui
                        .add_enabled(
                            state.webcam.config.local_profiles.len() > 1,
                            egui::Button::new("Delete"),
                        )
                        .on_hover_text("Delete the active local webcam profile")
                        .clicked()
                    {
                        let removed = state.webcam.config.local_active_profile.clone();
                        state.webcam.config.local_profiles.remove(&removed);
                        let next_name = if state
                            .webcam
                            .config
                            .local_profiles
                            .contains_key(crate::config::DEFAULT_LOCAL_WEBCAM_PROFILE)
                        {
                            crate::config::DEFAULT_LOCAL_WEBCAM_PROFILE.to_string()
                        } else {
                            state
                                .webcam
                                .config
                                .local_profiles
                                .keys()
                                .next()
                                .cloned()
                                .unwrap_or_else(|| {
                                    crate::config::DEFAULT_LOCAL_WEBCAM_PROFILE.to_string()
                                })
                        };
                        state.webcam.config.local_active_profile = next_name.clone();
                        if let Some(p) = state.webcam.config.local_profiles.get(&next_name).cloned()
                        {
                            state.webcam.config.apply_local_profile(&p);
                        }
                        switched_local = true;
                    }

                    if switched_local {
                        if state.webcam.config.device != prev_dev {
                            state.webcam.supported_formats.clear();
                            state.webcam.selected_format_index = 0;
                            if state.webcam.config.device.is_some() {
                                state.request_webcam_formats_probe(
                                    Some(ui.ctx().clone()),
                                    false,
                                    false,
                                );
                            } else {
                                state.webcam.formats_probed_for_device = None;
                            }
                        }
                        crate::config::save_config(state);
                        crate::config::save_global_hardware_config(state);
                        state.sync_webcam_stream(Some(ui.ctx()));
                        changed = true;
                    }
                });

                technical_separator(ui);

                // Row L2: Local Webcam Device, Format, Resolution, Framerate
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("Webcam Device:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );

                    let selected_dev_text =
                        state.webcam.config.device.as_deref().unwrap_or("None");

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
                                if state.webcam.config.device.is_some() {
                                    state.request_webcam_formats_probe(
                                        Some(ui.ctx().clone()),
                                        true,
                                        true,
                                    );
                                } else {
                                    state.webcam.formats_probed_for_device = None;
                                    state.webcam.config.format_fourcc = None;
                                    state.webcam.config.resolution = None;
                                    state.webcam.config.framerate = None;
                                }
                                state.webcam.config.sync_active_local_profile();
                                crate::config::save_config(state);
                                crate::config::save_global_hardware_config(state);
                                state.sync_webcam_stream(Some(ui.ctx()));
                                changed = true;
                            }
                        });

                    if state.webcam.supported_formats.is_empty() && !state.webcam_format_probing {
                        if let Some(dev) = state.webcam.config.device.clone() {
                            if state.webcam.formats_probed_for_device.as_deref()
                                != Some(dev.as_str())
                            {
                                state.request_webcam_formats_probe(
                                    Some(ui.ctx().clone()),
                                    false,
                                    false,
                                );
                            }
                        }
                    }
                    if state.webcam_format_probing {
                        ui.add_space(4.0);
                        ui.spinner();
                    }

                    if !state.webcam.supported_formats.is_empty() {
                        ui.add_space(6.0);
                        let formats = state.webcam.supported_formats.clone();
                        let fmt_idx = state
                            .webcam
                            .selected_format_index
                            .min(formats.len().saturating_sub(1));
                        let selected_format_desc = formats[fmt_idx].description.clone();
                        let resolutions = formats[fmt_idx].resolutions.clone();

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
                                    if ui.selectable_label(is_selected, &f.description).clicked() {
                                        state.webcam.selected_format_index = i;
                                        state.webcam.config.format_fourcc = Some(f.fourcc.clone());
                                        if let Some(res) = f.resolutions.first() {
                                            state.webcam.config.resolution =
                                                Some((res.width, res.height));
                                            state.webcam.config.framerate =
                                                res.framerates.first().cloned();
                                        }
                                        state.webcam.config.sync_active_local_profile();
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
                                        .selectable_label(
                                            is_selected,
                                            format!("{}x{}", res.width, res.height),
                                        )
                                        .clicked()
                                    {
                                        state.webcam.config.resolution =
                                            Some((res.width, res.height));
                                        state.webcam.config.framerate =
                                            res.framerates.first().cloned();
                                        state.webcam.config.sync_active_local_profile();
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
                                            state.webcam.config.sync_active_local_profile();
                                            crate::config::save_config(state);
                                            crate::config::save_global_hardware_config(state);
                                            state.sync_webcam_stream(Some(ui.ctx()));
                                            changed = true;
                                        }
                                    }
                                });
                        }
                    }
                });

                technical_separator(ui);

                // Row L3: Local Webcam Cut Out & Position
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("Cut Out (px):")
                            .monospace()
                            .size(11.0)
                            .strong()
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    );

                    let (raw_w, raw_h) = state.webcam.config.resolution.unwrap_or((640, 480));
                    let max_x = raw_w.saturating_sub(16);
                    let max_y = raw_h.saturating_sub(16);
                    let mut cut_changed = false;

                    ui.label(
                        egui::RichText::new("Left:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    let max_l = max_x.saturating_sub(state.webcam.config.cutout.right);
                    let mut cut_l = state.webcam.config.cutout.left;
                    if ui
                        .add(
                            egui::DragValue::new(&mut cut_l)
                                .clamp_range(0..=max_l)
                                .speed(1.0),
                        )
                        .changed()
                    {
                        state.webcam.config.cutout.left = cut_l;
                        cut_changed = true;
                    }

                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Right:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    let max_r = max_x.saturating_sub(state.webcam.config.cutout.left);
                    let mut cut_r = state.webcam.config.cutout.right;
                    if ui
                        .add(
                            egui::DragValue::new(&mut cut_r)
                                .clamp_range(0..=max_r)
                                .speed(1.0),
                        )
                        .changed()
                    {
                        state.webcam.config.cutout.right = cut_r;
                        cut_changed = true;
                    }

                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Top:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    let max_t = max_y.saturating_sub(state.webcam.config.cutout.bottom);
                    let mut cut_t = state.webcam.config.cutout.top;
                    if ui
                        .add(
                            egui::DragValue::new(&mut cut_t)
                                .clamp_range(0..=max_t)
                                .speed(1.0),
                        )
                        .changed()
                    {
                        state.webcam.config.cutout.top = cut_t;
                        cut_changed = true;
                    }

                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Bottom:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    let max_b = max_y.saturating_sub(state.webcam.config.cutout.top);
                    let mut cut_b = state.webcam.config.cutout.bottom;
                    if ui
                        .add(
                            egui::DragValue::new(&mut cut_b)
                                .clamp_range(0..=max_b)
                                .speed(1.0),
                        )
                        .changed()
                    {
                        state.webcam.config.cutout.bottom = cut_b;
                        cut_changed = true;
                    }

                    ui.add_space(8.0);
                    if ui
                        .button("Reset Cutout")
                        .on_hover_text("Reset shaved pixels on all sides back to 0")
                        .clicked()
                    {
                        state.webcam.config.cutout = crate::config::WebcamCutout::default();
                        cut_changed = true;
                    }

                    if ui
                        .button("Reset Overlay Position")
                        .on_hover_text(
                            "Reset local webcam overlay screen position and dimensions to default",
                        )
                        .clicked()
                    {
                        state.webcam.config.pos_x_pct = 0.72;
                        state.webcam.config.pos_y_pct = 0.65;
                        state.webcam.config.width_pct = 0.24;
                        cut_changed = true;
                    }

                    if cut_changed {
                        state.webcam.config.sync_active_local_profile();
                        crate::config::save_config(state);
                        crate::config::save_global_hardware_config(state);
                        changed = true;
                    }
                });
            });
        } else {
            ui.push_id("vtuber_avatar_controls", |ui| {
                // Row V1: VTuber Profiles & Presets
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("VTuber Profile:")
                            .monospace()
                            .size(11.0)
                            .strong()
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    );

                    let vtuber_names: Vec<String> =
                        state.webcam.config.vtuber.profiles.keys().cloned().collect();
                    let prev_tracking_dev = state.webcam.config.vtuber.tracking_device.clone();
                    let mut switched_vtuber = false;

                    egui::ComboBox::from_id_source("vtuber_profile_selector")
                        .selected_text(state.webcam.config.vtuber.active_profile.as_str())
                        .show_ui(ui, |ui| {
                            for name in &vtuber_names {
                                let is_sel = &state.webcam.config.vtuber.active_profile == name;
                                if ui.selectable_label(is_sel, name).clicked() && !is_sel {
                                    switched_vtuber |=
                                        state.webcam.config.vtuber.switch_to_profile(name);
                                }
                            }
                        });

                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("New:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    let enter_pressed = ui
                        .add(
                            egui::TextEdit::singleline(&mut state.webcam.new_vtuber_profile_name)
                                .desired_width(96.0)
                                .hint_text("Profile name"),
                        )
                        .lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter));

                    let trimmed_new = state.webcam.new_vtuber_profile_name.trim().to_string();
                    if (ui
                        .add_enabled(!trimmed_new.is_empty(), egui::Button::new("Add"))
                        .on_hover_text(
                            "Save current VTuber avatar settings and layout as a new profile and switch to it",
                        )
                        .clicked()
                        || (enter_pressed && !trimmed_new.is_empty()))
                        && !trimmed_new.is_empty()
                    {
                        state.webcam.config.vtuber.sync_active_profile();
                        let snap = state.webcam.config.vtuber.snapshot_profile();
                        state
                            .webcam
                            .config
                            .vtuber
                            .profiles
                            .insert(trimmed_new.clone(), snap);
                        state.webcam.config.vtuber.active_profile = trimmed_new;
                        state.webcam.new_vtuber_profile_name.clear();
                        crate::config::save_config(state);
                        crate::config::save_global_hardware_config(state);
                        changed = true;
                    }

                    if ui
                        .button("Reset")
                        .on_hover_text(
                            "Reset this VTuber profile's style, rig, cutout, and overlay layout (keeps VRM model and tracking camera)",
                        )
                        .clicked()
                    {
                        let keep_model = state.webcam.config.vtuber.model_path.clone();
                        let keep_dev = state.webcam.config.vtuber.tracking_device.clone();
                        let keep_fourcc = state.webcam.config.vtuber.tracking_format_fourcc.clone();
                        let keep_res = state.webcam.config.vtuber.tracking_resolution;
                        let keep_fps = state.webcam.config.vtuber.tracking_framerate;
                        let use_cutout_defaults = state.webcam.config.vtuber.active_profile
                            == crate::config::VTUBER_PROFILE_CUTOUT
                            || (state.webcam.config.vtuber.active_profile
                                != crate::config::VTUBER_PROFILE_FRAMED
                                && state.webcam.config.vtuber.display_style
                                    == crate::config::VtuberDisplayStyle::BorderlessCutout);
                        let mut def = if use_cutout_defaults {
                            crate::config::VtuberProfile::default_cutout()
                        } else {
                            crate::config::VtuberProfile::default_framed()
                        };
                        def.model_path = keep_model;
                        def.tracking_device = keep_dev;
                        def.tracking_format_fourcc = keep_fourcc;
                        def.tracking_resolution = keep_res;
                        def.tracking_framerate = keep_fps;
                        state.webcam.config.vtuber.apply_profile(&def);
                        state.webcam.config.vtuber.sync_active_profile();
                        state.webcam.vtuber.sync_assets_with_config(
                            &state.webcam.config.vtuber,
                            Some(ui.ctx().clone()),
                        );
                        crate::config::save_config(state);
                        crate::config::save_global_hardware_config(state);
                        changed = true;
                    }

                    if ui
                        .add_enabled(
                            state.webcam.config.vtuber.profiles.len() > 1,
                            egui::Button::new("Delete"),
                        )
                        .on_hover_text("Delete the active VTuber profile")
                        .clicked()
                    {
                        let removed = state.webcam.config.vtuber.active_profile.clone();
                        state.webcam.config.vtuber.profiles.remove(&removed);
                        let next_name = if state
                            .webcam
                            .config
                            .vtuber
                            .profiles
                            .contains_key(crate::config::VTUBER_PROFILE_FRAMED)
                        {
                            crate::config::VTUBER_PROFILE_FRAMED.to_string()
                        } else if state
                            .webcam
                            .config
                            .vtuber
                            .profiles
                            .contains_key(crate::config::VTUBER_PROFILE_CUTOUT)
                        {
                            crate::config::VTUBER_PROFILE_CUTOUT.to_string()
                        } else {
                            state
                                .webcam
                                .config
                                .vtuber
                                .profiles
                                .keys()
                                .next()
                                .cloned()
                                .unwrap_or_else(|| crate::config::VTUBER_PROFILE_FRAMED.to_string())
                        };
                        state.webcam.config.vtuber.active_profile = next_name.clone();
                        if let Some(p) =
                            state.webcam.config.vtuber.profiles.get(&next_name).cloned()
                        {
                            state.webcam.config.vtuber.apply_profile(&p);
                        }
                        switched_vtuber = true;
                    }

                    if switched_vtuber {
                        if state.webcam.config.vtuber.tracking_device != prev_tracking_dev {
                            state.webcam.vtuber_supported_formats.clear();
                            state.webcam.vtuber_selected_format_index = 0;
                            if state.webcam.config.vtuber.tracking_device.is_some() {
                                state.request_vtuber_formats_probe(
                                    Some(ui.ctx().clone()),
                                    false,
                                    false,
                                );
                            } else {
                                state.webcam.vtuber_formats_probed_for_device = None;
                            }
                        }
                        state.webcam.vtuber.sync_assets_with_config(
                            &state.webcam.config.vtuber,
                            Some(ui.ctx().clone()),
                        );
                        crate::config::save_config(state);
                        crate::config::save_global_hardware_config(state);
                        state.sync_webcam_stream(Some(ui.ctx()));
                        changed = true;
                    }
                });

                technical_separator(ui);

                // Row V2: Dedicated VTuber Tracking Camera (independent of Local Webcam device)
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("Tracking Camera:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );

                    let none_label = "None (Idle / Mic)";
                    let selected_dev_text = state
                        .webcam
                        .config
                        .vtuber
                        .tracking_device
                        .as_deref()
                        .unwrap_or(none_label);

                    egui::ComboBox::from_id_source("vtuber_tracking_device_selector")
                        .selected_text(selected_dev_text)
                        .show_ui(ui, |ui| {
                            let mut dev_changed = ui
                                .selectable_value(
                                    &mut state.webcam.config.vtuber.tracking_device,
                                    None,
                                    none_label,
                                )
                                .changed();
                            for device in &state.hardware.video_devices {
                                dev_changed |= ui
                                    .selectable_value(
                                        &mut state.webcam.config.vtuber.tracking_device,
                                        Some(device.clone()),
                                        device.as_str(),
                                    )
                                    .changed();
                            }
                            if dev_changed {
                                state.webcam.vtuber_supported_formats.clear();
                                state.webcam.vtuber_selected_format_index = 0;
                                if state.webcam.config.vtuber.tracking_device.is_some() {
                                    state.request_vtuber_formats_probe(
                                        Some(ui.ctx().clone()),
                                        true,
                                        true,
                                    );
                                } else {
                                    state.webcam.vtuber_formats_probed_for_device = None;
                                    state.webcam.config.vtuber.tracking_format_fourcc = None;
                                    state.webcam.config.vtuber.tracking_resolution = None;
                                    state.webcam.config.vtuber.tracking_framerate = None;
                                }
                                state.webcam.config.vtuber.sync_active_profile();
                                crate::config::save_config(state);
                                crate::config::save_global_hardware_config(state);
                                state.sync_webcam_stream(Some(ui.ctx()));
                                changed = true;
                            }
                        });

                    if state.webcam.vtuber_supported_formats.is_empty()
                        && !state.webcam.vtuber_format_probing
                    {
                        if let Some(dev) = state.webcam.config.vtuber.tracking_device.clone() {
                            if state.webcam.vtuber_formats_probed_for_device.as_deref()
                                != Some(dev.as_str())
                            {
                                state.request_vtuber_formats_probe(
                                    Some(ui.ctx().clone()),
                                    false,
                                    false,
                                );
                            }
                        }
                    }
                    if state.webcam.vtuber_format_probing {
                        ui.add_space(4.0);
                        ui.spinner();
                    }

                    if !state.webcam.vtuber_supported_formats.is_empty() {
                        ui.add_space(6.0);
                        let formats = state.webcam.vtuber_supported_formats.clone();
                        let fmt_idx = state
                            .webcam
                            .vtuber_selected_format_index
                            .min(formats.len().saturating_sub(1));
                        let selected_format_desc = formats[fmt_idx].description.clone();
                        let resolutions = formats[fmt_idx].resolutions.clone();

                        ui.label(
                            egui::RichText::new("Format:")
                                .monospace()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(170, 170, 170)),
                        );

                        egui::ComboBox::from_id_source("vtuber_format_selector")
                            .selected_text(selected_format_desc)
                            .show_ui(ui, |ui| {
                                for (i, f) in formats.iter().enumerate() {
                                    let is_selected =
                                        state.webcam.vtuber_selected_format_index == i;
                                    if ui.selectable_label(is_selected, &f.description).clicked() {
                                        state.webcam.vtuber_selected_format_index = i;
                                        state.webcam.config.vtuber.tracking_format_fourcc =
                                            Some(f.fourcc.clone());
                                        if let Some(res) = f.resolutions.first() {
                                            state.webcam.config.vtuber.tracking_resolution =
                                                Some((res.width, res.height));
                                            state.webcam.config.vtuber.tracking_framerate =
                                                res.framerates.first().cloned();
                                        }
                                        state.webcam.config.vtuber.sync_active_profile();
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

                        let current_res = state
                            .webcam
                            .config
                            .vtuber
                            .tracking_resolution
                            .unwrap_or((0, 0));
                        let res_text = if current_res.0 > 0 {
                            format!("{}x{}", current_res.0, current_res.1)
                        } else {
                            "Default".to_string()
                        };

                        egui::ComboBox::from_id_source("vtuber_resolution_selector")
                            .selected_text(res_text)
                            .show_ui(ui, |ui| {
                                for res in &resolutions {
                                    let is_selected = current_res == (res.width, res.height);
                                    if ui
                                        .selectable_label(
                                            is_selected,
                                            format!("{}x{}", res.width, res.height),
                                        )
                                        .clicked()
                                    {
                                        state.webcam.config.vtuber.tracking_resolution =
                                            Some((res.width, res.height));
                                        state.webcam.config.vtuber.tracking_framerate =
                                            res.framerates.first().cloned();
                                        state.webcam.config.vtuber.sync_active_profile();
                                        crate::config::save_config(state);
                                        crate::config::save_global_hardware_config(state);
                                        state.sync_webcam_stream(Some(ui.ctx()));
                                        changed = true;
                                    }
                                }
                            });

                        let selected_framerates = resolutions
                            .iter()
                            .find(|r| {
                                Some((r.width, r.height))
                                    == state.webcam.config.vtuber.tracking_resolution
                            })
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

                            let current_fps =
                                state.webcam.config.vtuber.tracking_framerate.unwrap_or(0);
                            let fps_text = if current_fps > 0 {
                                format!("{} fps", current_fps)
                            } else {
                                "Default".to_string()
                            };

                            egui::ComboBox::from_id_source("vtuber_framerate_selector")
                                .selected_text(fps_text)
                                .show_ui(ui, |ui| {
                                    for &fps in &selected_framerates {
                                        let is_selected = current_fps == fps;
                                        if ui
                                            .selectable_label(is_selected, format!("{} fps", fps))
                                            .clicked()
                                        {
                                            state.webcam.config.vtuber.tracking_framerate =
                                                Some(fps);
                                            state.webcam.config.vtuber.sync_active_profile();
                                            crate::config::save_config(state);
                                            crate::config::save_global_hardware_config(state);
                                            state.sync_webcam_stream(Some(ui.ctx()));
                                            changed = true;
                                        }
                                    }
                                });
                        }
                    }
                });

                technical_separator(ui);

                // Row V3: VRM Avatar Model & Neural Face Tracker
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("VRM Avatar:")
                            .monospace()
                            .size(11.0)
                            .strong()
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    );

                    if state.webcam.vtuber.loading_model_path.is_some() {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("Loading...")
                                .monospace()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(225, 195, 120)),
                        );
                    } else {
                        let (model_text, model_color) =
                            if let Some(title) = &state.webcam.vtuber.avatar_title {
                                (title.clone(), egui::Color32::from_rgb(140, 225, 165))
                            } else if let Some(path_str) = &state.webcam.config.vtuber.model_path {
                                let fname = std::path::Path::new(path_str)
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or(path_str.as_str())
                                    .to_string();
                                (fname, egui::Color32::from_rgb(215, 215, 225))
                            } else {
                                (
                                    "None (.vrm)".to_string(),
                                    egui::Color32::from_rgb(135, 135, 145),
                                )
                            };
                        ui.label(
                            egui::RichText::new(model_text)
                                .monospace()
                                .size(11.0)
                                .color(model_color),
                        );
                    }

                    let can_open_dialog =
                        !state.ui.is_file_dialog_open() && state.steam_deck.is_none();
                    if ui
                        .add_enabled(can_open_dialog, egui::Button::new("Select VRM..."))
                        .on_hover_text(
                            "Pick a VRM 0.x or VRM 1.0 (.vrm) 3D avatar file, or drag & drop a .vrm onto the app window.",
                        )
                        .clicked()
                    {
                        state.open_vtuber_model_dialog(ui.ctx());
                    }

                    if ui
                        .add_enabled(
                            state.webcam.config.vtuber.model_path.is_some(),
                            egui::Button::new("Clear"),
                        )
                        .clicked()
                    {
                        state.webcam.config.vtuber.model_path = None;
                        state.webcam.config.vtuber.sync_active_profile();
                        state.webcam.vtuber.sync_assets_with_config(
                            &state.webcam.config.vtuber,
                            Some(ui.ctx().clone()),
                        );
                        crate::config::save_config(state);
                        crate::config::save_global_hardware_config(state);
                        changed = true;
                    }

                    ui.add_space(8.0);

                    ui.label(
                        egui::RichText::new("Tracker:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );

                    let tracker_status = state
                        .webcam
                        .vtuber
                        .tracker_status(state.webcam.config.vtuber.tracking_device.is_some());
                    let status_color = match &tracker_status {
                        crate::vtuber::tracker::VtuberTrackerStatus::Tracking => {
                            egui::Color32::from_rgb(120, 225, 150)
                        }
                        crate::vtuber::tracker::VtuberTrackerStatus::AcquiringFace
                        | crate::vtuber::tracker::VtuberTrackerStatus::LoadingModels => {
                            egui::Color32::from_rgb(235, 195, 110)
                        }
                        crate::vtuber::tracker::VtuberTrackerStatus::IdleNoCamera => {
                            egui::Color32::from_rgb(155, 160, 180)
                        }
                        crate::vtuber::tracker::VtuberTrackerStatus::ModelsMissing
                        | crate::vtuber::tracker::VtuberTrackerStatus::Error(_) => {
                            egui::Color32::from_rgb(235, 125, 125)
                        }
                    };
                    let status_resp = ui.label(
                        egui::RichText::new(tracker_status.short_label())
                            .monospace()
                            .size(11.0)
                            .color(status_color),
                    );
                    if let crate::vtuber::tracker::VtuberTrackerStatus::Error(err) = &tracker_status
                    {
                        status_resp.on_hover_text(err);
                    }

                    let models_installed = state.webcam.vtuber.are_models_installed();
                    if !models_installed {
                        if state.webcam.vtuber.sync_in_progress {
                            ui.spinner();
                            if let Some((msg, prog)) = &state.webcam.vtuber.sync_progress {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} ({:.0}%)",
                                        msg,
                                        (prog * 100.0).clamp(0.0, 100.0)
                                    ))
                                    .monospace()
                                    .size(10.5)
                                    .color(egui::Color32::from_rgb(210, 195, 140)),
                                );
                            }
                        } else if ui
                            .button("Download Face Tracker")
                            .on_hover_text(
                                "Download BlazeFace + MediaPipe 478-landmark ONNX face tracking weights (~5.2 MB) for camera head/eye/mouth tracking.",
                            )
                            .clicked()
                        {
                            state
                                .webcam
                                .vtuber
                                .start_model_download(Some(ui.ctx().clone()));
                        }
                    } else {
                        if ui
                            .button("Calibrate Neutral")
                            .on_hover_text(
                                "Set your current camera head posture as the forward-facing neutral pose.",
                            )
                            .clicked()
                        {
                            state.webcam.config.vtuber.neutral_head_quat = Some(
                                state
                                    .webcam
                                    .vtuber
                                    .rig_animator
                                    .current_raw_head_quat_array(),
                            );
                            state.webcam.config.vtuber.sync_active_profile();
                            crate::config::save_config(state);
                            crate::config::save_global_hardware_config(state);
                            changed = true;
                        }
                        if ui
                            .add_enabled(
                                state.webcam.config.vtuber.neutral_head_quat.is_some(),
                                egui::Button::new("Reset Neutral"),
                            )
                            .clicked()
                        {
                            state.webcam.config.vtuber.neutral_head_quat = None;
                            state.webcam.config.vtuber.sync_active_profile();
                            crate::config::save_config(state);
                            crate::config::save_global_hardware_config(state);
                            changed = true;
                        }
                    }
                });

                if let Some(err) = &state.webcam.vtuber.model_error {
                    ui.label(
                        egui::RichText::new(format!("Model Error: {err}"))
                            .monospace()
                            .size(10.5)
                            .color(egui::Color32::from_rgb(235, 115, 115)),
                    );
                }
                if let Some(err) = &state.webcam.vtuber.sync_error {
                    ui.label(
                        egui::RichText::new(format!("Tracker Download Error: {err}"))
                            .monospace()
                            .size(10.5)
                            .color(egui::Color32::from_rgb(235, 115, 115)),
                    );
                }

                technical_separator(ui);

                // Row V4: Presentation Style, Virtual Aspect & Background
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("Style:")
                            .monospace()
                            .size(11.0)
                            .strong()
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    );

                    let style_label = match state.webcam.config.vtuber.display_style {
                        crate::config::VtuberDisplayStyle::FramedWebcam => "Framed Webcam",
                        crate::config::VtuberDisplayStyle::BorderlessCutout => "Borderless Cutout",
                    };
                    egui::ComboBox::from_id_source("vtuber_display_style_selector")
                        .selected_text(style_label)
                        .show_ui(ui, |ui| {
                            let mut style_changed = ui
                                .selectable_value(
                                    &mut state.webcam.config.vtuber.display_style,
                                    crate::config::VtuberDisplayStyle::FramedWebcam,
                                    "Framed Webcam",
                                )
                                .on_hover_text(
                                    "Draws the VTuber avatar inside the retro window frame over your chosen background.",
                                )
                                .changed();
                            style_changed |= ui
                                .selectable_value(
                                    &mut state.webcam.config.vtuber.display_style,
                                    crate::config::VtuberDisplayStyle::BorderlessCutout,
                                    "Borderless Cutout",
                                )
                                .on_hover_text(
                                    "Greenscreen-free character cutout with zero window borders (drag/resize handles stay hidden on stream & replays).",
                                )
                                .changed();
                            if style_changed {
                                state.webcam.config.vtuber.sync_active_profile();
                                crate::config::save_config(state);
                                crate::config::save_global_hardware_config(state);
                                changed = true;
                            }
                        });

                    ui.add_space(6.0);

                    ui.label(
                        egui::RichText::new("Aspect:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    let current_aspect = state.webcam.config.vtuber.active_layout().aspect_preset;
                    egui::ComboBox::from_id_source("vtuber_aspect_preset_selector")
                        .selected_text(current_aspect.label())
                        .show_ui(ui, |ui| {
                            let mut aspect_changed = false;
                            let mut next_aspect = current_aspect;
                            for preset in crate::config::VtuberAspectPreset::ALL {
                                aspect_changed |= ui
                                    .selectable_value(&mut next_aspect, preset, preset.label())
                                    .changed();
                            }
                            if aspect_changed {
                                state.webcam.config.vtuber.active_layout_mut().aspect_preset =
                                    next_aspect;
                                state.webcam.config.vtuber.sync_active_profile();
                                crate::config::save_config(state);
                                crate::config::save_global_hardware_config(state);
                                changed = true;
                            }
                        });

                    ui.add_space(6.0);

                    if state.webcam.config.vtuber.display_style
                        == crate::config::VtuberDisplayStyle::FramedWebcam
                    {
                        ui.label(
                            egui::RichText::new("Background:")
                                .monospace()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(170, 170, 170)),
                        );

                        let bg_label = match state.webcam.config.vtuber.background_kind {
                            crate::config::VtuberBackgroundKind::SolidColor => "Solid Color",
                            crate::config::VtuberBackgroundKind::StaticImage => "Static Image",
                            crate::config::VtuberBackgroundKind::Transparent => "Transparent",
                        };
                        egui::ComboBox::from_id_source("vtuber_background_kind_selector")
                            .selected_text(bg_label)
                            .show_ui(ui, |ui| {
                                let mut bg_changed = ui
                                    .selectable_value(
                                        &mut state.webcam.config.vtuber.background_kind,
                                        crate::config::VtuberBackgroundKind::SolidColor,
                                        "Solid Color",
                                    )
                                    .changed();
                                bg_changed |= ui
                                    .selectable_value(
                                        &mut state.webcam.config.vtuber.background_kind,
                                        crate::config::VtuberBackgroundKind::StaticImage,
                                        "Static Image",
                                    )
                                    .changed();
                                bg_changed |= ui
                                    .selectable_value(
                                        &mut state.webcam.config.vtuber.background_kind,
                                        crate::config::VtuberBackgroundKind::Transparent,
                                        "Transparent",
                                    )
                                    .changed();
                                if bg_changed {
                                    state.webcam.config.vtuber.sync_active_profile();
                                    crate::config::save_config(state);
                                    crate::config::save_global_hardware_config(state);
                                    changed = true;
                                }
                            });

                        match state.webcam.config.vtuber.background_kind {
                            crate::config::VtuberBackgroundKind::SolidColor => {
                                if ui
                                    .color_edit_button_srgba_unmultiplied(
                                        &mut state.webcam.config.vtuber.background_color,
                                    )
                                    .changed()
                                {
                                    state.webcam.config.vtuber.sync_active_profile();
                                    crate::config::save_config(state);
                                    crate::config::save_global_hardware_config(state);
                                    changed = true;
                                }
                            }
                            crate::config::VtuberBackgroundKind::StaticImage => {
                                if state.webcam.vtuber.loading_bg_path.is_some() {
                                    ui.spinner();
                                } else {
                                    let bg_name = state
                                        .webcam
                                        .config
                                        .vtuber
                                        .background_image_path
                                        .as_deref()
                                        .and_then(|p| std::path::Path::new(p).file_name())
                                        .and_then(|s| s.to_str())
                                        .unwrap_or("None");
                                    ui.label(
                                        egui::RichText::new(bg_name)
                                            .monospace()
                                            .size(11.0)
                                            .color(egui::Color32::from_rgb(200, 200, 210)),
                                    );
                                }
                                let can_open_bg =
                                    !state.ui.is_file_dialog_open() && state.steam_deck.is_none();
                                if ui
                                    .add_enabled(can_open_bg, egui::Button::new("Select Image..."))
                                    .on_hover_text("Choose a .png, .jpg, or .webp background image")
                                    .clicked()
                                {
                                    state.open_vtuber_background_dialog(ui.ctx());
                                }
                                if ui
                                    .add_enabled(
                                        state.webcam.config.vtuber.background_image_path.is_some(),
                                        egui::Button::new("Clear"),
                                    )
                                    .clicked()
                                {
                                    state.webcam.config.vtuber.background_image_path = None;
                                    state.webcam.config.vtuber.sync_active_profile();
                                    state.webcam.vtuber.sync_assets_with_config(
                                        &state.webcam.config.vtuber,
                                        Some(ui.ctx().clone()),
                                    );
                                    crate::config::save_config(state);
                                    crate::config::save_global_hardware_config(state);
                                    changed = true;
                                }
                            }
                            _ => {}
                        }
                    } else {
                        ui.label(
                            egui::RichText::new("Transparent alpha cutout (borderless)")
                                .monospace()
                                .size(10.5)
                                .color(egui::Color32::from_rgb(145, 150, 168)),
                        );
                    }
                });

                if let Some(bg_err) = &state.webcam.vtuber.bg_error {
                    ui.label(
                        egui::RichText::new(format!("Background Image Error: {bg_err}"))
                            .monospace()
                            .size(10.5)
                            .color(egui::Color32::from_rgb(235, 115, 115)),
                    );
                }

                technical_separator(ui);

                // Row V5: Camera Framing & Rig Tuning
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("Avatar Rig:")
                            .monospace()
                            .size(11.0)
                            .strong()
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    );

                    let mut rig_changed = false;

                    ui.label(
                        egui::RichText::new("Zoom:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    rig_changed |= ui
                        .add(
                            egui::DragValue::new(&mut state.webcam.config.vtuber.camera_zoom)
                                .clamp_range(0.5..=2.5)
                                .speed(0.02)
                                .fixed_decimals(2),
                        )
                        .changed();

                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Y Offset:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    rig_changed |= ui
                        .add(
                            egui::DragValue::new(&mut state.webcam.config.vtuber.camera_offset_y)
                                .clamp_range(-0.5..=0.5)
                                .speed(0.005)
                                .fixed_decimals(2),
                        )
                        .changed();

                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Smoothing:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    rig_changed |= ui
                        .add(
                            egui::DragValue::new(&mut state.webcam.config.vtuber.smoothing)
                                .clamp_range(0.0..=0.95)
                                .speed(0.01)
                                .fixed_decimals(2),
                        )
                        .changed();

                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Motion:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    rig_changed |= ui
                        .add(
                            egui::DragValue::new(&mut state.webcam.config.vtuber.movement_scale)
                                .clamp_range(0.2..=2.0)
                                .speed(0.02)
                                .fixed_decimals(2),
                        )
                        .changed();

                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Blink:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    rig_changed |= ui
                        .add(
                            egui::DragValue::new(&mut state.webcam.config.vtuber.blink_sensitivity)
                                .clamp_range(0.4..=2.0)
                                .speed(0.02)
                                .fixed_decimals(2),
                        )
                        .changed();

                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Mouth:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    rig_changed |= ui
                        .add(
                            egui::DragValue::new(&mut state.webcam.config.vtuber.mouth_sensitivity)
                                .clamp_range(0.4..=2.5)
                                .speed(0.02)
                                .fixed_decimals(2),
                        )
                        .changed();

                    ui.add_space(6.0);
                    rig_changed |= ui
                        .checkbox(&mut state.webcam.config.vtuber.mirror_tracking, "Mirror")
                        .on_hover_text("Mirror head, eye, and blink movements like a mirror.")
                        .changed();

                    rig_changed |= ui
                        .checkbox(
                            &mut state.webcam.config.vtuber.mic_lipsync_assist,
                            "Mic Lip-Sync",
                        )
                        .on_hover_text(
                            "Blends active microphone voice energy into avatar mouth movement.",
                        )
                        .changed();

                    if ui
                        .button("Reset Rig")
                        .on_hover_text("Reset avatar camera framing and tracking sensitivities")
                        .clicked()
                    {
                        let def = crate::config::VtuberConfig::default();
                        state.webcam.config.vtuber.camera_zoom = def.camera_zoom;
                        state.webcam.config.vtuber.camera_offset_y = def.camera_offset_y;
                        state.webcam.config.vtuber.smoothing = def.smoothing;
                        state.webcam.config.vtuber.movement_scale = def.movement_scale;
                        state.webcam.config.vtuber.blink_sensitivity = def.blink_sensitivity;
                        state.webcam.config.vtuber.mouth_sensitivity = def.mouth_sensitivity;
                        rig_changed = true;
                    }

                    if rig_changed {
                        state.webcam.config.vtuber.sync_active_profile();
                        crate::config::save_config(state);
                        crate::config::save_global_hardware_config(state);
                        changed = true;
                    }
                });

                technical_separator(ui);

                // Row V6: Dedicated VTuber Cut Out & Overlay Position (independent of Local Webcam)
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("VTuber Cut Out (px):")
                            .monospace()
                            .size(11.0)
                            .strong()
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    );

                    let (raw_w, raw_h) = state
                        .webcam
                        .config
                        .vtuber
                        .active_layout()
                        .aspect_preset
                        .virtual_dimensions();
                    let max_x = raw_w.saturating_sub(16);
                    let max_y = raw_h.saturating_sub(16);
                    let mut vcut_changed = false;

                    let cur_cut = state.webcam.config.vtuber.active_layout().cutout.clone();

                    ui.label(
                        egui::RichText::new("Left:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    let max_l = max_x.saturating_sub(cur_cut.right);
                    let mut cut_l = cur_cut.left;
                    if ui
                        .add(
                            egui::DragValue::new(&mut cut_l)
                                .clamp_range(0..=max_l)
                                .speed(1.0),
                        )
                        .changed()
                    {
                        state.webcam.config.vtuber.active_layout_mut().cutout.left = cut_l;
                        vcut_changed = true;
                    }

                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Right:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    let max_r = max_x.saturating_sub(
                        state.webcam.config.vtuber.active_layout().cutout.left,
                    );
                    let mut cut_r = cur_cut.right;
                    if ui
                        .add(
                            egui::DragValue::new(&mut cut_r)
                                .clamp_range(0..=max_r)
                                .speed(1.0),
                        )
                        .changed()
                    {
                        state.webcam.config.vtuber.active_layout_mut().cutout.right = cut_r;
                        vcut_changed = true;
                    }

                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Top:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    let max_t = max_y.saturating_sub(cur_cut.bottom);
                    let mut cut_t = cur_cut.top;
                    if ui
                        .add(
                            egui::DragValue::new(&mut cut_t)
                                .clamp_range(0..=max_t)
                                .speed(1.0),
                        )
                        .changed()
                    {
                        state.webcam.config.vtuber.active_layout_mut().cutout.top = cut_t;
                        vcut_changed = true;
                    }

                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Bottom:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    let max_b = max_y.saturating_sub(
                        state.webcam.config.vtuber.active_layout().cutout.top,
                    );
                    let mut cut_b = cur_cut.bottom;
                    if ui
                        .add(
                            egui::DragValue::new(&mut cut_b)
                                .clamp_range(0..=max_b)
                                .speed(1.0),
                        )
                        .changed()
                    {
                        state
                            .webcam
                            .config
                            .vtuber
                            .active_layout_mut()
                            .cutout
                            .bottom = cut_b;
                        vcut_changed = true;
                    }

                    ui.add_space(8.0);
                    if ui
                        .button("Reset Cutout")
                        .on_hover_text("Reset VTuber shaved pixels on all sides back to 0")
                        .clicked()
                    {
                        state.webcam.config.vtuber.active_layout_mut().cutout =
                            crate::config::WebcamCutout::default();
                        vcut_changed = true;
                    }

                    if ui
                        .button("Reset Overlay Position")
                        .on_hover_text(
                            "Reset VTuber overlay screen position and dimensions to default",
                        )
                        .clicked()
                    {
                        let def_layout = match state.webcam.config.vtuber.display_style {
                            crate::config::VtuberDisplayStyle::FramedWebcam => {
                                crate::config::VtuberOverlayLayout::default_framed()
                            }
                            crate::config::VtuberDisplayStyle::BorderlessCutout => {
                                crate::config::VtuberOverlayLayout::default_cutout()
                            }
                        };
                        let layout = state.webcam.config.vtuber.active_layout_mut();
                        layout.pos_x_pct = def_layout.pos_x_pct;
                        layout.pos_y_pct = def_layout.pos_y_pct;
                        layout.width_pct = def_layout.width_pct;
                        vcut_changed = true;
                    }

                    if vcut_changed {
                        state.webcam.config.vtuber.sync_active_profile();
                        crate::config::save_config(state);
                        crate::config::save_global_hardware_config(state);
                        changed = true;
                    }
                });
            });
        }
    });

    changed
}

pub fn draw_audio_device_controls(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;

    let selected_source_desc = state
        .hardware
        .audio_sources
        .iter()
        .find(|(_, name)| Some(name) == state.hardware.selected_audio_source_name.as_ref())
        .map(|(desc, _)| desc.clone())
        .unwrap_or_else(|| "Select an Input".to_string());

    ui.horizontal_wrapped(|ui| {
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

        ui.add_space(6.0);

        ui.label(
            egui::RichText::new("Buffer:")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(170, 170, 170)),
        );
        let buffer_sizes = [32, 64, 128, 256, 512, 1024, 2048, 4096];
        let current_size = state.hardware.audio_buffer_size;

        egui::ComboBox::from_id_source("audio_buffer_size")
            .selected_text(format!("{} smp", current_size))
            .width(82.0)
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
            egui::RichText::new("Rate:")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(170, 170, 170)),
        );
        let sample_rates = [44100, 48000];
        let current_rate = state.hardware.audio_sample_rate;

        egui::ComboBox::from_id_source("audio_sample_rate")
            .selected_text(format!("{} Hz", current_rate))
            .width(82.0)
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
            .width(70.0)
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

        ui.add_space(6.0);
        if ui.button("Refresh").clicked() {
            state.info("Refresh clicked. Please restart the app to re-scan devices.");
            changed = true;
        }
    });

    changed
}
