use crate::app::AppState;
use crate::ui::controls::{sub_heading, technical_group, technical_separator, two_columns};
use eframe::egui;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectedFilterTab {
    NoiseGate,
    Gain,
    VoiceChanger,
    Compressor,
}

pub fn draw_mic_tab(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;

    // Repaint while Audio & Mic tab has an active stream, download, or decaying meter
    let vc_active = state.mic.config.voice_changer.enabled && state.mic.active_stream.is_some();
    let mic_meters_active = (state.mic.config.enabled && state.mic.active_stream.is_some())
        || state.mic.vc_sync_in_progress
        || vc_active
        || state.hardware.live_mic_peak > 0.001
        || state.has_active_av_source()
        || state.hardware.live_audio_peak > 0.001;
    if mic_meters_active {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }

    // --- Unified Microphone Input, Volume, Live Monitor & Filters ---
    technical_group(ui, "Microphone & Voice Filters:", |ui| {
        let is_stream_active = state.mic.active_stream.is_some();

        // Row 1: Enable Checkbox + Mic Input Device Selector + Monitor Checkbox + Status Indicator
        ui.horizontal(|ui| {
            let mut mic_enabled = state.mic.config.enabled;
            if ui
                .checkbox(&mut mic_enabled, "Enable Microphone")
                .on_hover_text(
                    "Enables microphone audio input capture and recording into the replay buffer.",
                )
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

            ui.add_space(6.0);
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
                .width(230.0)
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

            ui.add_space(6.0);

            let mut monitor = state.mic.config.monitor;
            if ui
                .checkbox(&mut monitor, "Monitor")
                .on_hover_text("Live listen to the microphone")
                .changed()
            {
                state.mic.config.monitor = monitor;
                sync_mic_config_to_stream(state);
                crate::config::save_global_hardware_config(state);
                changed = true;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if is_stream_active {
                    ui.label(
                        egui::RichText::new("● Mic Active")
                            .color(egui::Color32::from_rgb(46, 204, 113))
                            .strong()
                            .monospace()
                            .size(11.0),
                    );
                } else if state.mic.config.enabled {
                    ui.label(
                        egui::RichText::new("○ Mic Connecting...")
                            .color(egui::Color32::from_rgb(241, 196, 15))
                            .monospace()
                            .size(11.0),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("○ Mic Disabled")
                            .color(egui::Color32::from_rgb(149, 165, 166))
                            .monospace()
                            .size(11.0),
                    );
                }
            });
        });

        ui.add_space(4.0);

        // Row 2: Volume Slider + Live Feedback dB Peak Meter side-by-side
        two_columns(ui, |col1, col2| {
            col1.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Volume:")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(170, 170, 170)),
                );
                let mut vol_percent = (state.mic.config.volume * 100.0).round();
                if ui
                    .add(
                        egui::Slider::new(&mut vol_percent, 0.0..=150.0)
                            .custom_formatter(|val, _| format!("{:.0}%", val)),
                    )
                    .changed()
                {
                    state.mic.config.volume = (vol_percent / 100.0).clamp(0.0, 2.0);
                    sync_mic_config_to_stream(state);
                    crate::config::save_global_hardware_config(state);
                    changed = true;
                }
            });

            let current_db = f32::from_bits(state.mic.current_level_db.load(Ordering::Relaxed));
            let peak_db = f32::from_bits(state.mic.peak_level_db.load(Ordering::Relaxed));
            let clipping = state.mic.is_clipping.load(Ordering::Relaxed) || current_db >= 0.0;
            draw_db_meter(col2, current_db, peak_db, clipping);
        });

        let show_voice_changer = state.steam_deck.is_none();
        technical_separator(ui);
        if show_voice_changer {
            sub_heading(
                ui,
                "Audio Filters (In Order: Gate -> Gain -> Voice Changer -> Compressor):",
            );
        } else {
            sub_heading(ui, "Audio Filters (In Order: Gate -> Gain -> Compressor):");
        }

        // Persistent selected filter tab in UI memory
        let filter_tab_id = ui.id().with("selected_mic_filter_tab");
        let mut selected_tab = ui.data_mut(|d| {
            d.get_temp::<SelectedFilterTab>(filter_tab_id)
                .unwrap_or(SelectedFilterTab::NoiseGate)
        });
        if !show_voice_changer && selected_tab == SelectedFilterTab::VoiceChanger {
            selected_tab = SelectedFilterTab::NoiseGate;
        }

        ui.horizontal(|ui| {
            // Left Column: Filter list with checkboxes matching OBS
            ui.vertical(|ui| {
                ui.set_width(170.0);

                // Filter 1: Noise Gate
                let mut gate_enabled = state.mic.config.noise_gate.enabled;
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut gate_enabled, "").changed() {
                        state.mic.config.noise_gate.enabled = gate_enabled;
                        sync_mic_config_to_stream(state);
                        crate::config::save_global_hardware_config(state);
                        changed = true;
                    }
                    let is_active = selected_tab == SelectedFilterTab::NoiseGate;
                    let text = egui::RichText::new("Noise Gate").monospace().size(11.0);
                    let btn = egui::Button::new(if is_active {
                        text.strong().color(egui::Color32::WHITE)
                    } else {
                        text.color(egui::Color32::from_rgb(170, 170, 170))
                    })
                    .fill(if is_active {
                        egui::Color32::from_rgb(33, 97, 159)
                    } else {
                        egui::Color32::from_rgb(34, 34, 34)
                    });
                    if ui.add(btn).clicked() {
                        selected_tab = SelectedFilterTab::NoiseGate;
                    }
                });

                ui.add_space(2.0);

                // Filter 2: Gain
                let mut gain_enabled = state.mic.config.gain.enabled;
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut gain_enabled, "").changed() {
                        state.mic.config.gain.enabled = gain_enabled;
                        sync_mic_config_to_stream(state);
                        crate::config::save_global_hardware_config(state);
                        changed = true;
                    }
                    let is_active = selected_tab == SelectedFilterTab::Gain;
                    let text = egui::RichText::new("Gain").monospace().size(11.0);
                    let btn = egui::Button::new(if is_active {
                        text.strong().color(egui::Color32::WHITE)
                    } else {
                        text.color(egui::Color32::from_rgb(170, 170, 170))
                    })
                    .fill(if is_active {
                        egui::Color32::from_rgb(33, 97, 159)
                    } else {
                        egui::Color32::from_rgb(34, 34, 34)
                    });
                    if ui.add(btn).clicked() {
                        selected_tab = SelectedFilterTab::Gain;
                    }
                });

                if show_voice_changer {
                    ui.add_space(2.0);

                    // Filter 3: Voice Changer (Desktop-only)
                    let mut vc_enabled = state.mic.config.voice_changer.enabled;
                    ui.horizontal(|ui| {
                        if ui
                            .checkbox(&mut vc_enabled, "")
                            .on_hover_text(
                                "Enable real-time neural RVC v2 voice conversion in the mic chain",
                            )
                            .changed()
                        {
                            state.mic.config.voice_changer.enabled = vc_enabled;
                            sync_mic_config_to_stream(state);
                            crate::config::save_global_hardware_config(state);
                            changed = true;
                        }
                        let is_active = selected_tab == SelectedFilterTab::VoiceChanger;
                        let text = egui::RichText::new("Voice Changer").monospace().size(11.0);
                        let btn = egui::Button::new(if is_active {
                            text.strong().color(egui::Color32::WHITE)
                        } else {
                            text.color(egui::Color32::from_rgb(170, 170, 170))
                        })
                        .fill(if is_active {
                            egui::Color32::from_rgb(33, 97, 159)
                        } else {
                            egui::Color32::from_rgb(34, 34, 34)
                        });
                        if ui.add(btn).clicked() {
                            selected_tab = SelectedFilterTab::VoiceChanger;
                        }
                    });
                }

                ui.add_space(2.0);

                // Filter 4: Compressor
                let mut comp_enabled = state.mic.config.compressor.enabled;
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut comp_enabled, "").changed() {
                        state.mic.config.compressor.enabled = comp_enabled;
                        sync_mic_config_to_stream(state);
                        crate::config::save_global_hardware_config(state);
                        changed = true;
                    }
                    let is_active = selected_tab == SelectedFilterTab::Compressor;
                    let text = egui::RichText::new("Compressor").monospace().size(11.0);
                    let btn = egui::Button::new(if is_active {
                        text.strong().color(egui::Color32::WHITE)
                    } else {
                        text.color(egui::Color32::from_rgb(170, 170, 170))
                    })
                    .fill(if is_active {
                        egui::Color32::from_rgb(33, 97, 159)
                    } else {
                        egui::Color32::from_rgb(34, 34, 34)
                    });
                    if ui.add(btn).clicked() {
                        selected_tab = SelectedFilterTab::Compressor;
                    }
                });
            });

            ui.separator();

            // Right Column: Filter settings controls
            ui.vertical(|ui| {
                let mut slider_changed = false;
                match selected_tab {
                    SelectedFilterTab::NoiseGate => {
                        let gate = &mut state.mic.config.noise_gate;
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Close Threshold:")
                                    .monospace()
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(170, 170, 170)),
                            );
                            slider_changed |= ui
                                .add(
                                    egui::Slider::new(&mut gate.close_threshold_db, -96.0..=0.0)
                                        .suffix(" dB")
                                        .custom_formatter(|v, _| format!("{:.2} dB", v)),
                                )
                                .changed();
                        });

                        ui.add_space(2.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Open Threshold: ")
                                    .monospace()
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(170, 170, 170)),
                            );
                            slider_changed |= ui
                                .add(
                                    egui::Slider::new(&mut gate.open_threshold_db, -96.0..=0.0)
                                        .suffix(" dB")
                                        .custom_formatter(|v, _| format!("{:.2} dB", v)),
                                )
                                .changed();
                        });

                        ui.add_space(2.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Attack Time:    ")
                                    .monospace()
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(170, 170, 170)),
                            );
                            slider_changed |= ui
                                .add(
                                    egui::Slider::new(&mut gate.attack_time_ms, 0.0..=500.0)
                                        .suffix(" ms")
                                        .custom_formatter(|v, _| format!("{:.0} ms", v)),
                                )
                                .changed();
                        });

                        ui.add_space(2.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Hold Time:      ")
                                    .monospace()
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(170, 170, 170)),
                            );
                            slider_changed |= ui
                                .add(
                                    egui::Slider::new(&mut gate.hold_time_ms, 0.0..=2000.0)
                                        .suffix(" ms")
                                        .custom_formatter(|v, _| format!("{:.0} ms", v)),
                                )
                                .changed();
                        });

                        ui.add_space(2.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Release Time:   ")
                                    .monospace()
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(170, 170, 170)),
                            );
                            slider_changed |= ui
                                .add(
                                    egui::Slider::new(&mut gate.release_time_ms, 0.0..=1000.0)
                                        .suffix(" ms")
                                        .custom_formatter(|v, _| format!("{:.0} ms", v)),
                                )
                                .changed();
                        });
                    }
                    SelectedFilterTab::Gain => {
                        let gain = &mut state.mic.config.gain;
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Gain:")
                                    .monospace()
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(170, 170, 170)),
                            );
                            slider_changed |= ui
                                .add(
                                    egui::Slider::new(&mut gain.gain_db, -30.0..=30.0)
                                        .suffix(" dB")
                                        .custom_formatter(|v, _| format!("{:.2} dB", v)),
                                )
                                .changed();
                        });
                    }
                    SelectedFilterTab::VoiceChanger => {
                        draw_voice_changer_panel(ui, state, &mut slider_changed);
                    }
                    SelectedFilterTab::Compressor => {
                        let comp = &mut state.mic.config.compressor;
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Ratio:      ")
                                    .monospace()
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(170, 170, 170)),
                            );
                            slider_changed |= ui
                                .add(
                                    egui::Slider::new(&mut comp.ratio, 1.0..=32.0)
                                        .custom_formatter(|v, _| format!("{:.2}:1", v)),
                                )
                                .changed();
                        });

                        ui.add_space(2.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Threshold:  ")
                                    .monospace()
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(170, 170, 170)),
                            );
                            slider_changed |= ui
                                .add(
                                    egui::Slider::new(&mut comp.threshold_db, -60.0..=0.0)
                                        .suffix(" dB")
                                        .custom_formatter(|v, _| format!("{:.2} dB", v)),
                                )
                                .changed();
                        });

                        ui.add_space(2.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Attack:     ")
                                    .monospace()
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(170, 170, 170)),
                            );
                            slider_changed |= ui
                                .add(
                                    egui::Slider::new(&mut comp.attack_time_ms, 1.0..=500.0)
                                        .suffix(" ms")
                                        .custom_formatter(|v, _| format!("{:.0} ms", v)),
                                )
                                .changed();
                        });

                        ui.add_space(2.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Release:    ")
                                    .monospace()
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(170, 170, 170)),
                            );
                            slider_changed |= ui
                                .add(
                                    egui::Slider::new(&mut comp.release_time_ms, 1.0..=1000.0)
                                        .suffix(" ms")
                                        .custom_formatter(|v, _| format!("{:.0} ms", v)),
                                )
                                .changed();
                        });

                        ui.add_space(2.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Output Gain:")
                                    .monospace()
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(170, 170, 170)),
                            );
                            slider_changed |= ui
                                .add(
                                    egui::Slider::new(&mut comp.output_gain_db, -32.0..=32.0)
                                        .suffix(" dB")
                                        .custom_formatter(|v, _| format!("{:.2} dB", v)),
                                )
                                .changed();
                        });
                    }
                }
                if slider_changed {
                    sync_mic_config_to_stream(state);
                    crate::config::save_global_hardware_config(state);
                    changed = true;
                }
            });
        });

        ui.data_mut(|d| d.insert_temp(filter_tab_id, selected_tab));
    });

    changed
}

fn sync_mic_config_to_stream(state: &AppState) {
    if let Some(stream) = &state.mic.active_stream {
        if let Ok(mut w) = stream.config.write() {
            *w = state.mic.config.clone();
        }
    }
}

fn draw_voice_changer_panel(ui: &mut egui::Ui, state: &mut AppState, slider_changed: &mut bool) {
    use crate::config::{
        VcExecutionProvider, VoiceChangerPreset, VoiceModelId, VoiceModelKind, VALID_VC_CHUNK_MS,
        VOICE_CHANGER_DEFAULT_PRESET,
    };
    use crate::devices::voice_changer::model_sync::{
        delete_builtin_voice_model, query_vc_install_status, resolve_voice_changer_directory,
        VcSyncTarget,
    };
    use crate::devices::voice_changer::VoiceChangerStatus;

    state.mic.config.voice_changer.ensure_builtin_presets();

    let vc_dir = resolve_voice_changer_directory();
    let install_status = query_vc_install_status(&vc_dir);

    // --- 1. Global User Preset Bar (independent of capture source profiles) ---
    let mut toast_info: Option<String> = None;
    let mut toast_err: Option<String> = None;

    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new("Voice Preset:")
                .monospace()
                .size(11.0)
                .strong()
                .color(egui::Color32::from_rgb(224, 224, 224)),
        );

        let vc = &state.mic.config.voice_changer;
        let is_modified = vc
            .presets
            .get(&vc.active_preset)
            .is_none_or(|saved| *saved != vc.current);
        let combo_label = if is_modified {
            format!("{} *", vc.active_preset)
        } else {
            vc.active_preset.clone()
        };

        let mut preset_names: Vec<String> = VoiceChangerPreset::builtin_preset_list()
            .into_iter()
            .filter_map(|(name, _)| vc.presets.contains_key(name).then(|| name.to_string()))
            .collect();
        for key in vc.presets.keys() {
            if !VoiceChangerPreset::is_builtin_preset(key) {
                preset_names.push(key.clone());
            }
        }

        let mut selected_preset = vc.active_preset.clone();
        egui::ComboBox::from_id_source("vc_global_preset_selector")
            .width(215.0)
            .selected_text(combo_label)
            .show_ui(ui, |ui| {
                for name in &preset_names {
                    if ui
                        .selectable_value(&mut selected_preset, name.clone(), name)
                        .clicked()
                        && state.mic.config.voice_changer.apply_preset(name)
                    {
                        toast_info = Some(format!("Loaded global voice preset '{}'", name));
                        *slider_changed = true;
                    }
                }
            });

        let name_resp = ui.add(
            egui::TextEdit::singleline(&mut state.mic.new_vc_preset_name)
                .hint_text("New preset name (optional)")
                .desired_width(145.0),
        );
        let trimmed_new = state.mic.new_vc_preset_name.trim().to_string();
        let enter_pressed = !trimmed_new.is_empty()
            && name_resp.lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter));

        let save_hover = if trimmed_new.is_empty() {
            "Overwrite the selected global voice preset with current slider & model settings"
        } else {
            "Save current voice model and sliders as a new global preset"
        };
        if ui.button("Save Preset").on_hover_text(save_hover).clicked() || enter_pressed {
            let target_name = if trimmed_new.is_empty() {
                state.mic.config.voice_changer.active_preset.clone()
            } else {
                trimmed_new.clone()
            };
            let clean = state.mic.config.voice_changer.current.sanitized();
            state.mic.config.voice_changer.current = clean.clone();
            state
                .mic
                .config
                .voice_changer
                .presets
                .insert(target_name.clone(), clean);
            state.mic.config.voice_changer.active_preset = target_name.clone();
            if !trimmed_new.is_empty() {
                state.mic.new_vc_preset_name.clear();
                toast_info = Some(format!("Created global voice preset '{}'", target_name));
            } else {
                toast_info = Some(format!("Saved global voice preset '{}'", target_name));
            }
            *slider_changed = true;
        }

        let active_name = state.mic.config.voice_changer.active_preset.clone();
        let is_builtin = VoiceChangerPreset::is_builtin_preset(&active_name);
        if ui
            .button("Reset")
            .on_hover_text(if is_builtin {
                "Reset this built-in voice preset to factory defaults"
            } else {
                "Revert unsaved slider edits back to the saved preset"
            })
            .clicked()
        {
            if let Some(factory) = VoiceChangerPreset::builtin_preset(&active_name) {
                state
                    .mic
                    .config
                    .voice_changer
                    .presets
                    .insert(active_name.clone(), factory.clone());
                state.mic.config.voice_changer.current = factory;
                toast_info = Some(format!(
                    "Reset voice preset '{}' to factory defaults",
                    active_name
                ));
                *slider_changed = true;
            } else if let Some(saved) = state
                .mic
                .config
                .voice_changer
                .presets
                .get(&active_name)
                .cloned()
            {
                state.mic.config.voice_changer.current = saved;
                *slider_changed = true;
            }
        }

        if ui
            .add_enabled(!is_builtin, egui::Button::new("Delete"))
            .on_hover_text(if is_builtin {
                "Built-in presets cannot be deleted (use Save Preset to override or Reset to restore)"
            } else {
                "Delete this custom global voice preset"
            })
            .clicked()
        {
            state.mic.config.voice_changer.presets.remove(&active_name);
            state.mic.config.voice_changer.ensure_builtin_presets();
            state
                .mic
                .config
                .voice_changer
                .apply_preset(VOICE_CHANGER_DEFAULT_PRESET);
            toast_info = Some(format!("Deleted voice preset '{}'", active_name));
            *slider_changed = true;
        }
    });

    // --- 2. Download Progress / Error Banner ---
    if state.mic.vc_sync_in_progress {
        ui.add_space(4.0);
        let (msg, prog) = state
            .mic
            .vc_sync_progress
            .clone()
            .unwrap_or_else(|| ("Downloading...".to_string(), 0.0));
        ui.add(
            egui::ProgressBar::new(prog.clamp(0.0, 1.0))
                .text(msg)
                .animate(true),
        );
    } else if let Some(err) = state.mic.vc_sync_error.clone() {
        ui.add_space(3.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(format!("⚠ {err}"))
                    .monospace()
                    .size(10.5)
                    .color(egui::Color32::from_rgb(231, 76, 60)),
            );
            if ui.small_button("Dismiss").clicked() {
                state.mic.vc_sync_error = None;
            }
        });
    }

    ui.add_space(4.0);

    // --- 3. Voice Model Selection + 1-Click Download + GPU Execution Provider ---
    let mut start_download_target: Option<VcSyncTarget> = None;
    let mut delete_model_target: Option<VoiceModelId> = None;

    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new("Source:      ")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(170, 170, 170)),
        );
        let kind = &mut state.mic.config.voice_changer.current.model_kind;
        if ui
            .selectable_value(kind, VoiceModelKind::BuiltIn, "Built-In Catalog")
            .clicked()
            || ui
                .selectable_value(kind, VoiceModelKind::CustomOnnx, "Custom (.onnx)")
                .clicked()
        {
            *slider_changed = true;
        }

        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Device:")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(170, 170, 170)),
        );
        let ep = &mut state.mic.config.voice_changer.execution_provider;
        egui::ComboBox::from_id_source("vc_ep_selector")
            .width(190.0)
            .selected_text(ep.label())
            .show_ui(ui, |ui| {
                for cand in VcExecutionProvider::ALL {
                    if ui.selectable_value(ep, cand, cand.label()).changed() {
                        *slider_changed = true;
                    }
                }
            });
    });

    ui.add_space(3.0);

    match state.mic.config.voice_changer.current.model_kind {
        VoiceModelKind::BuiltIn => {
            let current_id = state.mic.config.voice_changer.current.builtin_model;
            let voice_installed = install_status.is_voice_installed(current_id);
            let engine_ready = install_status.is_engine_ready();

            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new("Voice Model: ")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(170, 170, 170)),
                );

                let selected_label = if voice_installed {
                    format!("✓ {}", current_id.label())
                } else {
                    format!("⬇ {}", current_id.label())
                };

                egui::ComboBox::from_id_source("vc_builtin_voice_selector")
                    .width(265.0)
                    .selected_text(selected_label)
                    .show_ui(ui, |ui| {
                        for cand in VoiceModelId::ALL {
                            let tag = if install_status.is_voice_installed(cand) {
                                "✓"
                            } else {
                                "⬇"
                            };
                            let row_text = format!("{tag} {}", cand.label());
                            let resp = ui
                                .selectable_value(
                                    &mut state.mic.config.voice_changer.current.builtin_model,
                                    cand,
                                    row_text,
                                )
                                .on_hover_text(cand.credit_notice());
                            if resp.changed() {
                                *slider_changed = true;
                            }
                        }
                    });

                let updated_id = state.mic.config.voice_changer.current.builtin_model;
                let updated_installed = install_status.is_voice_installed(updated_id);

                if !updated_installed || !engine_ready {
                    let dl_label = if !engine_ready && !updated_installed {
                        "⬇ Download Engine + Voice"
                    } else if !engine_ready {
                        "⬇ Download Base Engine"
                    } else {
                        "⬇ Download Voice (~105 MB)"
                    };
                    let btn = egui::Button::new(
                        egui::RichText::new(dl_label)
                            .monospace()
                            .size(11.0)
                            .strong()
                            .color(egui::Color32::WHITE),
                    )
                    .fill(egui::Color32::from_rgb(39, 134, 84));
                    if ui
                        .add_enabled(!state.mic.vc_sync_in_progress, btn)
                        .on_hover_text(
                            "Downloads verified ONNX model files into ~/.config/michadame/voice_changer/",
                        )
                        .clicked()
                    {
                        start_download_target = if !updated_installed {
                            Some(VcSyncTarget::VoiceModel(updated_id))
                        } else {
                            Some(VcSyncTarget::BaseEngine)
                        };
                    }
                } else {
                    ui.label(
                        egui::RichText::new("● Ready")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(46, 204, 113)),
                    )
                    .on_hover_text(updated_id.credit_notice());

                    if ui
                        .add_enabled(
                            !state.mic.vc_sync_in_progress,
                            egui::Button::new(egui::RichText::new("Remove").size(10.5)),
                        )
                        .on_hover_text("Delete this downloaded voice file from local disk")
                        .clicked()
                    {
                        delete_model_target = Some(updated_id);
                    }
                }
            });
        }
        VoiceModelKind::CustomOnnx => {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new("ONNX File:   ")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(170, 170, 170)),
                );

                let cur_path = state
                    .mic
                    .config
                    .voice_changer
                    .current
                    .custom_model_path
                    .trim()
                    .to_string();
                let display_name = if cur_path.is_empty() {
                    "No .onnx file selected".to_string()
                } else {
                    std::path::Path::new(&cur_path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(&cur_path)
                        .to_string()
                };

                ui.label(
                    egui::RichText::new(display_name)
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(210, 210, 210)),
                )
                .on_hover_text(if cur_path.is_empty() {
                    "Choose any RVC v2 .onnx generator model (40k or 48k, FP32 or FP16)"
                } else {
                    &cur_path
                });

                if ui.button("Browse .onnx...").clicked() {
                    if let Some(picked) = rfd::FileDialog::new()
                        .add_filter("RVC ONNX Voice Model", &["onnx"])
                        .pick_file()
                    {
                        state.mic.config.voice_changer.current.custom_model_path =
                            picked.to_string_lossy().to_string();
                        *slider_changed = true;
                    }
                }

                if !install_status.is_engine_ready() {
                    let btn = egui::Button::new(
                        egui::RichText::new("⬇ Download Base Engine")
                            .monospace()
                            .size(11.0)
                            .strong()
                            .color(egui::Color32::WHITE),
                    )
                    .fill(egui::Color32::from_rgb(39, 134, 84));
                    if ui
                        .add_enabled(!state.mic.vc_sync_in_progress, btn)
                        .on_hover_text(
                            "Downloads ONNX Runtime + ContentVec + RMVPE required to run custom .onnx models",
                        )
                        .clicked()
                    {
                        start_download_target = Some(VcSyncTarget::BaseEngine);
                    }
                }
            });
        }
    }

    if let Some(target) = start_download_target {
        state.mic.start_vc_download(target, Some(ui.ctx().clone()));
    }
    if let Some(del_id) = delete_model_target {
        match delete_builtin_voice_model(&vc_dir, del_id) {
            Ok(()) => {
                state.mic.vc_telemetry.request_reload();
                toast_info = Some(format!(
                    "Removed downloaded model '{}'",
                    del_id.short_label()
                ));
            }
            Err(e) => toast_err = Some(e),
        }
    }

    ui.add_space(4.0);
    technical_separator(ui);

    // --- 4. Voice Customization Sliders ---
    let preset = &mut state.mic.config.voice_changer.current;

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Pitch Shift: ")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(170, 170, 170)),
        )
        .on_hover_text(
            "Shifts fundamental vocal pitch in semitones (+12 st = Male -> Girl octave)",
        );
        *slider_changed |= ui
            .add(
                egui::Slider::new(&mut preset.pitch_shift_semitones, -24.0..=24.0)
                    .step_by(0.1)
                    .custom_formatter(|v, _| format!("{:+.1} st", v)),
            )
            .changed();
        for (lbl, st) in [("-12", -12.0f32), ("0", 0.0), ("+6", 6.0), ("+12", 12.0)] {
            if ui.small_button(lbl).clicked() {
                preset.pitch_shift_semitones = st;
                *slider_changed = true;
            }
        }
    });

    ui.add_space(2.0);

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Formant:     ")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(170, 170, 170)),
        )
        .on_hover_text(
            "Independent vocal-tract resonance shift: positive = brighter/cuter, negative = warmer/deeper",
        );
        *slider_changed |= ui
            .add(
                egui::Slider::new(&mut preset.formant_shift_semitones, -4.0..=4.0)
                    .step_by(0.1)
                    .custom_formatter(|v, _| format!("{:+.1} st", v)),
            )
            .changed();
    });

    ui.add_space(2.0);

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Protect:     ")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(170, 170, 170)),
        )
        .on_hover_text(
            "Protects breathy consonants (s, sh, t, k, h) from artificial pitch buzz (0.33 recommended)",
        );
        *slider_changed |= ui
            .add(
                egui::Slider::new(&mut preset.protect, 0.0..=0.5)
                    .step_by(0.01)
                    .custom_formatter(|v, _| format!("{:.2}", v)),
            )
            .changed();
    });

    ui.add_space(2.0);

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Chunk Size:  ")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(170, 170, 170)),
        )
        .on_hover_text("Smaller chunks reduce monitor latency; larger chunks reduce GPU load");
        for ms in VALID_VC_CHUNK_MS {
            if ui
                .selectable_value(&mut preset.chunk_ms, ms, format!("{ms} ms"))
                .changed()
            {
                *slider_changed = true;
            }
        }

        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Context:")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(170, 170, 170)),
        )
        .on_hover_text(
            "Past phonetic context window (adds zero algorithmic latency; increases GPU work)",
        );
        *slider_changed |= ui
            .add(
                egui::Slider::new(&mut preset.extra_context_ms, 160..=800)
                    .step_by(20.0)
                    .custom_formatter(|v, _| format!("{:.0} ms", v)),
            )
            .changed();
    });

    ui.add_space(2.0);

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Silence Gate:")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(170, 170, 170)),
        )
        .on_hover_text("Skips neural synthesis when input falls below this level to prevent room-floor whispers");
        *slider_changed |= ui
            .add(
                egui::Slider::new(&mut preset.silence_threshold_db, -70.0..=-20.0)
                    .custom_formatter(|v, _| format!("{:.1} dB", v)),
            )
            .changed();

        ui.add_space(6.0);
        *slider_changed |= ui
            .checkbox(&mut preset.pre_denoise, "Pre-Clean (RNNoise)")
            .on_hover_text("Runs neural mic denoising before RVC feature extraction for cleaner vowels")
            .changed();
    });

    ui.add_space(2.0);

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Output Gain: ")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(170, 170, 170)),
        );
        *slider_changed |= ui
            .add(
                egui::Slider::new(&mut preset.output_gain_db, -18.0..=18.0)
                    .custom_formatter(|v, _| format!("{:+.1} dB", v)),
            )
            .changed();

        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Mix:")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(170, 170, 170)),
        );
        let mut mix_pct = (preset.dry_wet * 100.0).round();
        if ui
            .add(
                egui::Slider::new(&mut mix_pct, 0.0..=100.0)
                    .custom_formatter(|v, _| format!("{:.0}%", v)),
            )
            .changed()
        {
            preset.dry_wet = (mix_pct / 100.0).clamp(0.0, 1.0);
            *slider_changed = true;
        }
    });

    // --- 5. Live Telemetry Footer ---
    ui.add_space(4.0);
    let telem = &state.mic.vc_telemetry;
    let status = telem.status();
    let (status_txt, status_col) = if !state.mic.config.voice_changer.enabled {
        ("○ Filter Disabled", egui::Color32::from_rgb(140, 140, 140))
    } else if !state.mic.config.enabled {
        (
            "○ Enable Mic to Start",
            egui::Color32::from_rgb(160, 160, 160),
        )
    } else {
        match status {
            VoiceChangerStatus::Active => ("● Active", egui::Color32::from_rgb(46, 204, 113)),
            VoiceChangerStatus::Loading => {
                ("◌ Loading ONNX...", egui::Color32::from_rgb(241, 196, 15))
            }
            VoiceChangerStatus::MissingModels => (
                "○ Download Voice to Activate (Passing Dry Mic)",
                egui::Color32::from_rgb(243, 156, 18),
            ),
            VoiceChangerStatus::Error => ("⚠ Engine Error", egui::Color32::from_rgb(231, 76, 60)),
            VoiceChangerStatus::Disabled => ("○ Standby", egui::Color32::from_rgb(140, 140, 140)),
        }
    };

    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(status_txt)
                .monospace()
                .size(10.5)
                .strong()
                .color(status_col),
        );
        if status == VoiceChangerStatus::Active && state.mic.config.voice_changer.enabled {
            let ep_lbl = telem.active_ep_label();
            let lat_ms = telem.latency_ms();
            let inf_ms = telem.inference_ms();
            ui.label(
                egui::RichText::new(format!(
                    "| {ep_lbl} | Latency: {:.0} ms | Infer: {:.1} ms",
                    lat_ms, inf_ms
                ))
                .monospace()
                .size(10.5)
                .color(egui::Color32::from_rgb(175, 175, 175)),
            );
            if telem.is_xrun() {
                ui.label(
                    egui::RichText::new("⚠ XRUN (Increase Chunk Size)")
                        .monospace()
                        .size(10.5)
                        .color(egui::Color32::from_rgb(231, 76, 60)),
                );
            }
        } else if status == VoiceChangerStatus::Error {
            if let Some(err) = telem.last_error() {
                ui.label(
                    egui::RichText::new(err)
                        .monospace()
                        .size(10.0)
                        .color(egui::Color32::from_rgb(231, 76, 60)),
                );
            }
        }
    });

    if let Some(msg) = toast_info {
        state.info(msg);
    }
    if let Some(err) = toast_err {
        state.error(err);
    }
}

/// Custom colored dB Volume Meter with green/orange/red segments, peak hold indicator, and clipping LED
fn draw_db_meter(ui: &mut egui::Ui, current_db: f32, peak_db: f32, clipping: bool) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Level:")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(170, 170, 170)),
        );

        let meter_width = 240.0;
        let meter_height = 14.0;
        let (rect, _response) =
            ui.allocate_exact_size(egui::vec2(meter_width, meter_height), egui::Sense::hover());

        if ui.is_rect_visible(rect) {
            let painter = ui.painter();

            // Background trough
            painter.rect_filled(
                rect,
                egui::Rounding::ZERO,
                egui::Color32::from_rgb(17, 17, 17),
            );
            painter.rect_stroke(
                rect,
                egui::Rounding::ZERO,
                egui::Stroke::new(1.0, egui::Color32::from_rgb(53, 53, 53)),
            );

            // Normalized level 0.0 .. 1.0 (corresponds to -60 dB .. 0 dB)
            let norm_level = ((current_db + 60.0) / 60.0).clamp(0.0, 1.0);
            let norm_peak = ((peak_db + 60.0) / 60.0).clamp(0.0, 1.0);

            // Segments:
            // Green: -60 dB to -20 dB => 0.0 to 40.0/60.0 = 0.667
            // Orange: -20 dB to -9 dB => 0.667 to 51.0/60.0 = 0.85
            // Red: -9 dB to 0 dB => 0.85 to 1.0
            let p_green_end = 40.0 / 60.0;
            let p_orange_end = 51.0 / 60.0;

            let fill_width = norm_level * rect.width();

            if fill_width > 0.0 {
                // Green part
                let green_w = fill_width.min(p_green_end * rect.width());
                let r_green =
                    egui::Rect::from_min_size(rect.min, egui::vec2(green_w, rect.height()));
                painter.rect_filled(
                    r_green,
                    egui::Rounding::ZERO,
                    egui::Color32::from_rgb(46, 204, 113),
                );

                // Orange part
                if fill_width > p_green_end * rect.width() {
                    let orange_start = rect.min.x + p_green_end * rect.width();
                    let orange_w = (fill_width - p_green_end * rect.width())
                        .min((p_orange_end - p_green_end) * rect.width());
                    let r_orange = egui::Rect::from_min_size(
                        egui::pos2(orange_start, rect.min.y),
                        egui::vec2(orange_w, rect.height()),
                    );
                    painter.rect_filled(
                        r_orange,
                        egui::Rounding::ZERO,
                        egui::Color32::from_rgb(243, 156, 18),
                    );
                }

                // Red part
                if fill_width > p_orange_end * rect.width() {
                    let red_start = rect.min.x + p_orange_end * rect.width();
                    let red_w = fill_width - p_orange_end * rect.width();
                    let r_red = egui::Rect::from_min_size(
                        egui::pos2(red_start, rect.min.y),
                        egui::vec2(red_w, rect.height()),
                    );
                    painter.rect_filled(
                        r_red,
                        egui::Rounding::ZERO,
                        egui::Color32::from_rgb(231, 76, 60),
                    );
                }
            }

            // Peak hold tick mark
            if norm_peak > 0.01 {
                let peak_x = rect.min.x + norm_peak * rect.width();
                let peak_line = egui::Rect::from_min_size(
                    egui::pos2((peak_x - 1.0).max(rect.min.x), rect.min.y),
                    egui::vec2(2.0, rect.height()),
                );
                let peak_color = if norm_peak >= p_orange_end {
                    egui::Color32::from_rgb(255, 80, 80)
                } else if norm_peak >= p_green_end {
                    egui::Color32::from_rgb(255, 200, 80)
                } else {
                    egui::Color32::WHITE
                };
                painter.rect_filled(peak_line, egui::Rounding::ZERO, peak_color);
            }

            // Subdivision lines at -20 dB and -9 dB
            let line_20_x = rect.min.x + p_green_end * rect.width();
            painter.line_segment(
                [
                    egui::pos2(line_20_x, rect.min.y),
                    egui::pos2(line_20_x, rect.max.y),
                ],
                egui::Stroke::new(1.0, egui::Color32::from_rgb(30, 30, 30)),
            );
            let line_9_x = rect.min.x + p_orange_end * rect.width();
            painter.line_segment(
                [
                    egui::pos2(line_9_x, rect.min.y),
                    egui::pos2(line_9_x, rect.max.y),
                ],
                egui::Stroke::new(1.0, egui::Color32::from_rgb(30, 30, 30)),
            );
        }

        // Clipping indicator
        let clip_color = if clipping {
            egui::Color32::from_rgb(255, 50, 50)
        } else {
            egui::Color32::from_rgb(60, 20, 20)
        };
        let clip_text = if clipping { "CLIP!" } else { "CLIP" };
        ui.label(
            egui::RichText::new(clip_text)
                .monospace()
                .size(10.0)
                .color(clip_color)
                .strong(),
        );

        // Numeric Readout
        let level_display = if current_db < -70.0 {
            "-inf dB".to_string()
        } else {
            format!("{:.1} dB", current_db)
        };
        let peak_display = if peak_db < -70.0 {
            "-inf".to_string()
        } else {
            format!("{:.1}", peak_db)
        };

        ui.label(
            egui::RichText::new(format!("{} (Peak: {})", level_display, peak_display))
                .monospace()
                .size(10.0)
                .color(egui::Color32::from_rgb(180, 180, 180)),
        );
    });
}
