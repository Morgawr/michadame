use crate::app::AppState;
use crate::config;
use crate::pc98::{Pc98Command, Pc98MediaSlot};
use crate::ui::controls::{
    percent_slider_range, slider_item, sub_heading, technical_group, technical_separator,
    two_columns,
};
use eframe::egui;

pub fn draw_pc98_tab(ui: &mut egui::Ui, state: &mut AppState) -> bool {
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
            ui.label(
                egui::RichText::new(
                    "Floppy drives (FDD1 / FDD2) can be swapped live while the PC-98 is running. Hard disks (HDD1 / HDD2) are mounted on boot.",
                )
                .size(10.5)
                .color(egui::Color32::from_rgb(136, 136, 136)),
            );
            ui.add_space(4.0);

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
                .on_hover_text("Emulates the authentic mechanical stepper motor sounds of the PC-98 5.25\" / 3.5\" floppy drives")
                .changed()
            {
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
