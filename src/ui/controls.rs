use crate::app::{models::SettingsTab, AppState};
use eframe::egui;
use std::sync::atomic::Ordering;

use crate::ui::{devices, filters, profiles};

pub fn technical_group<R>(
    ui: &mut egui::Ui,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::none()
        .fill(egui::Color32::from_rgb(22, 22, 22)) // #161616
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(53, 53, 53))) // #353535
        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
        .rounding(egui::Rounding::ZERO)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !title.is_empty() {
                ui.label(
                    egui::RichText::new(title)
                        .monospace()
                        .size(11.0)
                        .strong()
                        .color(egui::Color32::from_rgb(224, 224, 224)),
                );
                ui.add_space(4.0);
            }
            add_contents(ui)
        })
        .inner
}

pub fn slider_item(
    ui: &mut egui::Ui,
    label: &str,
    slider: egui::Slider,
    tip: Option<&str>,
) -> egui::Response {
    ui.horizontal(|ui| {
        let lbl = ui.add_sized(
            [130.0, 18.0],
            egui::Label::new(
                egui::RichText::new(label)
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(153, 153, 153)),
            ),
        );
        if let Some(t) = tip {
            lbl.on_hover_text(t);
        }
        let avail_w = (ui.available_width() - 4.0).max(60.0);
        let mut resp = ui.add_sized([avail_w, 18.0], slider);
        if let Some(t) = tip {
            resp = resp.on_hover_text(t);
        }
        resp
    })
    .inner
}

pub fn two_columns<R>(
    ui: &mut egui::Ui,
    add_contents: impl FnOnce(&mut egui::Ui, &mut egui::Ui) -> R,
) -> R {
    ui.columns(2, |cols| {
        cols[0].spacing_mut().item_spacing = egui::vec2(6.0, 3.0);
        cols[1].spacing_mut().item_spacing = egui::vec2(6.0, 3.0);
        let (c0, rest) = cols.split_at_mut(1);
        let c1 = &mut rest[0];
        add_contents(&mut c0[0], c1)
    })
}

pub fn percent_slider<'a>(value: &'a mut f32, range: std::ops::RangeInclusive<f32>) -> egui::Slider<'a> {
    egui::Slider::new(value, range)
        .step_by(0.01)
        .custom_formatter(|n, _| format!("{:.0}%", n * 100.0))
        .custom_parser(|s| {
            let s = s.trim().trim_end_matches('%').trim();
            s.parse::<f64>().ok().map(|v| if v > 1.0 { (v / 100.0) as f64 } else { v })
        })
}

pub fn percent_slider_range<'a>(value: &'a mut f32, range: std::ops::RangeInclusive<f32>, max_pct: f64) -> egui::Slider<'a> {
    egui::Slider::new(value, range)
        .step_by(0.01)
        .custom_formatter(|n, _| format!("{:.0}%", n * 100.0))
        .custom_parser(move |s| {
            let s = s.trim().trim_end_matches('%').trim();
            s.parse::<f64>().ok().map(|v| if v > max_pct { (v / 100.0) as f64 } else { v })
        })
}

pub fn mult_slider<'a>(value: &'a mut f32, range: std::ops::RangeInclusive<f32>) -> egui::Slider<'a> {
    egui::Slider::new(value, range)
        .step_by(0.05)
        .custom_formatter(|n, _| format!("{:.2}x", n))
        .custom_parser(|s| {
            let s = s.trim().trim_end_matches('x').trim();
            s.parse::<f64>().ok()
        })
}

pub fn sub_heading(ui: &mut egui::Ui, title: &str) {
    ui.add_space(3.0);
    ui.label(
        egui::RichText::new(title)
            .monospace()
            .size(11.0)
            .strong()
            .color(egui::Color32::from_rgb(153, 153, 153)),
    );
    ui.add_space(2.0);
}

pub fn technical_separator(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.top(),
        egui::Stroke::new(1.0, egui::Color32::from_rgb(40, 40, 40)),
    );
    ui.add_space(4.0);
}

pub fn layout_top_ui(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;

    // --- Pinned Top Section: Real Logo on left + Header Controls on right ---
    egui::Frame::none()
        .fill(egui::Color32::from_rgb(30, 30, 30)) // #1e1e1e
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(51, 51, 51))) // #333333
        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
        .rounding(egui::Rounding::ZERO)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                // Logo in sharp border box on the left
                if let Some(logo) = &state.logo_texture {
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgb(10, 10, 10)) // #0a0a0a
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(51, 51, 51)))
                        .inner_margin(1.0)
                        .rounding(egui::Rounding::ZERO)
                        .show(ui, |ui| {
                            ui.add(egui::Image::new(logo).max_height(160.0));
                        });
                }

                ui.add_space(12.0);

                // Right column: Title, Stream/Audio row, Profile row, Tag row
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("Michadame Viewer")
                            .size(16.0)
                            .strong()
                            .color(egui::Color32::from_rgb(238, 238, 238)),
                    );
                    ui.add_space(6.0);

                    // Row 1: Stream Start/Stop + Compact Audio Meter (120x16)
                    ui.horizontal(|ui| {
                        if state.ui.video_window_open {
                            let stop_btn = egui::Button::new(
                                egui::RichText::new("Stop Stream")
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(255, 204, 204)),
                            )
                            .fill(egui::Color32::from_rgb(90, 24, 24)) // #5a1818
                            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(122, 40, 40)))
                            .rounding(egui::Rounding::ZERO);
                            if ui.add(stop_btn).clicked() {
                                state.stop_stream(ui.ctx());
                                state.info("Stream stopped.");
                            }
                        } else {
                            let can_stream = !state.hardware.selected_video_device.is_empty()
                                && state.hardware.selected_resolution.0 > 0;
                            let start_btn = egui::Button::new(
                                egui::RichText::new("Start Stream")
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(204, 255, 204)),
                            )
                            .fill(egui::Color32::from_rgb(26, 74, 26)) // #1a4a1a
                            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(40, 106, 40)))
                            .rounding(egui::Rounding::ZERO);
                            if ui.add_enabled(can_stream, start_btn).clicked() {
                                state.start_stream(ui.ctx());
                                state.info("Stream starting...");
                            }
                            if !can_stream {
                                ui.label(
                                    egui::RichText::new("Select Video Format/Resolution first.")
                                        .weak()
                                        .small(),
                                );
                            }
                        }

                        // Compact mechanical audio meter bar
                        let peak = state
                            .hardware
                            .audio_peak_amplitude
                            .swap(0, Ordering::Relaxed) as f32
                            / 1000.0;
                        let color = if peak > 0.9 {
                            egui::Color32::RED
                        } else if peak > 0.7 {
                            egui::Color32::YELLOW
                        } else {
                            egui::Color32::from_rgb(42, 122, 42) // #2a7a2a
                        };
                        let meter = egui::ProgressBar::new(peak.min(1.0))
                            .text(
                                egui::RichText::new(format!("Audio: {:.0}%", peak * 100.0))
                                    .monospace()
                                    .strong()
                                    .size(10.0)
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(color)
                            .desired_width(120.0)
                            .rounding(egui::Rounding::ZERO);
                        ui.add(meter);
                    });

                    ui.add_space(5.0);

                    // Row 2: Profile management inline row
                    changed |= profiles::draw_profile_management(ui, state);

                    ui.add_space(5.0);

                    // Row 3: Mining Tag inline row
                    changed |= super::bank::draw_tag_setting(ui, state);

                    ui.label(
                        egui::RichText::new(
                            "Newly mined words are tagged with this (e.g. the game being played). Leave empty for no tag.",
                        )
                        .size(10.5)
                        .color(egui::Color32::from_rgb(119, 119, 119)),
                    );
                });
            });
        });

    ui.add_space(6.0);

    // --- Tab Notebook Headers: Classic Technical Linux Tab Bar ---
    egui::Frame::none()
        .fill(egui::Color32::from_rgb(20, 20, 20)) // #141414
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(58, 58, 58))) // #3a3a3a
        .inner_margin(egui::Margin {
            left: 6.0,
            right: 6.0,
            top: 4.0,
            bottom: 0.0,
        })
        .rounding(egui::Rounding::ZERO)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);
                let tabs = [
                    (SettingsTab::Shaders, "Shaders"),
                    (SettingsTab::Geometry, "Geometry / Deinterlace"),
                    (SettingsTab::Effects, "Effects / Bezel"),
                    (SettingsTab::Devices, "Capture Devices"),
                    (SettingsTab::AudioReplay, "Audio / Replay"),
                    (SettingsTab::OcrDict, "OCR / Dictionaries"),
                    (SettingsTab::Hotkeys, "Hotkeys"),
                ];
                for (tab, label) in tabs {
                    let is_active = state.ui.active_settings_tab == tab;
                    let tab_btn = if is_active {
                        egui::Button::new(
                            egui::RichText::new(label)
                                .monospace()
                                .size(11.0)
                                .strong()
                                .color(egui::Color32::WHITE),
                        )
                        .fill(egui::Color32::from_rgb(26, 26, 26)) // #1a1a1a
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(74, 74, 74)))
                        .rounding(egui::Rounding::ZERO)
                    } else {
                        egui::Button::new(
                            egui::RichText::new(label)
                                .monospace()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(136, 136, 136)), // #888888
                        )
                        .fill(egui::Color32::from_rgb(34, 34, 34)) // #222222
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(56, 56, 56))) // #383838
                        .rounding(egui::Rounding::ZERO)
                    };
                    if ui.add(tab_btn).clicked() {
                        state.ui.active_settings_tab = tab;
                    }
                }
            });
        });

    ui.add_space(4.0);

    // --- Tab Contents Body: #1a1a1a ---
    egui::Frame::none()
        .fill(egui::Color32::from_rgb(26, 26, 26)) // #1a1a1a
        .inner_margin(egui::Margin::symmetric(2.0, 4.0))
        .rounding(egui::Rounding::ZERO)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            match state.ui.active_settings_tab {
                SettingsTab::Shaders => {
                    changed |= filters::draw_shaders_tab(ui, state);
                }
                SettingsTab::Geometry => {
                    changed |= filters::draw_geometry_tab(ui, state);
                }
                SettingsTab::Effects => {
                    changed |= filters::draw_effects_tab(ui, state);
                }
                SettingsTab::Devices => {
                    changed |= devices::draw_device_selectors(ui, state);
                }
                SettingsTab::AudioReplay => {
                    if crate::replay::ui::draw_audio_filter_group(&mut state.replay, ui) {
                        crate::config::save_config(state);
                        crate::config::save_global_hardware_config(state);
                        let _ = crate::config::save_replay_config(&state.replay.config);
                        changed = true;
                    }

                    ui.add_space(4.0);

                    if crate::replay::ui::draw_replay_buffer_group(
                        &mut state.replay,
                        ui,
                        state.ui.video_window_open,
                    ) {
                        changed = true;
                        if let Err(error) =
                            crate::config::save_replay_config(&state.replay.config)
                        {
                            state.error(format!("Could not save replay settings: {error}"));
                        }
                    }
                }
                SettingsTab::OcrDict => {
                    draw_ocr_dict_tab(ui, state, &mut changed);
                }
                SettingsTab::Hotkeys => {
                    draw_hotkeys_tab(ui);
                }
            }
        });

    changed
}

fn draw_ocr_dict_tab(ui: &mut egui::Ui, state: &mut AppState, changed: &mut bool) {
    technical_group(ui, "Quick OCR (Google Lens):", |ui| {
        two_columns(ui, |col1, col2| {
            let slider = egui::Slider::new(&mut state.ocr.sticky_distance, 0.0..=2.0)
                .step_by(0.05)
                .suffix("x");
            if slider_item(
                col1,
                "Sticky Box Distance:",
                slider,
                Some(
                    "Maximum vertical distance between adjacent lines to merge into one block (as a multiple of line height).\n• 0.0: Keep all lines separate\n• 0.6: Default (tight, dialogue lines only)\n• 1.0+: Looser merging",
                ),
            )
            .changed()
            {
                state.ocr.recompute_boxes();
                crate::config::save_config(state);
                *changed = true;
            }

            let slider = egui::Slider::new(&mut state.ocr.timeout_seconds, 0..=300).suffix("s");
            if slider_item(
                col2,
                "Auto-clear Timeout:",
                slider,
                Some(
                    "Time in seconds before OCR scan results automatically disappear.\n• 0s: Disabled (keep until Shift+Space or dismissed)\n• 45s: Default",
                ),
            )
            .changed()
            {
                crate::config::save_config(state);
                *changed = true;
            }
        });

        ui.add_space(4.0);

        if ui
            .checkbox(&mut state.ocr.hide_overlay, "Hide OCR boxes overlay")
            .on_hover_text(
                "Hides the blue bounding boxes and replacement text overlay on the video feed.\nDictionary lookup on hover and popup behavior will continue to work normally.",
            )
            .changed()
        {
            crate::config::save_config(state);
            *changed = true;
        }

        ui.label(
            egui::RichText::new("Space = capture OCR, Shift + Space = clear OCR boxes")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(102, 102, 102)),
        );

        if state.ocr.is_processing.load(Ordering::Relaxed) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Processing OCR with Google Lens...");
            });
        }
    });

    ui.add_space(4.0);

    technical_group(ui, "Japanese Dictionary (Jitendex):", |ui| {
        if state.dict.update_available {
            ui.label(
                egui::RichText::new("(!) An update is available for Jitendex!")
                    .color(egui::Color32::from_rgb(251, 191, 36))
                    .monospace()
                    .size(11.0),
            );
        }

        if let Some(meta) = &state.dict.installed_metadata {
            ui.label(
                egui::RichText::new(format!(
                    "Installed: {} entries (rev {})",
                    meta.total_entries, meta.revision
                ))
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(136, 136, 136)),
            );
        } else {
            ui.label(
                egui::RichText::new("Status: Not installed - click Sync to download")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(251, 191, 36)),
            );
        }

        ui.add_space(3.0);

        let is_syncing = state.dict.is_syncing.load(Ordering::Relaxed);
        if is_syncing {
            let (msg, prog) = if let Ok(lock) = state.dict.sync_progress.lock() {
                lock.clone().unwrap_or(("Syncing dictionary...".into(), 0.0))
            } else {
                ("Syncing dictionary...".into(), 0.0)
            };
            ui.add(egui::ProgressBar::new(prog).text(msg).animate(true));
        } else {
            ui.horizontal(|ui| {
                let button_text = if state.dict.update_available {
                    "Sync Jitendex Dictionary (!)"
                } else {
                    "Sync Jitendex Dictionary"
                };

                let sync_btn = egui::Button::new(
                    egui::RichText::new(button_text).color(if state.dict.update_available {
                        egui::Color32::from_rgb(251, 191, 36)
                    } else {
                        ui.visuals().text_color()
                    }),
                );

                if ui
                    .add(sync_btn)
                    .on_hover_text("Download and index the latest Jitendex dictionary release from jitendex.org")
                    .clicked()
                {
                    state.dict.trigger_sync();
                }

                if state.dict.is_checking_update.load(Ordering::Relaxed) {
                    ui.spinner();
                    ui.label(egui::RichText::new("Checking for updates...").weak().small());
                } else if ui.small_button("Check Updates").clicked() {
                    state.dict.trigger_check_version();
                }
            });
        }
    });

    ui.add_space(4.0);

    technical_group(ui, "", |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Frequency Dictionary (Jiten Global):")
                    .monospace()
                    .size(11.0)
                    .strong()
                    .color(egui::Color32::from_rgb(224, 224, 224)),
            );
            if state.dict.freq_update_available {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("(!) Update available")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(208, 144, 48)),
                    );
                });
            }
        });
        ui.add_space(4.0);

        if let Some(meta) = &state.dict.installed_freq_metadata {
            ui.label(
                egui::RichText::new(format!(
                    "Installed: {} entries (rev {})",
                    meta.total_entries, meta.revision
                ))
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(136, 136, 136)),
            );
        } else {
            ui.label(
                egui::RichText::new("Status: Not installed - click Sync to download")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(251, 191, 36)),
            );
        }

        ui.add_space(3.0);

        let is_freq_syncing = state.dict.is_freq_syncing.load(Ordering::Relaxed);
        if is_freq_syncing {
            let (msg, prog) = if let Ok(lock) = state.dict.freq_sync_progress.lock() {
                lock.clone().unwrap_or(("Syncing frequency dictionary...".into(), 0.0))
            } else {
                ("Syncing frequency dictionary...".into(), 0.0)
            };
            ui.add(egui::ProgressBar::new(prog).text(msg).animate(true));
        } else {
            ui.horizontal(|ui| {
                let button_text = if state.dict.freq_update_available {
                    "Sync Jiten Frequency (!)"
                } else {
                    "Sync Jiten Frequency"
                };

                let sync_btn = egui::Button::new(
                    egui::RichText::new(button_text).color(if state.dict.freq_update_available {
                        egui::Color32::from_rgb(251, 191, 36)
                    } else {
                        ui.visuals().text_color()
                    }),
                );

                if ui
                    .add(sync_btn)
                    .on_hover_text("Download and index the latest Jiten Global frequency dictionary from jiten.moe")
                    .clicked()
                {
                    state.dict.trigger_freq_sync();
                }

                if state.dict.is_checking_freq_update.load(Ordering::Relaxed) {
                    ui.spinner();
                    ui.label(egui::RichText::new("Checking for updates...").weak().small());
                } else if ui.small_button("Check Updates").clicked() {
                    state.dict.trigger_check_freq_version();
                }
            });
        }
    });
}

fn draw_hotkeys_tab(ui: &mut egui::Ui) {
    technical_group(ui, "Keyboard Shortcuts:", |ui| {
        let avail_w = ui.available_width();
        egui::Grid::new("hotkeys_grid")
            .striped(true)
            .num_columns(2)
            .min_col_width(160.0)
            .spacing([24.0, 6.0])
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new("Key")
                        .monospace()
                        .size(11.0)
                        .strong()
                        .color(egui::Color32::from_rgb(119, 119, 119)),
                );
                ui.label(
                    egui::RichText::new("Action")
                        .monospace()
                        .size(11.0)
                        .strong()
                        .color(egui::Color32::from_rgb(119, 119, 119)),
                );
                ui.end_row();

                let shortcuts = [
                    ("F", "Toggle Fullscreen"),
                    ("M", "Toggle Controls (Settings Window)"),
                    ("B", "Toggle Mining Bank Window"),
                    ("C", "Cycle CRT Filter (Off / Lottes / Halo)"),
                    ("K", "Toggle Lights Off Night Mode"),
                    ("G", "Toggle 480p Pixelate Filter"),
                    ("Q", "Stop Stream Confirmation Dialog"),
                    ("Escape", "Exit Fullscreen / Close Dialogs / Dismiss Popups"),
                    ("Space", "Capture OCR (Google Lens)"),
                    ("Shift + Space", "Clear OCR Bounding Boxes"),
                    ("Ctrl + C", "Export 15s Replay Clip to Clipboard"),
                    ("Ctrl + Shift + C", "Export 7s Audio-Only Clip to Clipboard"),
                    (
                        "F1 – F12",
                        "Save Replay Buffer to Disk (Durations configured in Audio / Replay tab)",
                    ),
                ];

                for (key, desc) in shortcuts {
                    ui.add_sized(
                        [160.0, 18.0],
                        egui::Label::new(
                            egui::RichText::new(key)
                                .monospace()
                                .size(11.0)
                                .strong()
                                .color(egui::Color32::WHITE),
                        ),
                    );
                    let action_w = (avail_w - 160.0 - 24.0).max(100.0);
                    ui.add_sized(
                        [action_w, 18.0],
                        egui::Label::new(
                            egui::RichText::new(desc)
                                .size(11.0)
                                .color(egui::Color32::from_rgb(208, 208, 208)),
                        ),
                    );
                    ui.end_row();
                }
            });
    });
}
