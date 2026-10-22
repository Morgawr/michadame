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

                ui.label(
                    egui::RichText::new(
                        "Automatically downloads np2kai_libretro.so on first launch if not installed.",
                    )
                    .size(10.5)
                    .color(egui::Color32::from_rgb(136, 136, 136)),
                );
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

    technical_group(ui, "Disk Drives & Media:", |ui| {
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
            }
            config::save_config(state);
            *changed = true;
        }
    });
}
