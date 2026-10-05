use super::{config::SAFETY_RESERVE, Replay};
use eframe::egui::{self, Key};
pub fn key(n: u8) -> Option<Key> {
    [
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
    ]
    .get(n.wrapping_sub(1) as usize)
    .copied()
}
fn gib(n: usize) -> String {
    format!("{:.2} GiB", n as f64 / (1024. * 1024. * 1024.))
}
pub fn draw_toggle(replay: &mut Replay, ui: &mut egui::Ui, streaming: bool) {
    let mut enabled = replay.runtime.is_some();
    if ui
        .add_enabled(
            streaming || enabled,
            egui::Checkbox::new(&mut enabled, "Enable replay buffer"),
        )
        .changed()
    {
        if enabled {
            if let Err(e) = replay.enable() {
                replay.last_status.message = e.to_string();
            }
        } else {
            replay.disable();
        }
    }
}

pub fn draw(replay: &mut Replay, ui: &mut egui::Ui) -> bool {
    let mut changed = false;
    ui.group(|ui| {
        ui.strong("Live replay buffer");
        if replay.runtime.is_none()
            && replay.available_now.is_some_and(|available| {
                replay.config.budget().saturating_add(SAFETY_RESERVE) >= available
            })
        {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Budget exceeds available RAM with the 512 MiB safety reserve.",
            );
        }
        let status = replay.status();
        if !status.message.is_empty() {
            ui.label(&status.message);
        }
        if replay.runtime.is_none()
            && !replay.last_status.message.is_empty()
            && replay.last_status.message != status.message
        {
            ui.label(&replay.last_status.message);
        }
        if status.saving {
            ui.label("Saving…");
        }
        ui.add_enabled_ui(replay.runtime.is_none(), |ui| {
            ui.horizontal(|ui| {
                ui.label("Maximum history (seconds)");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut replay.config.history_seconds)
                            .clamp_range(1..=3600),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("RAM budget (MiB)");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut replay.config.memory_mib)
                            .clamp_range(256..=32768),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Work queue RAM (MiB)");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut replay.config.work_queue_mib)
                            .clamp_range(32..=8192),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("GPU render device");
                changed |= ui
                    .text_edit_singleline(&mut replay.config.render_device)
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Save folder");
                changed |= ui
                    .text_edit_singleline(&mut replay.config.directory)
                    .changed();
            });
        });
        changed |= ui
            .checkbox(
                &mut replay.config.capture_overlays,
                "Capture overlays in replay buffer (popup & OCR)",
            )
            .on_hover_text(
                "Include active dictionary popups and OCR overlay boxes in replay recordings.\nIf 'Hide OCR boxes overlay' is enabled in OCR settings, only the dictionary popup is captured.\nWhen unchecked, replays record clean game video without any overlays.",
            )
            .changed();
        egui::CollapsingHeader::new("Save shortcuts")
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Custom clip seconds");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut replay.config.custom_seconds)
                                .clamp_range(1..=3600),
                        )
                        .changed();
                });
                for (index, seconds) in replay.config.durations().into_iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("Save {seconds}s"));
                        egui::ComboBox::from_id_source(("replay-key", index))
                            .selected_text(if replay.config.keys[index] == 0 {
                                "Disabled".into()
                            } else {
                                format!("F{}", replay.config.keys[index])
                            })
                            .show_ui(ui, |ui| {
                                for number in 0..=12 {
                                    let assigned = number != 0
                                        && replay
                                            .config
                                            .keys
                                            .iter()
                                            .enumerate()
                                            .any(|(i, k)| i != index && *k == number);
                                    ui.add_enabled_ui(!assigned, |ui| {
                                        changed |= ui
                                            .selectable_value(
                                                &mut replay.config.keys[index],
                                                number,
                                                if number == 0 {
                                                    "Disabled".into()
                                                } else {
                                                    format!("F{number}")
                                                },
                                            )
                                            .changed();
                                    });
                                }
                            });
                        if ui
                            .add_enabled(
                                replay.runtime.is_some() && !status.saving,
                                egui::Button::new("Save now"),
                            )
                            .clicked()
                        {
                            if let Err(e) = replay.save(seconds) {
                                replay.last_status.message = e.to_string();
                                if let Some(r) = &replay.runtime {
                                    r.shared.message(e.to_string());
                                }
                            }
                        }
                    });
                }
            });
    });
    changed
}

pub fn draw_debug(replay: &Replay, ui: &mut egui::Ui) {
    let available = replay.available_now;
    if let Some(before) = replay.available_before {
        ui.label(format!("Available RAM before enabling: {}", gib(before)));
    }
    if let Some(available) = available {
        ui.label(format!("Available RAM now: {}", gib(available)));
        if replay.runtime.is_none() {
            ui.label(format!(
                "Projected available RAM at full budget: {}",
                gib(available.saturating_sub(replay.config.budget()))
            ));
        }
    }
    let status = replay.status();
    ui.label(format!(
        "Clipboard clip / reservation: {}",
        gib(status.clipboard_bytes)
    ));
    ui.label(format!("History resets: {}", status.resets));
    if status.resets > 0 {
        ui.label(format!("Last reset: {}", status.last_reset));
    }
    if status.history_exhaustions > 0 {
        ui.label(format!(
            "History emptied by limits: {} · {}",
            status.history_exhaustions, status.last_exhaustion
        ));
    }
    ui.label(format!(
        "Retained: {:.1}s / {}s · packets and active save: {}",
        status.seconds,
        replay.config.history_seconds,
        gib(status.bytes)
    ));
    ui.label(format!(
        "Work queues + staging/encoder allowance: {}",
        gib(status.overhead)
    ));
    if status.queue_slots > 0 {
        ui.label(format!(
            "Work queue: {} GPU + {} CPU frames waiting · {} slots per stage · {} reserved",
            status.gpu_pending,
            status.cpu_pending,
            status.queue_slots,
            gib(status.queue_bytes)
        ));
        ui.label(format!("Recording behind live: {:.2}s · dropped video total: {} (+{} since last update) · audio drop events: {}", status.backlog_ms as f64 / 1000., status.video_dropped, status.recent_video_drops, status.audio_dropped));
    }
    if status.conversion_threads > 0 {
        ui.label(format!("Recording work per frame: conversion {:.2} ms ({} threads) · upload/encode {:.2} ms · frame interval {:.2} ms", status.conversion_ms, status.conversion_threads, status.hardware_ms, status.frame_interval_ms));
    }
    if let Some((w, h)) = status.surface {
        ui.label(format!("Recording surface: {w} × {h} · {}", status.codec));
    }
    if status.seconds > 2. && !status.saving && status.bytes > 0 {
        let estimate =
            (status.bytes as f64 / status.seconds * replay.config.history_seconds as f64) as usize;
        ui.label(format!(
            "At the observed bitrate: ~{} for {}s of packets + {} staging allowance",
            gib(estimate),
            replay.config.history_seconds,
            gib(status.overhead)
        ));
    }
    if !status.message.is_empty() {
        ui.label(&status.message);
    }
    ui.label(format!(
        "Configured: H.264 VBR 8/10 Mbit/s · 60 fps CFR · AAC 160k · {}",
        replay.config.render_device
    ));
    ui.label(format!(
        "RAM budget: {} MiB · work queue budget: {} MiB",
        replay.config.memory_mib, replay.config.work_queue_mib
    ));
}
