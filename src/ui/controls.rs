use crate::{app::AppState, config, devices, devices::filter_type::CrtFilter};
use eframe::egui;

pub fn layout_top_ui(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        if let Some(logo) = &state.logo_texture {
            ui.add(egui::Image::new(logo).max_height(160.0));
        }
        ui.heading("Michadame Viewer");
    });
    ui.separator();

    // Profile Management Settings
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label("Profile:");
            let pre_selected = state.active_profile.clone();
            let mut profile_keys: Vec<String> = state.profiles.keys().cloned().collect();
            profile_keys.sort();

            egui::ComboBox::from_id_source("profile_selector")
                .selected_text(&state.active_profile)
                .show_ui(ui, |ui| {
                    let mut combo_changed = false;
                    for key in profile_keys {
                        combo_changed |= ui
                            .selectable_value(&mut state.active_profile, key.clone(), key)
                            .changed();
                    }
                    if combo_changed && pre_selected != state.active_profile {
                        // Apply the new profile to the state
                        let profile_to_apply = state.profiles.get(&state.active_profile).cloned();
                        if let Some(profile) = profile_to_apply {
                            config::apply_profile_to_state(state, &profile);
                        }

                        changed = true;
                    }
                });

            if ui.button("Save Config").clicked() {
                let current_profile_data = config::build_profile_from_state(state);
                state
                    .profiles
                    .insert(state.active_profile.clone(), current_profile_data);
                config::save_config(state);
                state.status_message =
                    format!("Saved configuration to profile: {}", state.active_profile);
                changed = true;
            }

            ui.separator();
            ui.label("New Profile:");
            ui.add(egui::TextEdit::singleline(&mut state.new_profile_name).desired_width(100.0));
            if ui
                .add_enabled(
                    !state.new_profile_name.trim().is_empty()
                        && !state.profiles.contains_key(state.new_profile_name.trim()),
                    egui::Button::new("Add"),
                )
                .clicked()
            {
                let new_profile_name = state.new_profile_name.trim().to_string();
                let current_profile_data = config::build_profile_from_state(state);
                state
                    .profiles
                    .insert(new_profile_name.clone(), current_profile_data);
                state.active_profile = new_profile_name;
                state.new_profile_name.clear();

                config::save_config(state); // Save immediately on profile creation
                changed = true;
                state.status_message =
                    format!("Created and switched to profile: {}", state.active_profile);
            }

            if ui
                .add_enabled(
                    state.profiles.len() > 1,
                    egui::Button::new("Delete").fill(egui::Color32::from_rgb(180, 50, 50)),
                )
                .clicked()
            {
                state.profiles.remove(&state.active_profile);
                // Switch back to Default or the first available if Default somehow missing
                if state.profiles.contains_key("Default") {
                    state.active_profile = "Default".to_string();
                } else {
                    state.active_profile = state.profiles.keys().next().unwrap().clone();
                }
                let profile_to_apply = state.profiles.get(&state.active_profile).cloned();
                if let Some(profile) = profile_to_apply {
                    config::apply_profile_to_state(state, &profile);
                }

                config::save_config(state); // Save immediately on profile deletion
                changed = true;
                state.status_message =
                    format!("Deleted profile. Switched to: {}", state.active_profile);
            }
        });
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
                    combo_changed |= ui
                        .selectable_value(
                            &mut state.selected_video_device,
                            device.clone(),
                            device.as_str(),
                        )
                        .changed();
                }
                if combo_changed && !state.selected_video_device.is_empty() {
                    state.supported_formats.clear();
                    state.selected_format_index = 0;
                    state.selected_resolution = (0, 0);

                    match devices::video::find_video_formats(&state.selected_video_device) {
                        Ok(formats) => {
                            state.status_message = format!(
                                "Found {} formats for {}.",
                                formats.len(),
                                state.selected_video_device
                            );
                            state.supported_formats = formats;
                            if let Some(res) = state
                                .supported_formats
                                .first()
                                .and_then(|f| f.resolutions.first())
                            {
                                state.selected_resolution = (res.width, res.height);
                                state.selected_framerate =
                                    res.framerates.first().cloned().unwrap_or(0);
                            }
                            // After loading formats, try to apply the saved config for them.
                            if let Ok(cfg) =
                                confy::load::<config::MichadameConfig>("michadame", None)
                            {
                                crate::video::types::apply_saved_format_config(state, &cfg);
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
            let selected_format_description = state.supported_formats[state.selected_format_index]
                .description
                .clone();
            let resolutions = state.supported_formats[state.selected_format_index]
                .resolutions
                .clone();

            ui.label("Format:");
            egui::ComboBox::from_id_source("format_selector")
                .selected_text(selected_format_description)
                .show_ui(ui, |ui| {
                    for (i, format) in state.supported_formats.iter().enumerate() {
                        if ui
                            .selectable_value(
                                &mut state.selected_format_index,
                                i,
                                &format.description,
                            )
                            .changed()
                        {
                            if let Some(res) = state.supported_formats[i].resolutions.first() {
                                state.selected_resolution = (res.width, res.height);
                                state.selected_framerate =
                                    res.framerates.first().cloned().unwrap_or(0);
                            }

                            changed = true;
                        }
                    }
                });

            ui.label("Resolution:");
            egui::ComboBox::from_id_source("resolution_selector")
                .selected_text(format!(
                    "{}x{}",
                    state.selected_resolution.0, state.selected_resolution.1
                ))
                .show_ui(ui, |ui| {
                    for res in &resolutions {
                        if ui
                            .selectable_value(
                                &mut state.selected_resolution,
                                (res.width, res.height),
                                format!("{}x{}", res.width, res.height),
                            )
                            .changed()
                        {
                            state.selected_framerate = res.framerates.first().cloned().unwrap_or(0);

                            changed = true;
                        }
                    }
                });

            if let Some(res_info) = resolutions.iter().find(|r| {
                r.width == state.selected_resolution.0 && r.height == state.selected_resolution.1
            }) {
                if !res_info.framerates.is_empty() {
                    ui.label("Framerate:");
                    egui::ComboBox::from_id_source("framerate_selector")
                        .selected_text(format!("{} fps", state.selected_framerate))
                        .show_ui(ui, |ui| {
                            for &fps in &res_info.framerates {
                                if ui
                                    .selectable_value(
                                        &mut state.selected_framerate,
                                        fps,
                                        format!("{} fps", fps),
                                    )
                                    .changed()
                                {
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
                state.status_message =
                    "Refresh clicked. Please restart the app to re-scan devices.".to_string();
                changed = true;
            }
        });

        let selected_source_desc = state
            .pulse_sources
            .iter()
            .find(|(_, name)| Some(name) == state.selected_pulse_source_name.as_ref())
            .map(|(desc, _)| desc.as_str())
            .unwrap_or("Select an Input");

        egui::ComboBox::from_label("Input (Source)")
            .selected_text(selected_source_desc)
            .show_ui(ui, |ui| {
                let mut combo_changed = false;
                for (desc, name) in &state.pulse_sources {
                    combo_changed |= ui
                        .selectable_value(
                            &mut state.selected_pulse_source_name,
                            Some(name.clone()),
                            desc,
                        )
                        .changed();
                }
                if combo_changed {
                    changed = true;
                }
            });

        let selected_sink_desc = state
            .pulse_sinks
            .iter()
            .find(|(_, name)| Some(name) == state.selected_pulse_sink_name.as_ref())
            .map(|(desc, _)| desc.as_str())
            .unwrap_or("Select an Output");

        egui::ComboBox::from_label("Output (Sink)")
            .selected_text(selected_sink_desc)
            .show_ui(ui, |ui| {
                let mut combo_changed = false;
                for (desc, name) in &state.pulse_sinks {
                    combo_changed |= ui
                        .selectable_value(
                            &mut state.selected_pulse_sink_name,
                            Some(name.clone()),
                            desc,
                        )
                        .changed();
                }
                if combo_changed {
                    changed = true;
                }
            });
    });
    ui.separator();

    ui.horizontal(|ui| {
        let is_running = state.video_thread.is_some();
        let start_button = ui.add_enabled(
            !is_running && state.selected_resolution.0 > 0,
            egui::Button::new("▶ Start Stream"),
        );
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

    let current_filter =
        CrtFilter::from_u8(state.crt_filter.load(std::sync::atomic::Ordering::Relaxed));

    ui.horizontal(|ui| {
        if ui
            .checkbox(
                &mut state.pixelate_filter_enabled,
                "Enable 480p Pixelate Filter (GPU)",
            )
            .on_hover_text("This is a GPU-based pre-filter that runs before other effects.")
            .changed()
        {
            changed = true;
        }
        if ui
            .checkbox(&mut state.use_magenta_background, "Magenta Background")
            .on_hover_text(
                "Toggles background color between magenta and black for letterboxing/pillarboxing.",
            )
            .changed()
        {
            changed = true;
        }
    });

    ui.horizontal(|ui| {
        if ui.checkbox(&mut state.median_filter_enabled, "Enable Horizontal 3x1 Median Filter").on_hover_text("A GPU-based pre-filter that reduces horizontal noise by taking the median of 3 horizontal pixels.").changed() {

            changed = true;
        }
    });

    ui.horizontal(|ui| {
        ui.label("Horizontal Stretch:");
        if ui
            .add(egui::Slider::new(&mut state.horizontal_stretch, 0.5..=1.5).step_by(0.001))
            .changed()
        {
            changed = true;
        }
    });

    ui.horizontal(|ui| {
        ui.label("Video Vibrance:");
        if ui
            .add(egui::Slider::new(&mut state.vibrance, 0.0..=2.0).step_by(0.01))
            .changed()
        {
            changed = true;
        }
    });

    if current_filter == CrtFilter::Lottes {
        ui.group(|ui| {
            ui.label("Lottes Filter Settings");
            ui.collapsing("Geometry", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Warp X:");
                    if ui
                        .add(egui::Slider::new(&mut state.crt_warp_x, 0.0..=0.125))
                        .changed()
                    {
                        changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Warp Y:");
                    if ui
                        .add(egui::Slider::new(&mut state.crt_warp_y, 0.0..=0.125))
                        .changed()
                    {
                        changed = true;
                    }
                });
            });
            ui.collapsing("Scanlines & Pixels", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Scanline Hardness:");
                    if ui
                        .add(egui::Slider::new(&mut state.crt_hard_scan, -20.0..=-1.0))
                        .changed()
                    {
                        changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Pixel Hardness:");
                    if ui
                        .add(egui::Slider::new(&mut state.crt_hard_pix, -20.0..=0.0))
                        .changed()
                    {
                        changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Filter Shape:");
                    if ui
                        .add(egui::Slider::new(&mut state.crt_shape, 0.0..=10.0))
                        .changed()
                    {
                        changed = true;
                    }
                });
            });
            ui.collapsing("Bloom", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Bloom Amount:");
                    if ui
                        .add(egui::Slider::new(&mut state.crt_bloom_amount, 0.0..=1.0))
                        .changed()
                    {
                        changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Bloom X Softness:");
                    if ui
                        .add(egui::Slider::new(
                            &mut state.crt_hard_bloom_pix,
                            -4.0..=-0.5,
                        ))
                        .changed()
                    {
                        changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Bloom Y Softness:");
                    if ui
                        .add(egui::Slider::new(
                            &mut state.crt_hard_bloom_scan,
                            -4.0..=-1.0,
                        ))
                        .changed()
                    {
                        changed = true;
                    }
                });
            });
            ui.collapsing("Mask & Color", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Shadow Mask Type:");
                    if ui
                        .add(egui::Slider::new(&mut state.crt_shadow_mask, 0.0..=4.0).step_by(1.0))
                        .changed()
                    {
                        changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Brightness:");
                    if ui
                        .add(egui::Slider::new(&mut state.crt_brightboost, 0.0..=2.0))
                        .changed()
                    {
                        changed = true;
                    }
                });
                if ui.button("Reset to Defaults").clicked() {
                    let defaults = crate::video::gpu_filter::ShaderParams::default();
                    state.crt_hard_scan = defaults.hard_scan;
                    state.crt_warp_x = defaults.warp_x;
                    state.crt_warp_y = defaults.warp_y;
                    state.crt_shadow_mask = defaults.shadow_mask;
                    state.crt_brightboost = defaults.brightboost;
                    state.crt_hard_bloom_pix = defaults.hard_bloom_pix;
                    state.crt_hard_bloom_scan = defaults.hard_bloom_scan;
                    state.crt_bloom_amount = defaults.bloom_amount;
                    state.crt_shape = defaults.shape;
                    state.crt_hard_pix = defaults.hard_pix;

                    changed = true;
                }
            });
        });
    }

    ui.separator();
    ui.label(&state.status_message);
    changed
}
