use super::{
    config::{DenoiseBackend, FilterPreset, HumFilterMode, SAFETY_RESERVE},
    Replay,
};
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
    if replay.config.audio_filter.enabled {
        if let Some(spec) = replay.config.audio_filter.build_filter_spec() {
            ui.label(format!("Audio filter active: {}", spec));
        }
    }
}

pub fn draw_audio_filter(replay: &mut Replay, ui: &mut egui::Ui) -> bool {
    let mut changed = false;
    let filter = &mut replay.config.audio_filter;

    egui::CollapsingHeader::new("🔊 Console Audio Noise Filter")
        .default_open(filter.enabled)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut filter.enabled, "Enable Noise Filter")
                    .on_hover_text(
                        "Real-time zero-latency DSP filter applied to live audio and recordings.\n\
                         Eliminates console interference hum, power harmonics, and CRT whine\n\
                         without muffling high-frequency game audio.",
                    )
                    .changed()
                {
                    changed = true;
                }
            });

            if !filter.enabled {
                ui.label(
                    egui::RichText::new("Filter is disabled. Live audio passes through untouched.")
                        .italics()
                        .color(egui::Color32::GRAY),
                );
                return;
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Preset:");
                let current_label = match filter.preset {
                    FilterPreset::DreamcastNtsc => "Dreamcast / NTSC (FFmpeg afftdn + 60 Hz Notch + 15.7 kHz CRT)",
                    FilterPreset::ConsolePal => "PAL Console (FFmpeg afftdn + 50 Hz Notch + 15.6 kHz CRT)",
                    FilterPreset::MainsHum60 => "60 Hz Mains Hum (Single Notch Only)",
                    FilterPreset::VoiceChat => "Voice Chat / Mic (RNNoise Neural Network)",
                    FilterPreset::Custom => "Custom DSP Configuration",
                };

                egui::ComboBox::from_id_source("audio_filter_preset")
                    .selected_text(current_label)
                    .show_ui(ui, |ui| {
                        let presets = [
                            (
                                FilterPreset::DreamcastNtsc,
                                "Dreamcast / NTSC (FFmpeg afftdn + 60 Hz Notch + 15.7 kHz CRT)",
                            ),
                            (
                                FilterPreset::ConsolePal,
                                "PAL Console (FFmpeg afftdn + 50 Hz Notch + 15.6 kHz CRT)",
                            ),
                            (
                                FilterPreset::MainsHum60,
                                "60 Hz Mains Hum (Single Notch Only)",
                            ),
                            (
                                FilterPreset::VoiceChat,
                                "Voice Chat / Mic (RNNoise Neural Network)",
                            ),
                            (
                                FilterPreset::Custom,
                                "Custom DSP Configuration",
                            ),
                        ];
                        for (preset, label) in presets {
                            if ui
                                .selectable_value(&mut filter.preset, preset, label)
                                .changed()
                            {
                                filter.apply_preset(preset);
                                changed = true;
                            }
                        }
                    });
            });

            ui.add_space(4.0);
            ui.group(|ui| {
                ui.label(egui::RichText::new("DSP Filter Stages").strong());

                // Realtime Denoising Backend
                ui.horizontal(|ui| {
                    ui.label("Realtime Denoiser:");
                    let denoise_label = match filter.denoise_backend {
                        DenoiseBackend::Afftdn => "FFmpeg afftdn (Adaptive FFT, Lapped Windows)",
                        DenoiseBackend::Rnnoise => "RNNoise (nnnoiseless Neural Network)",
                        DenoiseBackend::Disabled => "Disabled (Pass-through)",
                    };
                    egui::ComboBox::from_id_source("audio_filter_denoise_backend")
                        .selected_text(denoise_label)
                        .show_ui(ui, |ui| {
                            let backends = [
                                (
                                    DenoiseBackend::Afftdn,
                                    "FFmpeg afftdn (Adaptive FFT, Lapped Windows - Recommended for Games)",
                                ),
                                (
                                    DenoiseBackend::Rnnoise,
                                    "RNNoise (nnnoiseless Neural Network - Voice/Mic)",
                                ),
                                (
                                    DenoiseBackend::Disabled,
                                    "Disabled (Pass-through)",
                                ),
                            ];
                            for (backend, label) in backends {
                                if ui
                                    .selectable_value(&mut filter.denoise_backend, backend, label)
                                    .changed()
                                {
                                    changed = true;
                                }
                            }
                        });
                });

                if filter.denoise_backend == DenoiseBackend::Afftdn {
                    ui.horizontal(|ui| {
                        ui.label("Noise Reduction:");
                        if ui
                            .add(
                                egui::Slider::new(&mut filter.denoise_reduction_db, 6.0..=40.0)
                                    .suffix(" dB"),
                            )
                            .on_hover_text("Amount of spectral noise floor reduction (afftdn). 20-30 dB recommended.")
                            .changed()
                        {
                            changed = true;
                        }
                    });
                }

                // High-Frequency Retention / Treble Bypass
                if filter.denoise_backend != DenoiseBackend::Disabled {
                    ui.horizontal(|ui| {
                        if ui
                            .checkbox(
                                &mut filter.treble_bypass,
                                "Retain High Frequencies (Treble Bypass)",
                            )
                            .on_hover_text(
                                "Protects high frequencies above the crossover from being muffled by the denoiser.\n\
                                 Uses a 4th-order Linkwitz-Riley (LR4) crossover with exact 0.00 dB magnitude summing.\n\
                                 Keeps game music, cymbals, sound effects, and treble crisp and bright\n\
                                 while eliminating low/mid buzzing hum. Prevents underwater/muffled sound.",
                            )
                            .changed()
                        {
                            changed = true;
                        }

                        if filter.treble_bypass {
                            ui.label("Crossover:");
                            if ui
                                .add(
                                    egui::Slider::new(&mut filter.treble_crossover_hz, 1500.0..=8000.0)
                                        .suffix(" Hz"),
                                )
                                .on_hover_text("Frequencies above this cutoff completely bypass the denoiser and remain 100% untouched.")
                                .changed()
                            {
                                changed = true;
                            }
                        }
                    });
                }

                // Mains Hum Filter (Single high-Q notch, NO harmonic comb)
                ui.horizontal(|ui| {
                    ui.label("Mains Hum Notch:");
                    let hum_label = match filter.hum_filter {
                        HumFilterMode::SingleNotch60Hz => "Single 60 Hz Notch (NTSC / 60 Hz Mains)",
                        HumFilterMode::SingleNotch50Hz => "Single 50 Hz Notch (PAL / 50 Hz Mains)",
                        HumFilterMode::Custom => "Custom Frequency Notch",
                        HumFilterMode::Disabled => "Disabled",
                    };
                    egui::ComboBox::from_id_source("audio_filter_hum_mode")
                        .selected_text(hum_label)
                        .show_ui(ui, |ui| {
                            let modes = [
                                (
                                    HumFilterMode::SingleNotch60Hz,
                                    "Single 60 Hz Notch (NTSC / 60 Hz Mains - Zero phaser distortion)",
                                ),
                                (
                                    HumFilterMode::SingleNotch50Hz,
                                    "Single 50 Hz Notch (PAL / 50 Hz Mains - Zero phaser distortion)",
                                ),
                                (
                                    HumFilterMode::Custom,
                                    "Custom Frequency Notch",
                                ),
                                (
                                    HumFilterMode::Disabled,
                                    "Disabled",
                                ),
                            ];
                            for (mode, label) in modes {
                                if ui
                                    .selectable_value(&mut filter.hum_filter, mode, label)
                                    .changed()
                                {
                                    changed = true;
                                }
                            }
                        });

                    if filter.hum_filter == HumFilterMode::Custom {
                        ui.label("Freq:");
                        if ui
                            .add(
                                egui::DragValue::new(&mut filter.hum_freq)
                                    .speed(0.1)
                                    .clamp_range(20.0..=200.0)
                                    .suffix(" Hz"),
                            )
                            .changed()
                        {
                            changed = true;
                        }
                    }
                });

                // CRT Flyback notch filter
                ui.horizontal(|ui| {
                    if ui
                        .checkbox(
                            &mut filter.crt_notch,
                            "15 kHz CRT Line Whine Notch",
                        )
                        .on_hover_text(
                            "Eliminates horizontal scanline whine (15.734 kHz NTSC / 15.625 kHz PAL)\n\
                             with a narrow high-Q biquad notch filter.\n\
                             Leaves all surrounding high-frequency game audio intact.",
                        )
                        .changed()
                    {
                        changed = true;
                    }
                    if filter.crt_notch && filter.preset == FilterPreset::Custom {
                        ui.label("Center Freq:");
                        if ui
                            .add(
                                egui::DragValue::new(&mut filter.crt_freq)
                                    .speed(1.0)
                                    .clamp_range(10000.0..=20000.0)
                                    .suffix(" Hz"),
                            )
                            .changed()
                        {
                            changed = true;
                        }
                    }
                });

                // 40 Hz Subsonic Rumble HPF
                ui.horizontal(|ui| {
                    if ui
                        .checkbox(
                            &mut filter.rumble_filter,
                            "40 Hz Subsonic Rumble Filter",
                        )
                        .on_hover_text(
                            "Cuts ultra-low sub-bass rumble (< 40 Hz) caused by capture card DC offset\n\
                             and power supply line interference.",
                        )
                        .changed()
                    {
                        changed = true;
                    }
                });

                // Dynamic Downward Expander / Noise Gate
                ui.horizontal(|ui| {
                    if ui
                        .checkbox(
                            &mut filter.gate,
                            "Dynamic Noise Gate (Downward Expander)",
                        )
                        .on_hover_text(
                            "Silences residual background hiss during loading screens and pauses\n\
                             with smooth 5ms attack and 80ms release (no abrupt clicking).",
                        )
                        .changed()
                    {
                        changed = true;
                    }

                    if filter.gate {
                        ui.label("Threshold:");
                        if ui
                            .add(
                                egui::Slider::new(
                                    &mut filter.gate_threshold_db,
                                    -70.0..=-30.0,
                                )
                                .suffix(" dBFS"),
                            )
                            .changed()
                        {
                            changed = true;
                        }
                    }
                });
            });

            if let Some(spec) = filter.build_filter_spec() {
                ui.label(
                    egui::RichText::new(format!("Active pipeline: {}", spec))
                        .small()
                        .color(egui::Color32::LIGHT_BLUE),
                );
            }
        });

    if changed {
        replay.sync_audio_filter();
    }
    changed
}
