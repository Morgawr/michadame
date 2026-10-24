use crate::app::AppState;
use crate::config;
use crate::pc88::{Pc88Command, Pc88MediaSlot, RetroPcSystem};
use crate::pc98::{Pc98Command, Pc98MediaSlot};
use crate::ui::controls::{
    percent_slider_range, slider_item, sub_heading, technical_group, technical_separator,
    two_columns,
};
use eframe::egui;

pub fn draw_pc98_tab(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;

    // Top-level Retro PC System Selector (PC-98 vs PC-88)
    egui::Frame::none()
        .fill(egui::Color32::from_rgb(22, 22, 22))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(53, 53, 53)))
        .inner_margin(egui::Margin::symmetric(10.0, 6.0))
        .rounding(egui::Rounding::ZERO)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Retro PC System:")
                        .monospace()
                        .size(11.0)
                        .strong()
                        .color(egui::Color32::from_rgb(224, 224, 224)),
                );
                ui.add_space(6.0);

                for (sys, label, tip) in [
                    (
                        RetroPcSystem::Pc98,
                        "NEC PC-9801 / PC-9821 (NP2kai)",
                        "Configure and launch 16-bit/32-bit NEC PC-98 games (Neko Project II Kai)",
                    ),
                    (
                        RetroPcSystem::Pc88,
                        "NEC PC-8801 (Quasi88)",
                        "Configure and launch 8-bit NEC PC-8801 / PC-8801mkIISR games (Quasi88)",
                    ),
                ] {
                    let is_selected = state.retro_pc_system == sys;
                    let btn = egui::Button::new(
                        egui::RichText::new(label)
                            .monospace()
                            .size(11.0)
                            .strong()
                            .color(if is_selected {
                                egui::Color32::from_rgb(210, 255, 220)
                            } else {
                                egui::Color32::from_rgb(165, 165, 165)
                            }),
                    )
                    .fill(if is_selected {
                        egui::Color32::from_rgb(24, 72, 42)
                    } else {
                        egui::Color32::from_rgb(34, 34, 34)
                    })
                    .stroke(egui::Stroke::new(
                        1.0,
                        if is_selected {
                            egui::Color32::from_rgb(42, 118, 68)
                        } else {
                            egui::Color32::from_rgb(60, 60, 60)
                        },
                    ))
                    .rounding(egui::Rounding::ZERO);

                    if ui.add_sized([240.0, 22.0], btn).on_hover_text(tip).clicked()
                        && state.retro_pc_system != sys
                    {
                        state.retro_pc_system = sys;
                        config::save_config(state);
                        changed = true;
                    }
                }
            });
        });

    ui.add_space(4.0);

    match state.retro_pc_system {
        RetroPcSystem::Pc98 => {
            changed |= draw_pc98_system_section(ui, state);
        }
        RetroPcSystem::Pc88 => {
            changed |= draw_pc88_system_section(ui, state);
        }
    }

    changed
}

fn draw_pc98_system_section(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;

    technical_group(ui, "NEC PC-9801 / PC-9821 Emulator (Neko Project II Kai):", |ui| {
        ui.horizontal(|ui| {
            if state.pc98.is_active {
                let stop_btn = egui::Button::new(
                    egui::RichText::new("⏹ Power Off PC-98")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(255, 204, 204)),
                )
                .fill(egui::Color32::from_rgb(90, 24, 24))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(122, 40, 40)))
                .rounding(egui::Rounding::ZERO);
                if ui.add(stop_btn).clicked() {
                    state.stop_stream(ui.ctx());
                    state.info("PC-98 emulator powered off.");
                    changed = true;
                }

                let reset_btn = egui::Button::new(
                    egui::RichText::new("🔄 Reset PC-98")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(255, 235, 180)),
                )
                .fill(egui::Color32::from_rgb(70, 50, 18))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 88, 30)))
                .rounding(egui::Rounding::ZERO);
                if ui
                    .add(reset_btn)
                    .on_hover_text("Warm-reset the emulated PC-98 (use after swapping boot floppies)")
                    .clicked()
                {
                    state.pc98.send_command(Pc98Command::Reset);
                }

                let menu_btn = egui::Button::new(
                    egui::RichText::new("⚙ NP2 OSD Menu (F12)")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(200, 230, 255)),
                )
                .fill(egui::Color32::from_rgb(24, 52, 82))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(44, 88, 136)))
                .rounding(egui::Rounding::ZERO);
                if ui
                    .add(menu_btn)
                    .on_hover_text("Toggle the internal Neko Project II Kai on-screen menu (rendered inside the CRT)")
                    .clicked()
                {
                    state.pc98.send_command(Pc98Command::TriggerMenu);
                }

                let (icon, label, color) = if state.pc98.passthrough_enabled {
                    ("⌨", "Passthrough Active (Shift+Tab to pause)", egui::Color32::from_rgb(140, 240, 140))
                } else {
                    ("🚫", "Input Paused (Shift+Tab to resume)", egui::Color32::from_rgb(255, 130, 130))
                };
                ui.label(
                    egui::RichText::new(format!("{} {}", icon, label))
                        .monospace()
                        .size(10.5)
                        .color(color),
                );
            } else {
                let boot_btn = egui::Button::new(
                    egui::RichText::new("🖥 Boot PC-98 Monitor")
                        .size(11.0)
                        .strong()
                        .color(egui::Color32::from_rgb(210, 255, 220)),
                )
                .fill(egui::Color32::from_rgb(24, 72, 42))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(42, 118, 68)))
                .rounding(egui::Rounding::ZERO);
                if ui
                    .add(boot_btn)
                    .on_hover_text("Boot the embedded NEC PC-98 computer directly inside Michadame's CRT monitor")
                    .clicked()
                {
                    state.start_pc98_mode(ui.ctx());
                    changed = true;
                }
            }
        });

        if let Some(ref status) = state.pc98.status_message {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    egui::RichText::new(status)
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(251, 191, 36)),
                );
            });
        }
    });

    ui.add_space(4.0);

    technical_group(ui, "PC-98 Game Library & Disk Media:", |ui| {
        draw_game_library_section(ui, state, &mut changed);

        ui.add_space(4.0);
        egui::CollapsingHeader::new(
            egui::RichText::new("Advanced: Manual Disk Drives & Media (FDD / HDD / CD-ROM)")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(170, 170, 170)),
        )
        .id_source("pc98_advanced_manual_drives")
        .default_open(false)
        .show(ui, |ui| {
            draw_media_row(
                ui,
                state,
                "FDD 1 (Floppy A:):",
                Pc98MediaSlot::Fdd1,
                ".d88, .fdi, .hdm, .xdf, .2hd, .dup",
                true,
                &mut changed,
            );
            draw_media_row(
                ui,
                state,
                "FDD 2 (Floppy B:):",
                Pc98MediaSlot::Fdd2,
                ".d88, .fdi, .hdm, .xdf, .2hd, .dup",
                true,
                &mut changed,
            );

            technical_separator(ui);

            draw_media_row(
                ui,
                state,
                "HDD 1 (SASI/IDE #0):",
                Pc98MediaSlot::Hdd1,
                ".hdi, .nhd, .thd, .vhd, .slh",
                false,
                &mut changed,
            );
            draw_media_row(
                ui,
                state,
                "HDD 2 (SASI/IDE #1):",
                Pc98MediaSlot::Hdd2,
                ".hdi, .nhd, .thd, .vhd, .slh",
                false,
                &mut changed,
            );
            draw_media_row(
                ui,
                state,
                "CD-ROM (ISO/CUE):",
                Pc98MediaSlot::CdRom,
                ".cue, .ccd, .iso, .mds",
                false,
                &mut changed,
            );
        });
    });

    ui.add_space(4.0);

    technical_group(ui, "", |ui| {
        draw_save_states_section(ui, state, RetroPcSystem::Pc98, &mut changed);
    });

    ui.add_space(4.0);

    technical_group(ui, "PC-98 Hardware Configuration (Applies on Next Boot):", |ui| {
        two_columns(ui, |col1, col2| {
            sub_heading(col1, "System Architecture:");

            col1.horizontal(|ui| {
                ui.add_sized(
                    [130.0, 18.0],
                    egui::Label::new(
                        egui::RichText::new("PC-98 Model:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    ),
                );
                let models = [
                    ("PC-9801VX", "PC-9801VX (80286 / Standard)"),
                    ("PC-9801VM", "PC-9801VM (V30)"),
                    ("PC-286", "EPSON PC-286"),
                ];
                let current_label = models
                    .iter()
                    .find(|(v, _)| *v == state.pc98.config.model)
                    .map(|(_, l)| *l)
                    .unwrap_or(&state.pc98.config.model);
                egui::ComboBox::from_id_source("pc98_model_combo")
                    .selected_text(current_label)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for (val, label) in models {
                            if ui
                                .selectable_value(
                                    &mut state.pc98.config.model,
                                    val.to_string(),
                                    label,
                                )
                                .changed()
                            {
                                config::save_config(state);
                                changed = true;
                            }
                        }
                    });
            });

            col1.horizontal(|ui| {
                ui.add_sized(
                    [130.0, 18.0],
                    egui::Label::new(
                        egui::RichText::new("Base Clock:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    ),
                );
                let clocks = [
                    ("2.4576 MHz", "2.4576 MHz (Standard)"),
                    ("1.9968 MHz", "1.9968 MHz (5MHz/10MHz series)"),
                ];
                egui::ComboBox::from_id_source("pc98_base_clock")
                    .selected_text(&state.pc98.config.base_clock)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for (val, label) in clocks {
                            if ui
                                .selectable_value(
                                    &mut state.pc98.config.base_clock,
                                    val.to_string(),
                                    label,
                                )
                                .changed()
                            {
                                config::save_config(state);
                                changed = true;
                            }
                        }
                    });
            });

            col1.horizontal(|ui| {
                ui.add_sized(
                    [130.0, 18.0],
                    egui::Label::new(
                        egui::RichText::new("Clock Multiplier:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    ),
                );
                let mults: &[(u32, &str)] = &[
                    (2, "2x (~5 MHz)"),
                    (4, "4x (~10 MHz)"),
                    (5, "5x (~12 MHz)"),
                    (8, "8x (~20 MHz - 386)"),
                    (12, "12x (~30 MHz - 486)"),
                    (16, "16x (~40 MHz - 486DX)"),
                    (20, "20x (~50 MHz - Pentium)"),
                    (32, "32x (~78 MHz)"),
                    (42, "42x (~103 MHz)"),
                ];
                let current_mult_label = mults
                    .iter()
                    .find(|(m, _)| *m == state.pc98.config.clock_mult)
                    .map(|(_, l)| *l)
                    .unwrap_or("16x");
                egui::ComboBox::from_id_source("pc98_clock_mult")
                    .selected_text(current_mult_label)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for (val, label) in mults {
                            if ui
                                .selectable_value(&mut state.pc98.config.clock_mult, *val, *label)
                                .changed()
                            {
                                config::save_config(state);
                                changed = true;
                            }
                        }
                    });
            });

            col1.horizontal(|ui| {
                ui.add_sized(
                    [130.0, 18.0],
                    egui::Label::new(
                        egui::RichText::new("RAM Size:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    ),
                );
                let rams: &[(u32, &str)] = &[
                    (1, "1 MB (640KB + EMS)"),
                    (3, "3 MB"),
                    (7, "7 MB"),
                    (13, "13 MB"),
                    (16, "16 MB (Standard Extended)"),
                    (32, "32 MB"),
                    (64, "64 MB"),
                    (128, "128 MB"),
                ];
                let current_ram_label = rams
                    .iter()
                    .find(|(r, _)| *r == state.pc98.config.ram_mb)
                    .map(|(_, l)| *l)
                    .unwrap_or("16 MB (Standard Extended)");
                egui::ComboBox::from_id_source("pc98_ram_size")
                    .selected_text(current_ram_label)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for (val, label) in rams {
                            if ui
                                .selectable_value(&mut state.pc98.config.ram_mb, *val, *label)
                                .changed()
                            {
                                config::save_config(state);
                                changed = true;
                            }
                        }
                    });
            });

            sub_heading(col2, "Audio & Peripherals:");

            col2.horizontal(|ui| {
                ui.add_sized(
                    [130.0, 18.0],
                    egui::Label::new(
                        egui::RichText::new("Sound Board:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    ),
                );
                let boards = [
                    ("PC9801-86", "PC-9801-86 (YM2608 OPNA + PCM)"),
                    ("PC9801-26K + 86", "PC-9801-26K + 86"),
                    ("PC9801-26K", "PC-9801-26K (YM2203 OPN)"),
                    ("Speak board", "Speak Board (YM2608)"),
                    ("Spark board", "Spark Board"),
                    ("Sound Orchestra", "Sound Orchestra (YM2203 + OPL2)"),
                    ("PC9801-118", "PC-9801-118 (CS4231 + OPL3)"),
                    ("None", "None"),
                ];
                let current_board_label = boards
                    .iter()
                    .find(|(b, _)| *b == state.pc98.config.sound_board)
                    .map(|(_, l)| *l)
                    .unwrap_or(&state.pc98.config.sound_board);
                egui::ComboBox::from_id_source("pc98_sound_board")
                    .selected_text(current_board_label)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for (val, label) in boards {
                            if ui
                                .selectable_value(
                                    &mut state.pc98.config.sound_board,
                                    val.to_string(),
                                    label,
                                )
                                .changed()
                            {
                                config::save_config(state);
                                changed = true;
                            }
                        }
                    });
            });

            col2.horizontal(|ui| {
                ui.add_sized(
                    [130.0, 18.0],
                    egui::Label::new(
                        egui::RichText::new("GDC Renderer:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    ),
                );
                let gdcs = [
                    ("uPD7220", "uPD7220 (Standard)"),
                    ("uPD72020", "uPD72020 (Enhanced)"),
                ];
                egui::ComboBox::from_id_source("pc98_gdc")
                    .selected_text(&state.pc98.config.gdc_type)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for (val, label) in gdcs {
                            if ui
                                .selectable_value(
                                    &mut state.pc98.config.gdc_type,
                                    val.to_string(),
                                    label,
                                )
                                .changed()
                            {
                                config::save_config(state);
                                changed = true;
                            }
                        }
                    });
            });

            if col2
                .checkbox(
                    &mut state.pc98.config.floppy_seek_sound,
                    "Mechanical Floppy Drive Seek Sound",
                )
                .on_hover_text("Emulates the authentic mechanical stepper motor sounds of the PC-98 5.25\" / 3.5\" floppy drives (applies immediately)")
                .changed()
            {
                if state.pc98.is_active {
                    state
                        .pc98
                        .send_command(Pc98Command::UpdateOptions(state.pc98.config.clone()));
                }
                config::save_config(state);
                changed = true;
            }

            if col2
                .checkbox(
                    &mut state.pc98.config.fast_memcheck,
                    "Fast BIOS Memory Check on Boot",
                )
                .on_hover_text("Skips the slow RAM count-up during the PC-98 power-on self-test")
                .changed()
            {
                config::save_config(state);
                changed = true;
            }

            if col2
                .checkbox(
                    &mut state.pc98.config.numpad_arrow_keys,
                    "Arrow Keys Send PC-98 Numpad (2/4/6/8)",
                )
                .on_hover_text("Sends PC-98 Tenkey 8/2/4/6 scan codes when pressing Arrow keys, required for directional movement in most PC-98 games (applies immediately)")
                .changed()
            {
                state.pc98.release_all_inputs();
                config::save_config(state);
                changed = true;
            }

            if slider_item(
                col2,
                "Mouse Sensitivity:",
                percent_slider_range(&mut state.pc98.config.mouse_sensitivity, 0.05..=2.0, 2.0),
                Some("Scales PC-98 bus mouse speed (applies immediately while running)"),
            )
            .changed()
            {
                config::save_config(state);
                changed = true;
            }
        });
    });

    changed
}

fn draw_game_library_section(ui: &mut egui::Ui, state: &mut AppState, changed: &mut bool) {
    // 1. Root Games Folder row
    ui.horizontal(|ui| {
        ui.add_sized(
            [140.0, 20.0],
            egui::Label::new(
                egui::RichText::new("Games Root Folder:")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            ),
        );

        let avail = (ui.available_width() - 165.0).max(120.0);
        if ui
            .add_sized(
                [avail, 20.0],
                egui::TextEdit::singleline(&mut state.pc98.config.games_dir)
                    .hint_text("/path/to/PC98/GameList"),
            )
            .changed()
        {
            config::save_config(state);
            *changed = true;
        }

        if ui.button("Browse...").clicked() {
            state.open_pc98_media_dialog(Pc98MediaSlot::GamesDir, ui.ctx());
        }

        let can_refresh = !state.pc98.config.games_dir.trim().is_empty();
        if ui
            .add_enabled(can_refresh, egui::Button::new("🔄 Refresh"))
            .on_hover_text("Scan the root games directory and all subdirectories for PC-98 games")
            .clicked()
        {
            match state.pc98.refresh_games_list() {
                Ok(count) => {
                    state.info(format!("Scanned PC-98 games folder: found {count} games."));
                }
                Err(e) => {
                    state.error(format!("Failed to scan PC-98 games folder: {e}"));
                }
            }
            config::save_config(state);
            *changed = true;
        }
    });

    // 2. Base Bootable DOS Drive row
    ui.horizontal(|ui| {
        ui.add_sized(
            [140.0, 20.0],
            egui::Label::new(
                egui::RichText::new("Base DOS HDD (A:):")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            ),
        );

        let avail = (ui.available_width() - 135.0).max(120.0);
        if ui
            .add_sized(
                [avail, 20.0],
                egui::TextEdit::singleline(&mut state.pc98.config.dos_hdd_path)
                    .hint_text("Master_MSDOS62.hdi (used for non-bootable DOS games)"),
            )
            .changed()
        {
            if let Some(sel_idx) = state
                .pc98
                .games_list
                .iter()
                .position(|g| g.id == state.pc98.config.selected_game)
            {
                if state.pc98.games_list[sel_idx].boot_mode.requires_dos_hdd() {
                    state.pc98.config.hdd1_path =
                        state.pc98.config.dos_hdd_path.trim().to_string();
                }
            }
            config::save_config(state);
            *changed = true;
        }

        if ui.button("Browse...").clicked() {
            state.open_pc98_media_dialog(Pc98MediaSlot::DosHdd, ui.ctx());
        }

        let has_dos = !state.pc98.config.dos_hdd_path.is_empty();
        if ui
            .add_enabled(has_dos, egui::Button::new("Clear"))
            .clicked()
        {
            state.pc98.config.dos_hdd_path.clear();
            config::save_config(state);
            *changed = true;
        }
    });

    technical_separator(ui);

    // 3. Game selection dropdown + search filter + quick launch button
    let total_games = state.pc98.games_list.len();
    let has_active_filter = !state.pc98.game_filter.trim().is_empty();
    let filtered_indices = if total_games > 0 {
        crate::pc98::search::filter_and_rank_games(
            &state.pc98.games_list,
            &state.pc98.game_filter,
        )
    } else {
        Vec::new()
    };

    let selected_idx = state
        .pc98
        .games_list
        .iter()
        .position(|g| g.id == state.pc98.config.selected_game);

    let mut newly_picked_idx: Option<usize> = None;

    ui.horizontal(|ui| {
        let label_text = if total_games > 0 {
            if has_active_filter {
                format!("Select Game ({}/{total_games}):", filtered_indices.len())
            } else {
                format!("Select Game ({total_games}):")
            }
        } else {
            "Select Game:".to_string()
        };
        ui.add_sized(
            [140.0, 20.0],
            egui::Label::new(
                egui::RichText::new(label_text)
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            ),
        );

        if total_games == 0 {
            ui.label(
                egui::RichText::new(
                    "No games scanned yet — set Games Root Folder above and click 🔄 Refresh.",
                )
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(136, 136, 136)),
            );
        } else {
            let filter_resp = ui
                .add_sized(
                    [155.0, 20.0],
                    egui::TextEdit::singleline(&mut state.pc98.game_filter)
                        .hint_text("🔎 Fuzzy / Romaji / JP..."),
                )
                .on_hover_text(
                    "Search by Japanese (Kanji/Kana), Romaji (e.g. 'ukiuki', 'dokyusei', 'touhou'), English ('island', 'wizardry 6'), or fuzzy abbreviation ('wiz6'). Press Enter to select top match.",
                );
            if filter_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if let Some(&top_idx) = filtered_indices.first() {
                    newly_picked_idx = Some(top_idx);
                }
            }
            if !state.pc98.game_filter.is_empty() && ui.small_button("✕").clicked() {
                state.pc98.game_filter.clear();
            }

            let full_selected_text = selected_idx
                .map(|i| state.pc98.games_list[i].display_label())
                .unwrap_or_else(|| "Choose a PC-98 game to load...".to_string());

            let combo_width = (ui.available_width() - 140.0).clamp(120.0, 320.0);
            let max_cols = ((combo_width / 8.5) as usize).clamp(14, 36);
            let selected_text = truncate_combo_label(&full_selected_text, max_cols);

            egui::ComboBox::from_id_source("pc98_game_selector_combo")
                .selected_text(selected_text)
                .width(combo_width)
                .show_ui(ui, |ui| {
                    if filtered_indices.is_empty() {
                        ui.label(
                            egui::RichText::new("No matching games found.")
                                .monospace()
                                .size(10.5)
                                .color(egui::Color32::from_rgb(180, 120, 120)),
                        );
                    } else {
                        for &idx in &filtered_indices {
                            let game = &state.pc98.games_list[idx];
                            let is_selected = selected_idx == Some(idx);
                            if ui
                                .selectable_label(is_selected, game.display_label())
                                .clicked()
                            {
                                newly_picked_idx = Some(idx);
                            }
                        }
                    }
                })
                .response
                .on_hover_text(&full_selected_text);

            let launch_label = if state.pc98.is_active {
                "🔄 Reboot Game"
            } else {
                "▶ Launch Game"
            };
            let launch_btn = egui::Button::new(
                egui::RichText::new(launch_label)
                    .size(11.0)
                    .strong()
                    .color(egui::Color32::from_rgb(210, 255, 220)),
            )
            .fill(egui::Color32::from_rgb(24, 72, 42))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(42, 118, 68)))
            .rounding(egui::Rounding::ZERO);

            if ui
                .add_enabled(selected_idx.is_some(), launch_btn)
                .on_hover_text("Boot (or restart) the PC-98 emulator with the selected game")
                .clicked()
            {
                if let Some(idx) = selected_idx {
                    let _ = state.pc98.select_game_by_index(idx);
                    config::save_config(state);
                }
                state.start_pc98_mode(ui.ctx());
                *changed = true;
            }
        }
    });

    // Live top fuzzy matches bar when filter text is entered
    if total_games > 0 && has_active_filter {
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.add_sized(
                [140.0, 18.0],
                egui::Label::new(
                    egui::RichText::new("Top Fuzzy Matches:")
                        .monospace()
                        .size(10.5)
                        .color(egui::Color32::from_rgb(153, 153, 153)),
                ),
            );
            if filtered_indices.is_empty() {
                ui.label(
                    egui::RichText::new(format!(
                        "No games match '{}'",
                        state.pc98.game_filter.trim()
                    ))
                    .monospace()
                    .size(10.5)
                    .color(egui::Color32::from_rgb(220, 130, 130)),
                );
            } else {
                for &idx in filtered_indices.iter().take(5) {
                    let game = &state.pc98.games_list[idx];
                    let is_sel = selected_idx == Some(idx);
                    let short_title = truncate_combo_label(&game.title, 22);
                    let btn = egui::Button::new(
                        egui::RichText::new(short_title)
                            .monospace()
                            .size(10.5)
                            .color(if is_sel {
                                egui::Color32::from_rgb(220, 255, 220)
                            } else {
                                egui::Color32::from_rgb(200, 220, 245)
                            }),
                    )
                    .fill(if is_sel {
                        egui::Color32::from_rgb(28, 74, 44)
                    } else {
                        egui::Color32::from_rgb(30, 38, 52)
                    })
                    .stroke(egui::Stroke::new(
                        1.0,
                        if is_sel {
                            egui::Color32::from_rgb(56, 138, 84)
                        } else {
                            egui::Color32::from_rgb(58, 76, 104)
                        },
                    ))
                    .rounding(egui::Rounding::ZERO);

                    if ui
                        .add(btn)
                        .on_hover_text(game.display_label())
                        .clicked()
                    {
                        newly_picked_idx = Some(idx);
                    }
                }
                if filtered_indices.len() > 5 {
                    ui.label(
                        egui::RichText::new(format!("(+{} more in dropdown)", filtered_indices.len() - 5))
                            .monospace()
                            .size(10.0)
                            .color(egui::Color32::from_rgb(136, 136, 136)),
                    );
                }
            }
        });
    }

    if let Some(idx) = newly_picked_idx {
        match state.pc98.select_game_by_index(idx) {
            Ok(title) => {
                state.info(format!("Loaded PC-98 game media for '{title}'."));
            }
            Err(e) => {
                state.error(format!("Failed to configure game media: {e}"));
            }
        }
        config::save_config(state);
        if state.pc98.is_active {
            state.start_pc98_mode(ui.ctx());
        }
        *changed = true;
    }

    // 4. Active game drive status + multi-disk floppy quick swapper
    if let Some(idx) = selected_idx {
        let game = state.pc98.games_list[idx].clone();

        if game.boot_mode.requires_dos_hdd() && state.pc98.config.dos_hdd_path.trim().is_empty() {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(
                    "⚠ This game requires a Base Bootable DOS Drive (e.g. Master_MSDOS62.hdi). Please set Base DOS HDD above.",
                )
                .monospace()
                .size(10.5)
                .color(egui::Color32::from_rgb(251, 191, 36)),
            );
        }

        // Concise summary of mounted media
        let mut mounted_parts = Vec::new();
        for (slot_name, p_str) in [
            ("HDD1", state.pc98.config.hdd1_path.trim()),
            ("HDD2", state.pc98.config.hdd2_path.trim()),
            ("FDD1", state.pc98.config.fdd1_path.trim()),
            ("FDD2", state.pc98.config.fdd2_path.trim()),
            ("CD", state.pc98.config.cdrom_path.trim()),
        ] {
            if !p_str.is_empty() {
                let short = std::path::Path::new(p_str)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(p_str);
                mounted_parts.push(format!("{slot_name}: {short}"));
            }
        }
        if !mounted_parts.is_empty() {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(format!(
                    "Mounted Media  ->  {}",
                    mounted_parts.join("   |   ")
                ))
                .monospace()
                .size(10.5)
                .color(egui::Color32::from_rgb(130, 200, 150)),
            );
        }

        // Quick floppy swapper when the selected game has multiple floppy disks
        if game.floppies.len() > 1 {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_sized(
                    [140.0, 20.0],
                    egui::Label::new(
                        egui::RichText::new("Quick Disk Swap:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    ),
                );

                for (drive_idx, slot_label) in [(0u8, "FDD 1:"), (1u8, "FDD 2:")] {
                    ui.label(
                        egui::RichText::new(slot_label)
                            .monospace()
                            .size(10.5)
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    );
                    let cur_path = if drive_idx == 0 {
                        state.pc98.config.fdd1_path.clone()
                    } else {
                        state.pc98.config.fdd2_path.clone()
                    };
                    let cur_name = std::path::Path::new(&cur_path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("(Empty)");

                    egui::ComboBox::from_id_source(format!("pc98_quick_fdd_swap_{drive_idx}"))
                        .selected_text(truncate_combo_label(cur_name, 20))
                        .width(175.0)
                        .show_ui(ui, |ui| {
                            for disk_path in &game.floppies {
                                let disk_str = disk_path.to_string_lossy().to_string();
                                let disk_name = disk_path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or(&disk_str);
                                if ui
                                    .selectable_label(cur_path == disk_str, disk_name)
                                    .clicked()
                                {
                                    if drive_idx == 0 {
                                        state.pc98.config.fdd1_path = disk_str;
                                    } else {
                                        state.pc98.config.fdd2_path = disk_str;
                                    }
                                    if state.pc98.is_active {
                                        state.pc98.send_command(Pc98Command::MountFdd {
                                            drive: drive_idx,
                                            path: Some(disk_path.clone()),
                                        });
                                    }
                                    config::save_config(state);
                                    *changed = true;
                                }
                            }
                        });
                    ui.add_space(8.0);
                }
            });
        }
    }
}

fn draw_media_row(
    ui: &mut egui::Ui,
    state: &mut AppState,
    label: &str,
    slot: Pc98MediaSlot,
    hint: &str,
    live_swappable: bool,
    changed: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [140.0, 20.0],
            egui::Label::new(
                egui::RichText::new(label)
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            ),
        );

        let path_ref = match slot {
            Pc98MediaSlot::Fdd1 => &mut state.pc98.config.fdd1_path,
            Pc98MediaSlot::Fdd2 => &mut state.pc98.config.fdd2_path,
            Pc98MediaSlot::Hdd1 => &mut state.pc98.config.hdd1_path,
            Pc98MediaSlot::Hdd2 => &mut state.pc98.config.hdd2_path,
            Pc98MediaSlot::CdRom => &mut state.pc98.config.cdrom_path,
            Pc98MediaSlot::GamesDir => &mut state.pc98.config.games_dir,
            Pc98MediaSlot::DosHdd => &mut state.pc98.config.dos_hdd_path,
        };

        let avail = (ui.available_width() - 135.0).max(120.0);
        let text_resp = ui.add_sized(
            [avail, 20.0],
            egui::TextEdit::singleline(path_ref).hint_text(hint),
        );
        if text_resp.changed() {
            if live_swappable && state.pc98.is_active {
                let drive = if slot == Pc98MediaSlot::Fdd1 { 0 } else { 1 };
                let p = if path_ref.trim().is_empty() {
                    None
                } else {
                    Some(std::path::PathBuf::from(path_ref.trim()))
                };
                state.pc98.send_command(Pc98Command::MountFdd { drive, path: p });
            }
            config::save_config(state);
            *changed = true;
        }

        if ui.button("Browse...").clicked() {
            state.open_pc98_media_dialog(slot, ui.ctx());
        }

        let has_media = match slot {
            Pc98MediaSlot::Fdd1 => !state.pc98.config.fdd1_path.is_empty(),
            Pc98MediaSlot::Fdd2 => !state.pc98.config.fdd2_path.is_empty(),
            Pc98MediaSlot::Hdd1 => !state.pc98.config.hdd1_path.is_empty(),
            Pc98MediaSlot::Hdd2 => !state.pc98.config.hdd2_path.is_empty(),
            Pc98MediaSlot::CdRom => !state.pc98.config.cdrom_path.is_empty(),
            Pc98MediaSlot::GamesDir => !state.pc98.config.games_dir.is_empty(),
            Pc98MediaSlot::DosHdd => !state.pc98.config.dos_hdd_path.is_empty(),
        };

        if ui
            .add_enabled(has_media, egui::Button::new("Eject"))
            .clicked()
        {
            match slot {
                Pc98MediaSlot::Fdd1 => {
                    state.pc98.config.fdd1_path.clear();
                    state.pc98.send_command(Pc98Command::MountFdd {
                        drive: 0,
                        path: None,
                    });
                }
                Pc98MediaSlot::Fdd2 => {
                    state.pc98.config.fdd2_path.clear();
                    state.pc98.send_command(Pc98Command::MountFdd {
                        drive: 1,
                        path: None,
                    });
                }
                Pc98MediaSlot::Hdd1 => state.pc98.config.hdd1_path.clear(),
                Pc98MediaSlot::Hdd2 => state.pc98.config.hdd2_path.clear(),
                Pc98MediaSlot::CdRom => state.pc98.config.cdrom_path.clear(),
                Pc98MediaSlot::GamesDir => state.pc98.config.games_dir.clear(),
                Pc98MediaSlot::DosHdd => state.pc98.config.dos_hdd_path.clear(),
            }
            config::save_config(state);
            *changed = true;
        }
    });
}

fn truncate_combo_label(s: &str, max_cols: usize) -> String {
    let mut cols = 0usize;
    let mut out = String::new();
    for ch in s.chars() {
        let w = if ch.is_ascii() { 1 } else { 2 };
        if cols + w > max_cols {
            out.push('…');
            return out;
        }
        cols += w;
        out.push(ch);
    }
    out
}

fn draw_pc88_system_section(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;

    technical_group(ui, "NEC PC-8801 Emulator (Quasi88):", |ui| {
        ui.horizontal(|ui| {
            if state.pc88.is_active {
                let stop_btn = egui::Button::new(
                    egui::RichText::new("⏹ Power Off PC-88")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(255, 204, 204)),
                )
                .fill(egui::Color32::from_rgb(90, 24, 24))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(122, 40, 40)))
                .rounding(egui::Rounding::ZERO);
                if ui.add(stop_btn).clicked() {
                    state.stop_stream(ui.ctx());
                    state.info("PC-88 emulator powered off.");
                    changed = true;
                }

                let reset_btn = egui::Button::new(
                    egui::RichText::new("🔄 Reset PC-88")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(255, 235, 180)),
                )
                .fill(egui::Color32::from_rgb(70, 50, 18))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 88, 30)))
                .rounding(egui::Rounding::ZERO);
                if ui
                    .add(reset_btn)
                    .on_hover_text("Warm-reset the emulated PC-88 (use after swapping boot floppies)")
                    .clicked()
                {
                    state.pc88.send_command(Pc88Command::Reset);
                }

                let (icon, label, color) = if state.pc88.passthrough_enabled {
                    (
                        "⌨",
                        "Passthrough Active (Shift+Tab to pause)",
                        egui::Color32::from_rgb(140, 240, 140),
                    )
                } else {
                    (
                        "🚫",
                        "Input Paused (Shift+Tab to resume)",
                        egui::Color32::from_rgb(255, 130, 130),
                    )
                };
                ui.label(
                    egui::RichText::new(format!("{} {}", icon, label))
                        .monospace()
                        .size(10.5)
                        .color(color),
                );
            } else {
                let boot_btn = egui::Button::new(
                    egui::RichText::new("🖥 Boot PC-88 Monitor")
                        .size(11.0)
                        .strong()
                        .color(egui::Color32::from_rgb(210, 255, 220)),
                )
                .fill(egui::Color32::from_rgb(24, 72, 42))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(42, 118, 68)))
                .rounding(egui::Rounding::ZERO);
                if ui
                    .add(boot_btn)
                    .on_hover_text("Boot the embedded NEC PC-88 computer directly inside Michadame's CRT monitor")
                    .clicked()
                {
                    state.start_pc88_mode(ui.ctx());
                    changed = true;
                }
            }
        });

        if let Some(ref status) = state.pc88.status_message {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    egui::RichText::new(status)
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(251, 191, 36)),
                );
            });
        }
    });

    ui.add_space(4.0);

    technical_group(ui, "PC-88 Game Library & Disk Media:", |ui| {
        draw_pc88_game_library_section(ui, state, &mut changed);

        ui.add_space(4.0);
        egui::CollapsingHeader::new(
            egui::RichText::new("Advanced: Manual Disk & Tape Drives (FDD / CMT)")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(170, 170, 170)),
        )
        .id_source("pc88_advanced_manual_drives")
        .default_open(false)
        .show(ui, |ui| {
            draw_pc88_media_row(
                ui,
                state,
                "FDD 1 (Drive 1:):",
                Pc88MediaSlot::Fdd1,
                ".d88, .88d, .2d",
                true,
                &mut changed,
            );
            draw_pc88_media_row(
                ui,
                state,
                "FDD 2 (Drive 2:):",
                Pc88MediaSlot::Fdd2,
                ".d88, .88d, .2d",
                true,
                &mut changed,
            );

            technical_separator(ui);

            draw_pc88_media_row(
                ui,
                state,
                "Cassette Tape (CMT):",
                Pc88MediaSlot::Tape,
                ".t88, .cmt",
                false,
                &mut changed,
            );
        });
    });

    ui.add_space(4.0);

    technical_group(ui, "", |ui| {
        draw_save_states_section(ui, state, RetroPcSystem::Pc88, &mut changed);
    });

    ui.add_space(4.0);

    technical_group(ui, "PC-88 Hardware Configuration (Applies on Next Boot):", |ui| {
        two_columns(ui, |col1, col2| {
            sub_heading(col1, "System Architecture:");

            col1.horizontal(|ui| {
                ui.add_sized(
                    [130.0, 18.0],
                    egui::Label::new(
                        egui::RichText::new("BASIC Mode:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    ),
                );
                let modes = [
                    ("N88 V2", "N88-BASIC V2 (PC-8801mkIISR/FR/MR)"),
                    ("N88 V1H", "N88-BASIC V1H (High-Speed V1)"),
                    ("N88 V1S", "N88-BASIC V1S (Standard V1)"),
                    ("N", "N-BASIC (PC-8001 Mode)"),
                ];
                let current_label = modes
                    .iter()
                    .find(|(v, _)| *v == state.pc88.config.basic_mode)
                    .map(|(_, l)| *l)
                    .unwrap_or(&state.pc88.config.basic_mode);
                egui::ComboBox::from_id_source("pc88_basic_mode_combo")
                    .selected_text(current_label)
                    .width(210.0)
                    .show_ui(ui, |ui| {
                        for (val, label) in modes {
                            if ui
                                .selectable_value(
                                    &mut state.pc88.config.basic_mode,
                                    val.to_string(),
                                    label,
                                )
                                .changed()
                            {
                                config::save_config(state);
                                changed = true;
                            }
                        }
                    });
            });

            col1.horizontal(|ui| {
                ui.add_sized(
                    [130.0, 18.0],
                    egui::Label::new(
                        egui::RichText::new("CPU Clock:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    ),
                );
                let clocks = [
                    ("4", "4 MHz (Standard Z80A)"),
                    ("8", "8 MHz (FE / FH / MH / MA)"),
                    ("16", "16 MHz (Overclock)"),
                    ("32", "32 MHz (Turbo)"),
                    ("64", "64 MHz (Max Turbo)"),
                    ("2", "2 MHz (Underclock)"),
                    ("1", "1 MHz (Slow)"),
                ];
                let current_clock_label = clocks
                    .iter()
                    .find(|(v, _)| *v == state.pc88.config.cpu_clock)
                    .map(|(_, l)| *l)
                    .unwrap_or(&state.pc88.config.cpu_clock);
                egui::ComboBox::from_id_source("pc88_cpu_clock")
                    .selected_text(current_clock_label)
                    .width(210.0)
                    .show_ui(ui, |ui| {
                        for (val, label) in clocks {
                            if ui
                                .selectable_value(
                                    &mut state.pc88.config.cpu_clock,
                                    val.to_string(),
                                    label,
                                )
                                .changed()
                            {
                                config::save_config(state);
                                changed = true;
                            }
                        }
                    });
            });

            col1.horizontal(|ui| {
                ui.add_sized(
                    [130.0, 18.0],
                    egui::Label::new(
                        egui::RichText::new("Sub-CPU Mode:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    ),
                );
                let sub_modes: &[(u32, &str)] = &[
                    (0, "0: Run During Disk Access"),
                    (1, "1: Sync Both CPUs During Disk"),
                    (2, "2: Always Run Both CPUs"),
                ];
                let current_sub_label = sub_modes
                    .iter()
                    .find(|(m, _)| *m == state.pc88.config.sub_cpu_mode)
                    .map(|(_, l)| *l)
                    .unwrap_or("0: Run During Disk Access");
                egui::ComboBox::from_id_source("pc88_sub_cpu_mode")
                    .selected_text(current_sub_label)
                    .width(210.0)
                    .show_ui(ui, |ui| {
                        for (val, label) in sub_modes {
                            if ui
                                .selectable_value(&mut state.pc88.config.sub_cpu_mode, *val, *label)
                                .changed()
                            {
                                config::save_config(state);
                                changed = true;
                            }
                        }
                    });
            });

            col1.horizontal(|ui| {
                ui.add_sized(
                    [130.0, 18.0],
                    egui::Label::new(
                        egui::RichText::new("Screen Size:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    ),
                );
                let sizes = [
                    ("full", "Full (640x400 Standard)"),
                    ("half", "Half (320x200 Low-Res)"),
                ];
                let current_size_label = sizes
                    .iter()
                    .find(|(s, _)| *s == state.pc88.config.screen_size)
                    .map(|(_, l)| *l)
                    .unwrap_or("Full (640x400 Standard)");
                egui::ComboBox::from_id_source("pc88_screen_size")
                    .selected_text(current_size_label)
                    .width(210.0)
                    .show_ui(ui, |ui| {
                        for (val, label) in sizes {
                            if ui
                                .selectable_value(
                                    &mut state.pc88.config.screen_size,
                                    val.to_string(),
                                    label,
                                )
                                .changed()
                            {
                                config::save_config(state);
                                changed = true;
                            }
                        }
                    });
            });

            sub_heading(col2, "Audio & Peripherals:");

            col2.horizontal(|ui| {
                ui.add_sized(
                    [130.0, 18.0],
                    egui::Label::new(
                        egui::RichText::new("Sound Board:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    ),
                );
                let boards = [
                    ("OPNA", "Sound Board II (YM2608 OPNA)"),
                    ("OPN", "Sound Board (YM2203 OPN)"),
                ];
                let current_board_label = boards
                    .iter()
                    .find(|(b, _)| *b == state.pc88.config.sound_board)
                    .map(|(_, l)| *l)
                    .unwrap_or(&state.pc88.config.sound_board);
                egui::ComboBox::from_id_source("pc88_sound_board")
                    .selected_text(current_board_label)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for (val, label) in boards {
                            if ui
                                .selectable_value(
                                    &mut state.pc88.config.sound_board,
                                    val.to_string(),
                                    label,
                                )
                                .changed()
                            {
                                config::save_config(state);
                                changed = true;
                            }
                        }
                    });
            });

            if col2
                .checkbox(
                    &mut state.pc88.config.fdc_wait,
                    "Accurate Floppy Drive Timing (FDC Wait)",
                )
                .on_hover_text("Emulates realistic floppy disk controller timing required by timing-sensitive PC-88 games")
                .changed()
            {
                config::save_config(state);
                changed = true;
            }

            if col2
                .checkbox(
                    &mut state.pc88.config.floppy_seek_sound,
                    "Mechanical Floppy Drive Seek Sound",
                )
                .on_hover_text("Emulates the authentic mechanical stepper motor sounds of the PC-88 5.25\" floppy drives (applies immediately)")
                .changed()
            {
                if state.pc88.is_active {
                    state
                        .pc88
                        .send_command(Pc88Command::UpdateOptions(state.pc88.config.clone()));
                }
                config::save_config(state);
                changed = true;
            }

            if col2
                .checkbox(
                    &mut state.pc88.config.pcg_8100,
                    "PCG-8100 Programmable Character Generator",
                )
                .on_hover_text("Enables HAL Laboratory PCG-8100 sound/character hardware for early PC-8001/PC-8801 titles")
                .changed()
            {
                config::save_config(state);
                changed = true;
            }

            if col2
                .checkbox(
                    &mut state.pc88.config.save_to_disk_image,
                    "Persist Floppy Disk Writes to Disk Image",
                )
                .on_hover_text("Writes in-game floppy saves back to the .d88 disk image file")
                .changed()
            {
                config::save_config(state);
                changed = true;
            }

            if col2
                .checkbox(
                    &mut state.pc88.config.numpad_arrow_keys,
                    "Arrow Keys Send PC-88 Numpad (2/4/6/8)",
                )
                .on_hover_text("Sends PC-88 Tenkey 8/2/4/6 scan codes when pressing Arrow keys, required for directional movement in many PC-88 games (applies immediately)")
                .changed()
            {
                state.pc88.release_all_inputs();
                config::save_config(state);
                changed = true;
            }

            if slider_item(
                col2,
                "Mouse Sensitivity:",
                percent_slider_range(&mut state.pc88.config.mouse_sensitivity, 0.05..=2.0, 2.0),
                Some("Scales PC-88 mouse speed (applies immediately while running)"),
            )
            .changed()
            {
                config::save_config(state);
                changed = true;
            }
        });
    });

    changed
}

fn draw_pc88_game_library_section(ui: &mut egui::Ui, state: &mut AppState, changed: &mut bool) {
    // 1. Root Games Folder row
    ui.horizontal(|ui| {
        ui.add_sized(
            [140.0, 20.0],
            egui::Label::new(
                egui::RichText::new("Games Root Folder:")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            ),
        );

        let avail = (ui.available_width() - 165.0).max(120.0);
        if ui
            .add_sized(
                [avail, 20.0],
                egui::TextEdit::singleline(&mut state.pc88.config.games_dir)
                    .hint_text("/path/to/PC88/GameList"),
            )
            .changed()
        {
            config::save_config(state);
            *changed = true;
        }

        if ui.button("Browse...").clicked() {
            state.open_pc88_media_dialog(Pc88MediaSlot::GamesDir, ui.ctx());
        }

        let can_refresh = !state.pc88.config.games_dir.trim().is_empty();
        if ui
            .add_enabled(can_refresh, egui::Button::new("🔄 Refresh"))
            .on_hover_text("Scan the root games directory and all subdirectories for PC-88 games")
            .clicked()
        {
            match state.pc88.refresh_games_list() {
                Ok(count) => {
                    state.info(format!("Scanned PC-88 games folder: found {count} games."));
                }
                Err(e) => {
                    state.error(format!("Failed to scan PC-88 games folder: {e}"));
                }
            }
            config::save_config(state);
            *changed = true;
        }
    });

    technical_separator(ui);

    // 2. Game selection dropdown + search filter + quick launch button
    let total_games = state.pc88.games_list.len();
    let has_active_filter = !state.pc88.game_filter.trim().is_empty();
    let filtered_indices = if total_games > 0 {
        crate::pc88::library::filter_and_rank_games(
            &state.pc88.games_list,
            &state.pc88.game_filter,
        )
    } else {
        Vec::new()
    };

    let selected_idx = state
        .pc88
        .games_list
        .iter()
        .position(|g| g.id == state.pc88.config.selected_game);

    let mut newly_picked_idx: Option<usize> = None;

    ui.horizontal(|ui| {
        let label_text = if total_games > 0 {
            if has_active_filter {
                format!("Select Game ({}/{total_games}):", filtered_indices.len())
            } else {
                format!("Select Game ({total_games}):")
            }
        } else {
            "Select Game:".to_string()
        };
        ui.add_sized(
            [140.0, 20.0],
            egui::Label::new(
                egui::RichText::new(label_text)
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            ),
        );

        if total_games == 0 {
            ui.label(
                egui::RichText::new(
                    "No games scanned yet — set Games Root Folder above and click 🔄 Refresh.",
                )
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(136, 136, 136)),
            );
        } else {
            let filter_resp = ui
                .add_sized(
                    [155.0, 20.0],
                    egui::TextEdit::singleline(&mut state.pc88.game_filter)
                        .hint_text("🔎 Fuzzy / Romaji / JP..."),
                )
                .on_hover_text(
                    "Search by Japanese (Kanji/Kana), Romaji (e.g. 'ys', 'snatcher', 'xanadu'), English, or fuzzy abbreviation. Press Enter to select top match.",
                );
            if filter_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if let Some(&top_idx) = filtered_indices.first() {
                    newly_picked_idx = Some(top_idx);
                }
            }
            if !state.pc88.game_filter.is_empty() && ui.small_button("✕").clicked() {
                state.pc88.game_filter.clear();
            }

            let full_selected_text = selected_idx
                .map(|i| state.pc88.games_list[i].display_label())
                .unwrap_or_else(|| "Choose a PC-88 game to load...".to_string());

            let combo_width = (ui.available_width() - 140.0).clamp(120.0, 320.0);
            let max_cols = ((combo_width / 8.5) as usize).clamp(14, 36);
            let selected_text = truncate_combo_label(&full_selected_text, max_cols);

            egui::ComboBox::from_id_source("pc88_game_selector_combo")
                .selected_text(selected_text)
                .width(combo_width)
                .show_ui(ui, |ui| {
                    if filtered_indices.is_empty() {
                        ui.label(
                            egui::RichText::new("No matching games found.")
                                .monospace()
                                .size(10.5)
                                .color(egui::Color32::from_rgb(180, 120, 120)),
                        );
                    } else {
                        for &idx in &filtered_indices {
                            let game = &state.pc88.games_list[idx];
                            let is_selected = selected_idx == Some(idx);
                            if ui
                                .selectable_label(is_selected, game.display_label())
                                .clicked()
                            {
                                newly_picked_idx = Some(idx);
                            }
                        }
                    }
                })
                .response
                .on_hover_text(&full_selected_text);

            let launch_label = if state.pc88.is_active {
                "🔄 Reboot Game"
            } else {
                "▶ Launch Game"
            };
            let launch_btn = egui::Button::new(
                egui::RichText::new(launch_label)
                    .size(11.0)
                    .strong()
                    .color(egui::Color32::from_rgb(210, 255, 220)),
            )
            .fill(egui::Color32::from_rgb(24, 72, 42))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(42, 118, 68)))
            .rounding(egui::Rounding::ZERO);

            if ui
                .add_enabled(selected_idx.is_some(), launch_btn)
                .on_hover_text("Boot (or restart) the PC-88 emulator with the selected game")
                .clicked()
            {
                if let Some(idx) = selected_idx {
                    let _ = state.pc88.select_game_by_index(idx);
                    config::save_config(state);
                }
                state.start_pc88_mode(ui.ctx());
                *changed = true;
            }
        }
    });

    // Live top fuzzy matches bar when filter text is entered
    if total_games > 0 && has_active_filter {
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.add_sized(
                [140.0, 18.0],
                egui::Label::new(
                    egui::RichText::new("Top Fuzzy Matches:")
                        .monospace()
                        .size(10.5)
                        .color(egui::Color32::from_rgb(153, 153, 153)),
                ),
            );
            if filtered_indices.is_empty() {
                ui.label(
                    egui::RichText::new(format!(
                        "No games match '{}'",
                        state.pc88.game_filter.trim()
                    ))
                    .monospace()
                    .size(10.5)
                    .color(egui::Color32::from_rgb(220, 130, 130)),
                );
            } else {
                for &idx in filtered_indices.iter().take(5) {
                    let game = &state.pc88.games_list[idx];
                    let is_sel = selected_idx == Some(idx);
                    let short_title = truncate_combo_label(&game.title, 22);
                    let btn = egui::Button::new(
                        egui::RichText::new(short_title)
                            .monospace()
                            .size(10.5)
                            .color(if is_sel {
                                egui::Color32::from_rgb(220, 255, 220)
                            } else {
                                egui::Color32::from_rgb(200, 220, 245)
                            }),
                    )
                    .fill(if is_sel {
                        egui::Color32::from_rgb(28, 74, 44)
                    } else {
                        egui::Color32::from_rgb(30, 38, 52)
                    })
                    .stroke(egui::Stroke::new(
                        1.0,
                        if is_sel {
                            egui::Color32::from_rgb(56, 138, 84)
                        } else {
                            egui::Color32::from_rgb(58, 76, 104)
                        },
                    ))
                    .rounding(egui::Rounding::ZERO);

                    if ui
                        .add(btn)
                        .on_hover_text(game.display_label())
                        .clicked()
                    {
                        newly_picked_idx = Some(idx);
                    }
                }
                if filtered_indices.len() > 5 {
                    ui.label(
                        egui::RichText::new(format!(
                            "(+{} more in dropdown)",
                            filtered_indices.len() - 5
                        ))
                        .monospace()
                        .size(10.0)
                        .color(egui::Color32::from_rgb(136, 136, 136)),
                    );
                }
            }
        });
    }

    if let Some(idx) = newly_picked_idx {
        match state.pc88.select_game_by_index(idx) {
            Ok(title) => {
                state.info(format!("Loaded PC-88 game media for '{title}'."));
            }
            Err(e) => {
                state.error(format!("Failed to configure PC-88 game media: {e}"));
            }
        }
        config::save_config(state);
        if state.pc88.is_active {
            state.start_pc88_mode(ui.ctx());
        }
        *changed = true;
    }

    // 3. Active game drive status + multi-disk floppy quick swapper
    if selected_idx.is_some() {
        let mut mounted_parts = Vec::new();
        for (slot_name, p_str) in [
            ("FDD1", state.pc88.config.fdd1_path.trim()),
            ("FDD2", state.pc88.config.fdd2_path.trim()),
            ("Tape", state.pc88.config.tape_path.trim()),
        ] {
            if !p_str.is_empty() {
                let label = state
                    .pc88
                    .active_game_floppies
                    .iter()
                    .position(|p| p.to_string_lossy() == p_str)
                    .and_then(|i| state.pc88.active_game_disk_labels.get(i))
                    .cloned()
                    .unwrap_or_else(|| {
                        std::path::Path::new(p_str)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(p_str)
                            .to_string()
                    });
                mounted_parts.push(format!("{slot_name}: {label}"));
            }
        }
        if !mounted_parts.is_empty() {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(format!(
                    "Mounted Media  ->  {}",
                    mounted_parts.join("   |   ")
                ))
                .monospace()
                .size(10.5)
                .color(egui::Color32::from_rgb(130, 200, 150)),
            );
        }

        if state.pc88.active_game_floppies.len() > 1 {
            let disks: Vec<(std::path::PathBuf, String)> = state
                .pc88
                .active_game_floppies
                .iter()
                .cloned()
                .enumerate()
                .map(|(i, p)| {
                    let lbl = state
                        .pc88
                        .active_game_disk_labels
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| {
                            p.file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("Disk")
                                .to_string()
                        });
                    (p, lbl)
                })
                .collect();

            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_sized(
                    [140.0, 20.0],
                    egui::Label::new(
                        egui::RichText::new("Quick Disk Swap:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    ),
                );

                for (drive_idx, slot_label) in [(0u8, "FDD 1:"), (1u8, "FDD 2:")] {
                    ui.label(
                        egui::RichText::new(slot_label)
                            .monospace()
                            .size(10.5)
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    );
                    let cur_path = if drive_idx == 0 {
                        state.pc88.config.fdd1_path.clone()
                    } else {
                        state.pc88.config.fdd2_path.clone()
                    };
                    let cur_name = disks
                        .iter()
                        .find(|(p, _)| p.to_string_lossy() == cur_path)
                        .map(|(_, l)| l.as_str())
                        .or_else(|| {
                            std::path::Path::new(&cur_path)
                                .file_name()
                                .and_then(|n| n.to_str())
                        })
                        .unwrap_or("(Empty)");

                    egui::ComboBox::from_id_source(format!("pc88_quick_fdd_swap_{drive_idx}"))
                        .selected_text(truncate_combo_label(cur_name, 20))
                        .width(175.0)
                        .show_ui(ui, |ui| {
                            for (disk_path, disk_label) in &disks {
                                let disk_str = disk_path.to_string_lossy().to_string();
                                if ui
                                    .selectable_label(cur_path == disk_str, disk_label)
                                    .clicked()
                                {
                                    if drive_idx == 0 {
                                        state.pc88.config.fdd1_path = disk_str;
                                    } else {
                                        state.pc88.config.fdd2_path = disk_str;
                                    }
                                    if state.pc88.is_active {
                                        state.pc88.send_command(Pc88Command::MountFdd {
                                            drive: drive_idx,
                                            path: Some(disk_path.clone()),
                                        });
                                    }
                                    config::save_config(state);
                                    *changed = true;
                                }
                            }
                        });
                    ui.add_space(8.0);
                }
            });
        }
    }
}

fn draw_pc88_media_row(
    ui: &mut egui::Ui,
    state: &mut AppState,
    label: &str,
    slot: Pc88MediaSlot,
    hint: &str,
    live_swappable: bool,
    changed: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [140.0, 20.0],
            egui::Label::new(
                egui::RichText::new(label)
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            ),
        );

        let path_ref = match slot {
            Pc88MediaSlot::Fdd1 => &mut state.pc88.config.fdd1_path,
            Pc88MediaSlot::Fdd2 => &mut state.pc88.config.fdd2_path,
            Pc88MediaSlot::Tape => &mut state.pc88.config.tape_path,
            Pc88MediaSlot::GamesDir => &mut state.pc88.config.games_dir,
        };

        let avail = (ui.available_width() - 135.0).max(120.0);
        let text_resp = ui.add_sized(
            [avail, 20.0],
            egui::TextEdit::singleline(path_ref).hint_text(hint),
        );
        if text_resp.changed() {
            if live_swappable && state.pc88.is_active {
                let drive = if slot == Pc88MediaSlot::Fdd1 { 0 } else { 1 };
                let p = if path_ref.trim().is_empty() {
                    None
                } else {
                    Some(std::path::PathBuf::from(path_ref.trim()))
                };
                state.pc88.send_command(Pc88Command::MountFdd { drive, path: p });
            }
            config::save_config(state);
            *changed = true;
        }

        if ui.button("Browse...").clicked() {
            state.open_pc88_media_dialog(slot, ui.ctx());
        }

        let has_media = match slot {
            Pc88MediaSlot::Fdd1 => !state.pc88.config.fdd1_path.is_empty(),
            Pc88MediaSlot::Fdd2 => !state.pc88.config.fdd2_path.is_empty(),
            Pc88MediaSlot::Tape => !state.pc88.config.tape_path.is_empty(),
            Pc88MediaSlot::GamesDir => !state.pc88.config.games_dir.is_empty(),
        };

        if ui
            .add_enabled(has_media, egui::Button::new("Eject"))
            .clicked()
        {
            match slot {
                Pc88MediaSlot::Fdd1 => {
                    state.pc88.config.fdd1_path.clear();
                    state.pc88.send_command(Pc88Command::MountFdd {
                        drive: 0,
                        path: None,
                    });
                }
                Pc88MediaSlot::Fdd2 => {
                    state.pc88.config.fdd2_path.clear();
                    state.pc88.send_command(Pc88Command::MountFdd {
                        drive: 1,
                        path: None,
                    });
                }
                Pc88MediaSlot::Tape => state.pc88.config.tape_path.clear(),
                Pc88MediaSlot::GamesDir => state.pc88.config.games_dir.clear(),
            }
            config::save_config(state);
            *changed = true;
        }
    });
}

enum SlotCardAction {
    Select(u8),
    Save(u8),
    Load(u8),
    Delete(u8),
}

fn draw_save_states_section(
    ui: &mut egui::Ui,
    state: &mut AppState,
    system: RetroPcSystem,
    changed: &mut bool,
) {
    use crate::pc98::savestate::{self, SAVE_SLOT_COUNT};

    let (game_key, game_title, is_active, active_slot, slots_meta, states_root, id_src) =
        match system {
            RetroPcSystem::Pc98 => {
                let (k, t) = state.pc98.ensure_slots_cache();
                (
                    k,
                    t,
                    state.pc98.is_active,
                    state.pc98.active_slot,
                    state.pc98.slots_cache.clone(),
                    savestate::pc98_states_root(),
                    "pc98_save_states_collapsible",
                )
            }
            RetroPcSystem::Pc88 => {
                let (k, t) = state.pc88.ensure_slots_cache();
                (
                    k,
                    t,
                    state.pc88.is_active,
                    state.pc88.active_slot,
                    state.pc88.slots_cache.clone(),
                    savestate::pc88_states_root(),
                    "pc88_save_states_collapsible",
                )
            }
        };

    let saved_count = slots_meta.iter().filter(|s| s.is_some()).count();
    let header_title = format!(
        "Save States — {} ({}/{} saved)",
        truncate_combo_label(&game_title, 36),
        saved_count,
        SAVE_SLOT_COUNT
    );

    let mut pending_action: Option<SlotCardAction> = None;

    egui::CollapsingHeader::new(
        egui::RichText::new(header_title)
            .monospace()
            .size(11.0)
            .strong()
            .color(egui::Color32::from_rgb(210, 224, 214)),
    )
    .id_source(id_src)
    .default_open(false)
    .show(ui, |ui| {
        ui.add_space(4.0);

        // Ensure thumbnail textures are loaded for occupied slots when expanded
        for idx in 0..SAVE_SLOT_COUNT {
            let slot = (idx + 1) as u8;
            if slots_meta[idx].is_some() {
                let cache_key = (game_key.clone(), slot);
                let already_loaded = match system {
                    RetroPcSystem::Pc98 => state.pc98.slot_textures.contains_key(&cache_key),
                    RetroPcSystem::Pc88 => state.pc88.slot_textures.contains_key(&cache_key),
                };
                if !already_loaded {
                    if let Some((w, h, rgba)) =
                        savestate::load_slot_thumbnail_rgba(&states_root, &game_key, slot)
                    {
                        let color_img = egui::ColorImage::from_rgba_unmultiplied(
                            [w as usize, h as usize],
                            &rgba,
                        );
                        let tex = ui.ctx().load_texture(
                            format!("savestate_thumb_{:?}_{}_{}", system, game_key, slot),
                            color_img,
                            egui::TextureOptions::LINEAR,
                        );
                        match system {
                            RetroPcSystem::Pc98 => {
                                state.pc98.slot_textures.insert(cache_key, tex);
                            }
                            RetroPcSystem::Pc88 => {
                                state.pc88.slot_textures.insert(cache_key, tex);
                            }
                        }
                    }
                }
            }
        }

        // 2 rows x 4 columns of visual CRT snapshot cards fitted strictly inside available width
        let avail_w = ui.available_width();
        let spacing = 6.0;
        let cols = 4usize;
        let frame_margin = 4.0;
        // Total horizontal overhead per card frame = 2 * frame_margin (8px) + 2 * stroke (2px) = 10px
        let card_outer_w =
            ((avail_w - spacing * ((cols - 1) as f32)) / (cols as f32)).floor().max(90.0);
        let thumb_w = (card_outer_w - (frame_margin * 2.0 + 2.0)).max(76.0);
        let thumb_h = (thumb_w * (200.0 / 320.0)).round();

        for row in 0..2 {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = spacing;
                for col in 0..cols {
                    let idx = row * cols + col;
                    let slot = (idx + 1) as u8;
                    let is_sel = active_slot == slot;
                    let meta_opt = &slots_meta[idx];
                    let tex_id = {
                        let key_pair = (game_key.clone(), slot);
                        match system {
                            RetroPcSystem::Pc98 => {
                                state.pc98.slot_textures.get(&key_pair).map(|t| t.id())
                            }
                            RetroPcSystem::Pc88 => {
                                state.pc88.slot_textures.get(&key_pair).map(|t| t.id())
                            }
                        }
                    };

                    let frame_fill = if is_sel {
                        egui::Color32::from_rgb(20, 30, 24)
                    } else {
                        egui::Color32::from_rgb(18, 18, 18)
                    };
                    let frame_stroke = if is_sel {
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(56, 152, 88))
                    } else if meta_opt.is_some() {
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(52, 66, 56))
                    } else {
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(42, 42, 42))
                    };

                    egui::Frame::none()
                        .fill(frame_fill)
                        .stroke(frame_stroke)
                        .inner_margin(egui::Margin::same(frame_margin))
                        .rounding(egui::Rounding::ZERO)
                        .show(ui, |ui| {
                            ui.set_width(thumb_w);
                            ui.set_max_width(thumb_w);
                            ui.vertical(|ui| {
                                // Card header: Slot number + size/empty badge
                                ui.horizontal(|ui| {
                                    let badge_text = if is_sel {
                                        format!("▶ SLOT {}", slot)
                                    } else {
                                        format!("SLOT {}", slot)
                                    };
                                    if ui
                                        .selectable_label(
                                            is_sel,
                                            egui::RichText::new(badge_text)
                                                .monospace()
                                                .size(10.0)
                                                .strong()
                                                .color(if is_sel {
                                                    egui::Color32::from_rgb(160, 255, 185)
                                                } else {
                                                    egui::Color32::from_rgb(210, 210, 210)
                                                }),
                                        )
                                        .clicked()
                                    {
                                        pending_action = Some(SlotCardAction::Select(slot));
                                    }

                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let right_text = meta_opt
                                                .as_ref()
                                                .map(|m| m.formatted_size())
                                                .unwrap_or_else(|| "EMPTY".to_string());
                                            ui.label(
                                                egui::RichText::new(right_text)
                                                    .monospace()
                                                    .size(9.0)
                                                    .color(if meta_opt.is_some() {
                                                        egui::Color32::from_rgb(135, 195, 155)
                                                    } else {
                                                        egui::Color32::from_rgb(100, 100, 100)
                                                    }),
                                            );
                                        },
                                    );
                                });

                                ui.add_space(2.0);

                                // 16:10 CRT Preview Screen
                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(thumb_w, thumb_h),
                                    egui::Sense::click(),
                                );
                                let painter = ui.painter_at(rect);
                                painter.rect_filled(
                                    rect,
                                    egui::Rounding::same(2.0),
                                    egui::Color32::from_rgb(10, 12, 11),
                                );

                                if let Some(tid) = tex_id {
                                    painter.image(
                                        tid,
                                        rect.shrink(1.0),
                                        egui::Rect::from_min_max(
                                            egui::pos2(0.0, 0.0),
                                            egui::pos2(1.0, 1.0),
                                        ),
                                        egui::Color32::WHITE,
                                    );
                                    // Subtle CRT scanline overlay across thumbnail
                                    let mut y = rect.top() + 2.0;
                                    while y < rect.bottom() {
                                        painter.line_segment(
                                            [
                                                egui::pos2(rect.left(), y),
                                                egui::pos2(rect.right(), y),
                                            ],
                                            egui::Stroke::new(
                                                1.0,
                                                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 42),
                                            ),
                                        );
                                        y += 3.0;
                                    }
                                } else {
                                    let label = if meta_opt.is_some() {
                                        "SAVED STATE"
                                    } else {
                                        "NO SNAPSHOT"
                                    };
                                    painter.text(
                                        rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        label,
                                        egui::FontId::monospace(9.5),
                                        egui::Color32::from_rgb(78, 86, 80),
                                    );
                                }

                                painter.rect_stroke(
                                    rect,
                                    egui::Rounding::same(2.0),
                                    egui::Stroke::new(
                                        1.0,
                                        if is_sel {
                                            egui::Color32::from_rgb(68, 168, 100)
                                        } else {
                                            egui::Color32::from_rgb(38, 44, 40)
                                        },
                                    ),
                                );

                                if resp.double_clicked() && meta_opt.is_some() {
                                    pending_action = Some(SlotCardAction::Load(slot));
                                } else if resp.clicked() {
                                    pending_action = Some(SlotCardAction::Select(slot));
                                }
                                if let Some(m) = meta_opt {
                                    let hover = match m.media_label() {
                                        Some(disk) => format!(
                                            "Slot {} — {}\nSaved: {}\nMedia: {}\nDouble-click to load",
                                            slot,
                                            m.game_title,
                                            m.formatted_timestamp(),
                                            disk
                                        ),
                                        None => format!(
                                            "Slot {} — {}\nSaved: {}\nDouble-click to load",
                                            slot,
                                            m.game_title,
                                            m.formatted_timestamp()
                                        ),
                                    };
                                    resp.on_hover_text(hover);
                                } else {
                                    resp.on_hover_text(format!(
                                        "Slot {} is empty. Click Save while playing to capture a snapshot.",
                                        slot
                                    ));
                                }

                                ui.add_space(2.0);

                                // Timestamp & disk subtitle
                                let ts_text = meta_opt
                                    .as_ref()
                                    .map(|m| m.formatted_timestamp())
                                    .unwrap_or_else(|| "Empty slot".to_string());
                                ui.label(
                                    egui::RichText::new(ts_text)
                                        .monospace()
                                        .size(9.0)
                                        .color(if meta_opt.is_some() {
                                            egui::Color32::from_rgb(175, 175, 175)
                                        } else {
                                            egui::Color32::from_rgb(95, 95, 95)
                                        }),
                                );

                                let max_sub_cols = ((thumb_w / 6.2) as usize).clamp(12, 24);
                                let media_sub = meta_opt
                                    .as_ref()
                                    .and_then(|m| m.media_label())
                                    .map(|d| truncate_combo_label(&d, max_sub_cols))
                                    .unwrap_or_else(|| "—".to_string());
                                ui.label(
                                    egui::RichText::new(media_sub)
                                        .monospace()
                                        .size(8.5)
                                        .color(egui::Color32::from_rgb(120, 155, 130)),
                                );

                                ui.add_space(2.0);

                                // Action buttons fitted strictly within thumb_w
                                let btn_gap = 3.0;
                                let del_w = 24.0;
                                let main_btn_w = ((thumb_w - del_w - btn_gap * 2.0) * 0.5)
                                    .floor()
                                    .max(24.0);
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = btn_gap;
                                    if ui
                                        .add_enabled(
                                            is_active,
                                            egui::Button::new(
                                                egui::RichText::new("Save").size(10.0),
                                            )
                                            .min_size(egui::vec2(main_btn_w, 18.0)),
                                        )
                                        .on_hover_text(format!("Save state to Slot {}", slot))
                                        .clicked()
                                    {
                                        pending_action = Some(SlotCardAction::Save(slot));
                                    }

                                    let load_text = if is_active { "Load" } else { "Boot" };
                                    if ui
                                        .add_enabled(
                                            meta_opt.is_some(),
                                            egui::Button::new(
                                                egui::RichText::new(load_text).size(10.0),
                                            )
                                            .min_size(egui::vec2(main_btn_w, 18.0)),
                                        )
                                        .on_hover_text(if is_active {
                                            format!("Load state from Slot {}", slot)
                                        } else {
                                            format!(
                                                "Boot emulator and automatically load Slot {}",
                                                slot
                                            )
                                        })
                                        .clicked()
                                    {
                                        pending_action = Some(SlotCardAction::Load(slot));
                                    }

                                    if ui
                                        .add_enabled(
                                            meta_opt.is_some(),
                                            egui::Button::new(egui::RichText::new("🗑").size(10.0))
                                                .min_size(egui::vec2(del_w, 18.0)),
                                        )
                                        .on_hover_text(format!("Delete Slot {}", slot))
                                        .clicked()
                                    {
                                        pending_action = Some(SlotCardAction::Delete(slot));
                                    }
                                });
                            });
                        });
                }
            });
            if row == 0 {
                ui.add_space(spacing);
            }
        }
    });

    if let Some(action) = pending_action {
        match action {
            SlotCardAction::Select(slot) => match system {
                RetroPcSystem::Pc98 => state.pc98.active_slot = slot,
                RetroPcSystem::Pc88 => state.pc88.active_slot = slot,
            },
            SlotCardAction::Save(slot) => match system {
                RetroPcSystem::Pc98 => {
                    state.pc98.active_slot = slot;
                    state.pc98.send_command(Pc98Command::SaveState { slot });
                }
                RetroPcSystem::Pc88 => {
                    state.pc88.active_slot = slot;
                    state.pc88.send_command(Pc88Command::SaveState { slot });
                }
            },
            SlotCardAction::Load(slot) => match system {
                RetroPcSystem::Pc98 => {
                    state.pc98.active_slot = slot;
                    if state.pc98.is_active {
                        state.pc98.send_command(Pc98Command::LoadState { slot });
                    } else {
                        state.pc98.pending_boot_load_slot = Some(slot);
                        state.start_pc98_mode(ui.ctx());
                        *changed = true;
                    }
                }
                RetroPcSystem::Pc88 => {
                    state.pc88.active_slot = slot;
                    if state.pc88.is_active {
                        state.pc88.send_command(Pc88Command::LoadState { slot });
                    } else {
                        state.pc88.pending_boot_load_slot = Some(slot);
                        state.start_pc88_mode(ui.ctx());
                        *changed = true;
                    }
                }
            },
            SlotCardAction::Delete(slot) => {
                if let Err(e) = savestate::delete_slot(&states_root, &game_key, slot) {
                    state.error(format!("Failed to delete slot {}: {e}", slot));
                } else {
                    match system {
                        RetroPcSystem::Pc98 => {
                            state.pc98.slot_textures.remove(&(game_key.clone(), slot));
                            state.pc98.refresh_slots_cache();
                        }
                        RetroPcSystem::Pc88 => {
                            state.pc88.slot_textures.remove(&(game_key.clone(), slot));
                            state.pc88.refresh_slots_cache();
                        }
                    }
                    state.info(format!("Deleted Save Slot {} for '{}'.", slot, game_title));
                }
            }
        }
    }
}
