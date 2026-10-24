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

                    // Row 1: Stream Start/Stop / Video File + Compact Audio Meter (120x16)
                    ui.horizontal(|ui| {
                        if state.ui.video_window_open {
                            let is_gamescope = state.gamescope_hook.as_ref().map_or(false, |h| h.is_active);
                            let is_pc98 = state.pc98.is_active;
                            let is_pc88 = state.pc88.is_active;
                            let is_file_mode = state.video_player.is_some();
                            let label = if is_gamescope {
                                "Stop Gamescope"
                            } else if is_pc98 {
                                "Stop PC-98"
                            } else if is_pc88 {
                                "Stop PC-88"
                            } else if is_file_mode {
                                "Stop Video"
                            } else {
                                "Stop Stream"
                            };
                            let stop_btn = egui::Button::new(
                                egui::RichText::new(label)
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(255, 204, 204)),
                            )
                            .fill(egui::Color32::from_rgb(90, 24, 24)) // #5a1818
                            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(122, 40, 40)))
                            .rounding(egui::Rounding::ZERO);
                            if ui.add(stop_btn).clicked() {
                                state.stop_stream(ui.ctx());
                                state.info(if is_gamescope {
                                    "Gamescope hook stopped."
                                } else if is_pc98 {
                                    "PC-98 emulator stopped."
                                } else if is_pc88 {
                                    "PC-88 emulator stopped."
                                } else if is_file_mode {
                                    "Video playback stopped."
                                } else {
                                    "Stream stopped."
                                });
                            }

                            if is_gamescope {
                                if let Some(hook) = &state.gamescope_hook {
                                    let (icon, input_label, color) = if hook.passthrough_enabled {
                                        ("🎮", "Passthrough", egui::Color32::from_rgb(140, 240, 140))
                                    } else {
                                        ("🚫", "Input Paused", egui::Color32::from_rgb(255, 120, 120))
                                    };
                                    ui.label(
                                        egui::RichText::new(format!("{} {}", icon, input_label))
                                            .monospace()
                                            .size(10.5)
                                            .color(color),
                                    );
                                }
                            } else if is_pc98 {
                                let (icon, input_label, color) = if state.pc98.passthrough_enabled {
                                    ("⌨", "Passthrough", egui::Color32::from_rgb(140, 240, 140))
                                } else {
                                    ("🚫", "Input Paused", egui::Color32::from_rgb(255, 120, 120))
                                };
                                ui.label(
                                    egui::RichText::new(format!("{} {}", icon, input_label))
                                        .monospace()
                                        .size(10.5)
                                        .color(color),
                                );
                                if ui.small_button("Reset").on_hover_text("Warm-reset PC-98").clicked() {
                                    state.pc98.send_command(crate::pc98::Pc98Command::Reset);
                                }
                                if ui.small_button("NP2 Menu").on_hover_text("Toggle Neko Project II OSD menu (F12)").clicked() {
                                    state.pc98.send_command(crate::pc98::Pc98Command::TriggerMenu);
                                }
                            } else if is_pc88 {
                                let (icon, input_label, color) = if state.pc88.passthrough_enabled {
                                    ("⌨", "Passthrough", egui::Color32::from_rgb(140, 240, 140))
                                } else {
                                    ("🚫", "Input Paused", egui::Color32::from_rgb(255, 120, 120))
                                };
                                ui.label(
                                    egui::RichText::new(format!("{} {}", icon, input_label))
                                        .monospace()
                                        .size(10.5)
                                        .color(color),
                                );
                                if ui.small_button("Reset").on_hover_text("Warm-reset PC-88").clicked() {
                                    state.pc88.send_command(crate::pc88::Pc88Command::Reset);
                                }
                                if ui.small_button("Quasi88 Menu").on_hover_text("Toggle Quasi88 OSD menu (F12)").clicked() {
                                    state.pc88.send_command(crate::pc88::Pc88Command::TriggerMenu);
                                }
                            }
                        } else {
                            let can_stream = !state.hardware.selected_video_device.is_empty()
                                && state.hardware.selected_resolution.0 > 0;
                            let start_btn = egui::Button::new(
                                egui::RichText::new("HDMI")
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

                            let open_btn = egui::Button::new(
                                egui::RichText::new("Video File")
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(204, 230, 255)),
                            )
                            .fill(egui::Color32::from_rgb(24, 50, 80)) // #183250
                            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(40, 80, 130)))
                            .rounding(egui::Rounding::ZERO);
                            if ui.add(open_btn).clicked() {
                                state.open_video_file_dialog(ui.ctx());
                            }

                            let gamescope_btn = egui::Button::new(
                                egui::RichText::new("Local Game")
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(255, 230, 150)),
                            )
                            .fill(egui::Color32::from_rgb(80, 60, 20))
                            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(130, 100, 30)))
                            .rounding(egui::Rounding::ZERO);
                            if ui.add(gamescope_btn).clicked() {
                                state.start_gamescope_hook(ui.ctx());
                            }

                            let pc98_btn = egui::Button::new(
                                egui::RichText::new("PC98 Game")
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(210, 255, 220)),
                            )
                            .fill(egui::Color32::from_rgb(24, 72, 42))
                            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(42, 118, 68)))
                            .rounding(egui::Rounding::ZERO);
                            if ui
                                .add(pc98_btn)
                                .on_hover_text("Boot embedded NEC PC-98 emulator (Neko Project II Kai) inside CRT monitor")
                                .clicked()
                            {
                                state.retro_pc_system = crate::pc88::RetroPcSystem::Pc98;
                                state.start_pc98_mode(ui.ctx());
                            }

                            let pc88_btn = egui::Button::new(
                                egui::RichText::new("PC88 Game")
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(210, 255, 220)),
                            )
                            .fill(egui::Color32::from_rgb(24, 72, 42))
                            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(42, 118, 68)))
                            .rounding(egui::Rounding::ZERO);
                            if ui
                                .add(pc88_btn)
                                .on_hover_text("Boot embedded NEC PC-88 emulator (Quasi88) inside CRT monitor")
                                .clicked()
                            {
                                state.retro_pc_system = crate::pc88::RetroPcSystem::Pc88;
                                state.start_pc88_mode(ui.ctx());
                            }

                            if !can_stream {
                                ui.label(
                                    egui::RichText::new("Select format for capture card, open local video, hook Gamescope, or boot PC-98/PC-88.")
                                        .weak()
                                        .small(),
                                );
                            }
                        }

                        // Compact mechanical audio meter bar
                        let peak = state.hardware.live_audio_peak;
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

                        ui.add_space(8.0);
                        // Same toggle as the Audio / Replay tab; both reflect `replay.runtime`.
                        crate::replay::ui::draw_toggle(
                            &mut state.replay,
                            ui,
                            state.ui.video_window_open,
                        );

                        ui.add_space(8.0);
                        let studio_btn = egui::Button::new(
                            egui::RichText::new("Studio (L)")
                                .monospace()
                                .strong()
                                .size(10.5)
                                .color(if state.ui.studio_window_open {
                                    egui::Color32::from_rgb(190, 150, 255)
                                } else {
                                    egui::Color32::from_rgb(208, 208, 208)
                                }),
                        )
                        .fill(if state.ui.studio_window_open {
                            egui::Color32::from_rgb(60, 20, 100)
                        } else {
                            egui::Color32::from_rgb(44, 44, 44)
                        })
                        .stroke(egui::Stroke::new(
                            1.0,
                            if state.ui.studio_window_open {
                                egui::Color32::from_rgb(145, 70, 255)
                            } else {
                                egui::Color32::from_rgb(72, 72, 72)
                            },
                        ))
                        .rounding(egui::Rounding::ZERO);
                        if ui
                            .add(studio_btn)
                            .on_hover_text("Open Twitch Studio dashboard window (hotkey: L)")
                            .clicked()
                        {
                            state.ui.studio_window_open = !state.ui.studio_window_open;
                            changed = true;
                        }
                    });

                    ui.add_space(5.0);

                    // Row 2: Profile management inline row
                    changed |= profiles::draw_profile_management(ui, state);

                    ui.add_space(5.0);

                    // Row 3: Mining Tag inline row
                    changed |= super::bank::draw_tag_setting(ui, state);

                    ui.add_space(5.0);

                    // Row 4: Master Volume horizontal slider
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Master Volume:")
                                .monospace()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(150, 150, 150)),
                        );
                        let mut vol = state.hardware.master_volume;
                        let slider = percent_slider(&mut vol, 0.0..=1.0);
                        if ui
                            .add(slider)
                            .on_hover_text("Controls the overall master volume of the stream (both live stream and file playback streams)")
                            .changed()
                        {
                            state.set_master_volume(vol);
                            crate::config::save_config(state);
                            crate::config::save_global_hardware_config(state);
                            changed = true;
                        }
                    });

                    ui.add_space(5.0);

                    // Row 5: Steam Game selector
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Steam Game:")
                                .monospace()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(150, 150, 150)),
                        );
                        let selected_game_text = state
                            .selected_steam_game
                            .as_ref()
                            .and_then(|appid| {
                                state
                                    .steam_games
                                    .iter()
                                    .find(|g| &g.appid == appid)
                                    .map(|g| g.name.as_str())
                            })
                            .unwrap_or("None");

                        egui::ComboBox::from_id_source("steam_game_selector")
                            .selected_text(selected_game_text)
                            .width(240.0)
                            .show_ui(ui, |ui| {
                                let mut game_changed = ui
                                    .selectable_value(&mut state.selected_steam_game, None, "None")
                                    .changed();
                                for game in &state.steam_games {
                                    game_changed |= ui
                                        .selectable_value(
                                            &mut state.selected_steam_game,
                                            Some(game.appid.clone()),
                                            &game.name,
                                        )
                                        .changed();
                                }
                                if game_changed {
                                    crate::config::save_config(state);
                                    crate::config::save_global_hardware_config(state);
                                    changed = true;
                                }
                            });

                        if ui.button("Refresh").clicked() {
                            state.refresh_steam_games();
                            changed = true;
                        }
                    });
                });
            });
        });

    ui.add_space(6.0);

    // --- Tab Notebook Headers: Classic Technical Linux Single-Row Balanced Tab Bar ---
    egui::Frame::none()
        .fill(egui::Color32::from_rgb(20, 20, 20)) // #141414
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(58, 58, 58))) // #3a3a3a
        .inner_margin(egui::Margin {
            left: 4.0,
            right: 4.0,
            top: 4.0,
            bottom: 0.0,
        })
        .rounding(egui::Rounding::ZERO)
        .show(ui, |ui| {
            let avail_w = ui.available_width();
            ui.set_width(avail_w);
            ui.horizontal(|ui| {
                let spacing = 2.0;
                ui.spacing_mut().item_spacing = egui::vec2(spacing, 0.0);
                let tabs = [
                    (SettingsTab::Shaders, "Shaders"),
                    (SettingsTab::GeometryBezel, "Geometry & Bezel"),
                    (SettingsTab::SpecialFx, "Special FX"),
                    (SettingsTab::CaptureReplay, "Capture & Replay"),
                    (SettingsTab::AudioMic, "Audio & Mic"),
                    (SettingsTab::Pc98, "PC-98 / PC-88"),
                    (SettingsTab::OcrDict, "OCR & Dict"),
                    (SettingsTab::Twitch, "Twitch"),
                    (SettingsTab::Hotkeys, "Hotkeys"),
                ];

                // Measure natural text widths so we can distribute remaining horizontal space evenly
                // across all tabs, filling the single row edge-to-edge with balanced padding.
                let font_id = egui::FontId::monospace(10.5);
                let text_widths: Vec<f32> = ui.fonts(|f| {
                    tabs.iter()
                        .map(|(_, label)| {
                            f.layout_no_wrap((*label).to_string(), font_id.clone(), egui::Color32::WHITE)
                                .size()
                                .x
                        })
                        .collect()
                });
                let total_text_w: f32 = text_widths.iter().sum();
                let total_spacing = spacing * (tabs.len().saturating_sub(1) as f32);
                let extra_per_tab = ((avail_w - total_text_w - total_spacing) / (tabs.len() as f32)).max(6.0);

                for ((tab, label), text_w) in tabs.into_iter().zip(text_widths.into_iter()) {
                    let is_active = state.ui.active_settings_tab == tab;
                    let btn_w = text_w + extra_per_tab;
                    let tab_btn = if is_active {
                        egui::Button::new(
                            egui::RichText::new(label)
                                .monospace()
                                .size(10.5)
                                .strong()
                                .color(egui::Color32::WHITE),
                        )
                        .fill(egui::Color32::from_rgb(26, 26, 26)) // #1a1a1a
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(84, 136, 184)))
                        .rounding(egui::Rounding::ZERO)
                    } else {
                        egui::Button::new(
                            egui::RichText::new(label)
                                .monospace()
                                .size(10.5)
                                .color(egui::Color32::from_rgb(148, 148, 148)),
                        )
                        .fill(egui::Color32::from_rgb(34, 34, 34)) // #222222
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(56, 56, 56))) // #383838
                        .rounding(egui::Rounding::ZERO)
                    };
                    if ui.add_sized([btn_w, 24.0], tab_btn).clicked() {
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
                SettingsTab::GeometryBezel => {
                    changed |= filters::draw_geometry_bezel_tab(ui, state);
                }
                SettingsTab::SpecialFx => {
                    changed |= filters::draw_special_fx_tab(ui, state);
                }
                SettingsTab::CaptureReplay => {
                    changed |= devices::draw_capture_and_webcam_selectors(ui, state);
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
                SettingsTab::AudioMic => {
                    technical_group(ui, "Game Audio Input & Noise Filter:", |ui| {
                        changed |= devices::draw_audio_device_controls(ui, state);
                        technical_separator(ui);
                        if crate::replay::ui::draw_audio_filter_contents(&mut state.replay, ui) {
                            crate::config::save_config(state);
                            crate::config::save_global_hardware_config(state);
                            let _ = crate::config::save_replay_config(&state.replay.config);
                            changed = true;
                        }
                    });
                    ui.add_space(4.0);
                    changed |= crate::ui::mic::draw_mic_tab(ui, state);
                }
                SettingsTab::Pc98 => {
                    changed |= super::pc98::draw_pc98_tab(ui, state);
                }
                SettingsTab::OcrDict => {
                    draw_ocr_dict_tab(ui, state, &mut changed);
                }
                SettingsTab::Twitch => {
                    changed |= super::twitch::draw_twitch_tab(ui, state);
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

        two_columns(ui, |col1, col2| {
            if col1
                .checkbox(&mut state.ocr.hide_overlay, "Hide OCR boxes overlay")
                .on_hover_text(
                    "Hides the blue bounding boxes and replacement text overlay on the video feed.\nDictionary lookup on hover and popup behavior will continue to work normally.",
                )
                .changed()
            {
                crate::config::save_config(state);
                *changed = true;
            }

            if col2
                .checkbox(
                    &mut state.dict.popup_under_crt,
                    "Show dictionary popup under CRT filters",
                )
                .on_hover_text(
                    "Renders the dictionary lookup window underneath CRT filters, glass reflections, and bezel borders instead of on top of them.\nClamps the popup within the visible CRT viewport so it never cuts into the borders.",
                )
                .changed()
            {
                crate::config::save_config(state);
                *changed = true;
            }
        });

        two_columns(ui, |col1, _col2| {
            if col1
                .checkbox(
                    &mut state.twitch.config.share_mined_words_to_chat,
                    "Share mined words to Twitch chat (when live)",
                )
                .on_hover_text(
                    "When live streaming, automatically posts newly mined words with their sentence and Jitendex definition into Twitch chat.",
                )
                .changed()
            {
                crate::config::save_config(state);
                *changed = true;
            }
        });

        if state.ocr.is_processing.load(Ordering::Relaxed) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Processing OCR with Google Lens...");
            });
        }
    });

    ui.add_space(4.0);

    two_columns(ui, |col1, col2| {
        technical_group(col1, "Japanese Dictionary (Jitendex):", |ui| {
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
                        ui.label(egui::RichText::new("Checking...").weak().small());
                    } else if ui.small_button("Check Updates").clicked() {
                        state.dict.trigger_check_version();
                    }
                });
            }
        });

        technical_group(col2, "Frequency Dictionary (Jiten Global):", |ui| {
            if state.dict.freq_update_available {
                ui.label(
                    egui::RichText::new("(!) An update is available for Jiten Frequency!")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(208, 144, 48)),
                );
            }

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
                        ui.label(egui::RichText::new("Checking...").weak().small());
                    } else if ui.small_button("Check Updates").clicked() {
                        state.dict.trigger_check_freq_version();
                    }
                });
            }
        });
    });
}

fn draw_hotkeys_tab(ui: &mut egui::Ui) {
    technical_group(ui, "Keyboard Shortcuts:", |ui| {
        let shortcuts = [
            ("F", "Toggle Fullscreen"),
            ("M", "Toggle Controls (Settings Window)"),
            ("B", "Toggle Mining Bank Window"),
            ("L", "Toggle Studio Window"),
            ("H", "Toggle Drawing Whiteboard"),
            ("Ctrl + Z", "Undo Drawing (Whiteboard)"),
            ("Ctrl + Y", "Redo Drawing (Whiteboard)"),
            ("C", "Cycle CRT Filter (Off / Lottes / Halo)"),
            ("J", "Toggle Audio-Reactive CRT Interference"),
            ("K", "Toggle Lights Off Night Mode"),
            ("G", "Toggle 480p Pixelate Filter"),
            ("Q", "Stop Stream Confirmation Dialog"),
            ("T", "Hide / Show Twitch Chat Overlay"),
            ("Escape", "Exit Fullscreen / Dismiss Popups"),
            ("P", "Toggle Play / Pause (Video Player)"),
            ("← / →", "Seek -5s / +5s (Video Player)"),
            ("Space", "Capture OCR (Google Lens)"),
            ("Shift + Space", "Clear OCR Bounding Boxes"),
            ("Shift + Tab", "Toggle Gamescope / PC-98 / PC-88 Input"),
            ("Shift + F5", "Quick Save State (PC-98 / PC-88)"),
            ("Shift + F9", "Quick Load State (PC-98 / PC-88)"),
            ("Shift + F6 / F7", "Prev / Next Save Slot (PC-98 / PC-88)"),
            ("F12", "Open / Close NP2 / Quasi88 Menu"),
            ("Ctrl + C", "Export 15s Replay Clip to Clipboard"),
            ("Ctrl + Shift + C", "Export 7s Audio Clip to Clipboard"),
            ("F1 – F12", "Save Replay Buffer (Capture & Replay tab)"),
        ];

        let mid = (shortcuts.len() + 1) / 2;
        let (left_shortcuts, right_shortcuts) = shortcuts.split_at(mid);

        two_columns(ui, |col1, col2| {
            let draw_hotkey_grid = |col: &mut egui::Ui, id: &str, list: &[(&str, &str)]| {
                let col_w = col.available_width();
                egui::Grid::new(id)
                    .striped(true)
                    .num_columns(2)
                    .min_col_width(115.0)
                    .spacing([12.0, 5.0])
                    .show(col, |ui| {
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

                        for (key, desc) in list {
                            ui.add_sized(
                                [115.0, 18.0],
                                egui::Label::new(
                                    egui::RichText::new(*key)
                                        .monospace()
                                        .size(11.0)
                                        .strong()
                                        .color(egui::Color32::WHITE),
                                ),
                            );
                            let action_w = (col_w - 115.0 - 16.0).max(80.0);
                            ui.add_sized(
                                [action_w, 18.0],
                                egui::Label::new(
                                    egui::RichText::new(*desc)
                                        .size(11.0)
                                        .color(egui::Color32::from_rgb(208, 208, 208)),
                                ),
                            );
                            ui.end_row();
                        }
                    });
            };

            draw_hotkey_grid(col1, "hotkeys_grid_left", left_shortcuts);
            draw_hotkey_grid(col2, "hotkeys_grid_right", right_shortcuts);
        });
    });
}
