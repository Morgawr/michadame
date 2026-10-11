use crate::app::AppState;
use crate::ui::controls::technical_group;
use eframe::egui;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectedFilterTab {
    NoiseGate,
    Gain,
    Compressor,
}

pub fn draw_mic_tab(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;

    // Repaint while Mic tab is active to smoothly animate the live dB meter
    ui.ctx().request_repaint_after(Duration::from_millis(16));

    // --- Master Volume, Level Meter & Monitor Mode ---
    technical_group(ui, "Microphone Output & Live Monitor:", |ui| {
        let is_stream_active = state.mic.active_stream.is_some();

        ui.horizontal(|ui| {
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
                    egui::RichText::new("○ Mic Disabled (Turn on in Capture Devices tab)")
                        .color(egui::Color32::from_rgb(149, 165, 166))
                        .monospace()
                        .size(11.0),
                );
            }

            ui.add_space(16.0);

            let mut monitor = state.mic.config.monitor;
            if ui
                .checkbox(&mut monitor, "Monitor")
                .on_hover_text("Live listen to the microphone")
                .changed()
            {
                state.mic.config.monitor = monitor;
                if let Some(stream) = &state.mic.active_stream {
                    if let Ok(mut w) = stream.config.write() {
                        *w = state.mic.config.clone();
                    }
                }
                crate::config::save_global_hardware_config(state);
                changed = true;
            }
        });

        ui.add_space(6.0);

        // Volume Slider
        ui.horizontal(|ui| {
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
                if let Some(stream) = &state.mic.active_stream {
                    if let Ok(mut w) = stream.config.write() {
                        *w = state.mic.config.clone();
                    }
                }
                crate::config::save_global_hardware_config(state);
                changed = true;
            }
        });

        ui.add_space(6.0);

        // Live Feedback dB Peak Meter with Green / Orange / Red zones
        let current_db = f32::from_bits(state.mic.current_level_db.load(Ordering::Relaxed));
        let peak_db = f32::from_bits(state.mic.peak_level_db.load(Ordering::Relaxed));
        let clipping = state.mic.is_clipping.load(Ordering::Relaxed) || current_db >= 0.0;

        draw_db_meter(ui, current_db, peak_db, clipping);
    });

    ui.add_space(6.0);

    // --- Audio Filters Section (OBS-style filter stack) ---
    technical_group(ui, "Audio Filters (In Order: Gate -> Gain -> Compressor):", |ui| {
        // Persistent selected filter tab in UI memory
        let filter_tab_id = ui.id().with("selected_mic_filter_tab");
        let mut selected_tab = ui.data_mut(|d| {
            d.get_temp::<SelectedFilterTab>(filter_tab_id)
                .unwrap_or(SelectedFilterTab::NoiseGate)
        });

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
                    let btn = egui::Button::new(if is_active { text.strong().color(egui::Color32::WHITE) } else { text.color(egui::Color32::from_rgb(170, 170, 170)) })
                        .fill(if is_active { egui::Color32::from_rgb(33, 97, 159) } else { egui::Color32::from_rgb(34, 34, 34) });
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
                    let btn = egui::Button::new(if is_active { text.strong().color(egui::Color32::WHITE) } else { text.color(egui::Color32::from_rgb(170, 170, 170)) })
                        .fill(if is_active { egui::Color32::from_rgb(33, 97, 159) } else { egui::Color32::from_rgb(34, 34, 34) });
                    if ui.add(btn).clicked() {
                        selected_tab = SelectedFilterTab::Gain;
                    }
                });

                ui.add_space(2.0);

                // Filter 3: Compressor
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
                    let btn = egui::Button::new(if is_active { text.strong().color(egui::Color32::WHITE) } else { text.color(egui::Color32::from_rgb(170, 170, 170)) })
                        .fill(if is_active { egui::Color32::from_rgb(33, 97, 159) } else { egui::Color32::from_rgb(34, 34, 34) });
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
                            ui.label(egui::RichText::new("Close Threshold:").monospace().size(11.0).color(egui::Color32::from_rgb(170, 170, 170)));
                            slider_changed |= ui.add(egui::Slider::new(&mut gate.close_threshold_db, -96.0..=0.0).suffix(" dB").custom_formatter(|v, _| format!("{:.2} dB", v))).changed();
                        });

                        ui.add_space(3.0);

                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Open Threshold: ").monospace().size(11.0).color(egui::Color32::from_rgb(170, 170, 170)));
                            slider_changed |= ui.add(egui::Slider::new(&mut gate.open_threshold_db, -96.0..=0.0).suffix(" dB").custom_formatter(|v, _| format!("{:.2} dB", v))).changed();
                        });

                        ui.add_space(3.0);

                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Attack Time:    ").monospace().size(11.0).color(egui::Color32::from_rgb(170, 170, 170)));
                            slider_changed |= ui.add(egui::Slider::new(&mut gate.attack_time_ms, 0.0..=500.0).suffix(" ms").custom_formatter(|v, _| format!("{:.0} ms", v))).changed();
                        });

                        ui.add_space(3.0);

                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Hold Time:      ").monospace().size(11.0).color(egui::Color32::from_rgb(170, 170, 170)));
                            slider_changed |= ui.add(egui::Slider::new(&mut gate.hold_time_ms, 0.0..=2000.0).suffix(" ms").custom_formatter(|v, _| format!("{:.0} ms", v))).changed();
                        });

                        ui.add_space(3.0);

                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Release Time:   ").monospace().size(11.0).color(egui::Color32::from_rgb(170, 170, 170)));
                            slider_changed |= ui.add(egui::Slider::new(&mut gate.release_time_ms, 0.0..=1000.0).suffix(" ms").custom_formatter(|v, _| format!("{:.0} ms", v))).changed();
                        });
                    }
                    SelectedFilterTab::Gain => {
                        let gain = &mut state.mic.config.gain;
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Gain:").monospace().size(11.0).color(egui::Color32::from_rgb(170, 170, 170)));
                            slider_changed |= ui.add(egui::Slider::new(&mut gain.gain_db, -30.0..=30.0).suffix(" dB").custom_formatter(|v, _| format!("{:.2} dB", v))).changed();
                        });
                    }
                    SelectedFilterTab::Compressor => {
                        let comp = &mut state.mic.config.compressor;
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Ratio:      ").monospace().size(11.0).color(egui::Color32::from_rgb(170, 170, 170)));
                            slider_changed |= ui.add(egui::Slider::new(&mut comp.ratio, 1.0..=32.0).custom_formatter(|v, _| format!("{:.2}:1", v))).changed();
                        });

                        ui.add_space(3.0);

                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Threshold:  ").monospace().size(11.0).color(egui::Color32::from_rgb(170, 170, 170)));
                            slider_changed |= ui.add(egui::Slider::new(&mut comp.threshold_db, -60.0..=0.0).suffix(" dB").custom_formatter(|v, _| format!("{:.2} dB", v))).changed();
                        });

                        ui.add_space(3.0);

                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Attack:     ").monospace().size(11.0).color(egui::Color32::from_rgb(170, 170, 170)));
                            slider_changed |= ui.add(egui::Slider::new(&mut comp.attack_time_ms, 1.0..=500.0).suffix(" ms").custom_formatter(|v, _| format!("{:.0} ms", v))).changed();
                        });

                        ui.add_space(3.0);

                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Release:    ").monospace().size(11.0).color(egui::Color32::from_rgb(170, 170, 170)));
                            slider_changed |= ui.add(egui::Slider::new(&mut comp.release_time_ms, 1.0..=1000.0).suffix(" ms").custom_formatter(|v, _| format!("{:.0} ms", v))).changed();
                        });

                        ui.add_space(3.0);

                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Output Gain:").monospace().size(11.0).color(egui::Color32::from_rgb(170, 170, 170)));
                            slider_changed |= ui.add(egui::Slider::new(&mut comp.output_gain_db, -32.0..=32.0).suffix(" dB").custom_formatter(|v, _| format!("{:.2} dB", v))).changed();
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
        let (rect, _response) = ui.allocate_exact_size(
            egui::vec2(meter_width, meter_height),
            egui::Sense::hover(),
        );

        if ui.is_rect_visible(rect) {
            let painter = ui.painter();

            // Background trough
            painter.rect_filled(rect, egui::Rounding::ZERO, egui::Color32::from_rgb(17, 17, 17));
            painter.rect_stroke(rect, egui::Rounding::ZERO, egui::Stroke::new(1.0, egui::Color32::from_rgb(53, 53, 53)));

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
                let r_green = egui::Rect::from_min_size(rect.min, egui::vec2(green_w, rect.height()));
                painter.rect_filled(r_green, egui::Rounding::ZERO, egui::Color32::from_rgb(46, 204, 113));

                // Orange part
                if fill_width > p_green_end * rect.width() {
                    let orange_start = rect.min.x + p_green_end * rect.width();
                    let orange_w = (fill_width - p_green_end * rect.width()).min((p_orange_end - p_green_end) * rect.width());
                    let r_orange = egui::Rect::from_min_size(
                        egui::pos2(orange_start, rect.min.y),
                        egui::vec2(orange_w, rect.height()),
                    );
                    painter.rect_filled(r_orange, egui::Rounding::ZERO, egui::Color32::from_rgb(243, 156, 18));
                }

                // Red part
                if fill_width > p_orange_end * rect.width() {
                    let red_start = rect.min.x + p_orange_end * rect.width();
                    let red_w = fill_width - p_orange_end * rect.width();
                    let r_red = egui::Rect::from_min_size(
                        egui::pos2(red_start, rect.min.y),
                        egui::vec2(red_w, rect.height()),
                    );
                    painter.rect_filled(r_red, egui::Rounding::ZERO, egui::Color32::from_rgb(231, 76, 60));
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
                [egui::pos2(line_20_x, rect.min.y), egui::pos2(line_20_x, rect.max.y)],
                egui::Stroke::new(1.0, egui::Color32::from_rgb(30, 30, 30)),
            );
            let line_9_x = rect.min.x + p_orange_end * rect.width();
            painter.line_segment(
                [egui::pos2(line_9_x, rect.min.y), egui::pos2(line_9_x, rect.max.y)],
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
