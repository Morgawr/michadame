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

pub fn draw_replay_buffer_group(
    replay: &mut Replay,
    ui: &mut egui::Ui,
    streaming: bool,
) -> bool {
    let mut changed = false;

    crate::ui::controls::technical_group(ui, "Live Replay Buffer:", |ui| {
        draw_toggle(replay, ui, streaming);

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

        ui.add_space(4.0);

        ui.add_enabled_ui(replay.runtime.is_none(), |ui| {
            crate::ui::controls::two_columns(ui, |c1, c2| {
                c1.horizontal(|ui| {
                    ui.add_sized(
                        [180.0, 18.0],
                        egui::Label::new(
                            egui::RichText::new("Maximum history (seconds):")
                                .monospace()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(170, 170, 170)),
                        ),
                    );
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut replay.config.history_seconds)
                                .clamp_range(1..=3600),
                        )
                        .changed();
                });

                c2.horizontal(|ui| {
                    ui.add_sized(
                        [160.0, 18.0],
                        egui::Label::new(
                            egui::RichText::new("RAM budget (MiB):")
                                .monospace()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(170, 170, 170)),
                        ),
                    );
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut replay.config.memory_mib)
                                .clamp_range(256..=32768),
                        )
                        .changed();
                });
            });

            crate::ui::controls::two_columns(ui, |c1, c2| {
                c1.horizontal(|ui| {
                    ui.add_sized(
                        [180.0, 18.0],
                        egui::Label::new(
                            egui::RichText::new("Work queue RAM (MiB):")
                                .monospace()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(170, 170, 170)),
                        ),
                    );
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut replay.config.work_queue_mib)
                                .clamp_range(32..=8192),
                        )
                        .changed();
                });

                c2.horizontal(|ui| {
                    ui.add_sized(
                        [160.0, 18.0],
                        egui::Label::new(
                            egui::RichText::new("GPU render device:")
                                .monospace()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(170, 170, 170)),
                        ),
                    );
                    let avail = (ui.available_width() - 4.0).max(60.0);
                    changed |= ui
                        .add_sized([avail, 18.0], egui::TextEdit::singleline(&mut replay.config.render_device))
                        .changed();
                });
            });

            ui.horizontal(|ui| {
                ui.add_sized(
                    [180.0, 18.0],
                    egui::Label::new(
                        egui::RichText::new("Save folder:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    ),
                );
                let avail = (ui.available_width() - 4.0).max(100.0);
                changed |= ui
                    .add_sized([avail, 18.0], egui::TextEdit::singleline(&mut replay.config.directory))
                    .changed();
            });
        });

        ui.add_space(2.0);
        changed |= ui
            .checkbox(
                &mut replay.config.capture_overlays,
                "Capture overlays in replay buffer (popup & OCR)",
            )
            .on_hover_text(
                "Include active dictionary popups and OCR overlay boxes in replay recordings.\nIf 'Hide OCR boxes overlay' is enabled in OCR settings, only the dictionary popup is captured.\nWhen unchecked, replays record clean game video without any overlays.",
            )
            .changed();

        crate::ui::controls::technical_separator(ui);
        crate::ui::controls::sub_heading(ui, "Save Shortcuts:");

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Custom clip seconds:")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            );
            changed |= ui
                .add(
                    egui::DragValue::new(&mut replay.config.custom_seconds)
                        .clamp_range(1..=3600),
                )
                .changed();
        });

        ui.add_space(2.0);

        let durations = replay.config.durations();
        let items: Vec<(usize, u32)> = durations.into_iter().enumerate().collect();
        if items.len() >= 4 {
            let row1 = (items[0], items[1]);
            let row2 = (items[2], items[3]);
            for ((idx_a, sec_a), (idx_b, sec_b)) in [row1, row2] {
                crate::ui::controls::two_columns(ui, |c1, c2| {
                    let mut draw_shortcut = |col: &mut egui::Ui, index: usize, seconds: u32| {
                        col.horizontal(|ui| {
                            let label_str = if index == 3 {
                                "Save Custom:".to_string()
                            } else {
                                format!("Save {}s:", seconds)
                            };
                            ui.add_sized(
                                [110.0, 18.0],
                                egui::Label::new(
                                    egui::RichText::new(label_str)
                                        .monospace()
                                        .size(11.0)
                                        .color(egui::Color32::from_rgb(170, 170, 170)),
                                ),
                            );

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
                    };

                    draw_shortcut(c1, idx_a, sec_a);
                    draw_shortcut(c2, idx_b, sec_b);
                });
            }
        }
    });

    changed
}

#[allow(dead_code)]
pub fn draw(replay: &mut Replay, ui: &mut egui::Ui) -> bool {
    draw_replay_buffer_group(replay, ui, false)
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

pub fn draw_audio_filter_group(replay: &mut Replay, ui: &mut egui::Ui) -> bool {
    let mut changed = false;
    let filter = &mut replay.config.audio_filter;

    crate::ui::controls::technical_group(ui, "", |ui| {
        ui.horizontal(|ui| {
            if ui
                .checkbox(&mut filter.enabled, "Console Audio Noise Filter")
                .on_hover_text(
                    "Real-time zero-latency DSP filter applied to live audio and recordings.\n\
                     Eliminates console interference hum, power harmonics, and CRT whine\n\
                     without muffling high-frequency game audio.",
                )
                .changed()
            {
                changed = true;
            }

            if filter.enabled {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Reset to Preset Defaults").clicked() {
                        filter.apply_preset(filter.preset);
                        changed = true;
                    }
                });
            }
        });

        if !filter.enabled {
            ui.label(
                egui::RichText::new("Filter is disabled. Live audio passes through untouched.")
                    .italics()
                    .color(egui::Color32::from_rgb(136, 136, 136)),
            );
            return;
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Preset:")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(170, 170, 170)),
                );
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

            crate::ui::controls::technical_separator(ui);
            crate::ui::controls::sub_heading(ui, "DSP Filter Stages:");

                // Realtime Denoising Backend
                ui.horizontal(|ui| {
                    ui.label("Realtime Denoiser:");
                    let denoise_label = match filter.denoise_backend {
                        DenoiseBackend::Afftdn => "FFmpeg afftdn (Adaptive FFT - Recommended for Games)",
                        DenoiseBackend::Rnnoise => "RNNoise (Voice / Mic Only - Mutes Music)",
                        DenoiseBackend::Disabled => "Disabled (Pass-through)",
                    };
                    egui::ComboBox::from_id_source("audio_filter_denoise_backend")
                        .selected_text(denoise_label)
                        .show_ui(ui, |ui| {
                            let backends = [
                                (
                                    DenoiseBackend::Afftdn,
                                    "FFmpeg afftdn (Adaptive FFT - Recommended for Games)",
                                ),
                                (
                                    DenoiseBackend::Rnnoise,
                                    "RNNoise (AI Voice / Mic Only - Mutes Game Music)",
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

                if filter.denoise_backend == DenoiseBackend::Rnnoise {
                    ui.label(
                        egui::RichText::new(
                            "⚠ RNNoise is trained strictly to isolate human speech. It classifies \
                             video game music, synthesizer tracks, and sound effects as noise and \
                             will mute them. For console gaming, use FFmpeg afftdn.",
                        )
                        .small()
                        .color(egui::Color32::from_rgb(230, 160, 40)),
                    );
                }

                if filter.denoise_backend == DenoiseBackend::Afftdn {
                    ui.add_space(2.0);
                    crate::ui::controls::two_columns(ui, |c1, c2| {
                        let slider = egui::Slider::new(&mut filter.denoise_reduction_db, 6.0..=40.0)
                            .step_by(0.5)
                            .suffix(" dB");
                        if crate::ui::controls::slider_item(
                            c1,
                            "Noise Reduction:",
                            slider,
                            Some(
                                "Amount of spectral noise floor reduction (afftdn).\n\
                                 12-20 dB is recommended for transparent filtering without phase smearing.\n\
                                 Values above 25 dB can cause underwater sound.",
                            ),
                        )
                        .changed()
                        {
                            changed = true;
                        }

                        let slider = egui::Slider::new(&mut filter.denoise_noise_floor_db, -70.0..=-25.0)
                            .step_by(0.5)
                            .suffix(" dBFS");
                        if crate::ui::controls::slider_item(
                            c2,
                            "Noise Floor:",
                            slider,
                            Some(
                                "Expected background noise floor level.\n\
                                 Setting this to -45 to -40 dBFS matches console AV cable ground buzz,\n\
                                 allowing modest noise reduction (15-20 dB) to completely eliminate buzz\n\
                                 without requiring aggressive reduction that causes artifacts.",
                            ),
                        )
                        .changed()
                        {
                            changed = true;
                        }
                    });

                    crate::ui::controls::two_columns(ui, |c1, c2| {
                        let slider = egui::Slider::new(&mut filter.denoise_gain_smooth, 0..=10)
                            .suffix(" bins");
                        if crate::ui::controls::slider_item(
                            c1,
                            "Smoothing:",
                            slider,
                            Some(
                                "FFT bin gain smoothing radius (default 0).\n\
                                 Set to 0 for pure point-wise filtering without bin smearing.\n\
                                 Values > 0 smear gain across neighboring bins.",
                            ),
                        )
                        .changed()
                        {
                            changed = true;
                        }

                        c2.horizontal(|ui| {
                            if ui
                                .checkbox(
                                    &mut filter.denoise_track_noise,
                                    "Track Bright-Scene Surges",
                                )
                                .on_hover_text(
                                    "Dynamically tracks fluctuating noise levels in real time.\n\
                                     Adapts when bright video scenes draw higher current through\n\
                                     shared video ground cables and cause the buzz to intensify.",
                                )
                                .changed()
                            {
                                changed = true;
                            }
                        });
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

                // Hum & Ground Return Buzz Notches
                ui.horizontal(|ui| {
                    ui.label("Hum / Buzz Notch:");
                    let hum_label = match filter.hum_filter {
                        HumFilterMode::HarmonicNotch60Hz => {
                            "Harmonic Buzz Filter (59.94 Hz NTSC / Dreamcast Harmonics to 1.5 kHz)"
                        }
                        HumFilterMode::HarmonicNotch50Hz => {
                            "Harmonic Buzz Filter (50.00 Hz PAL Harmonics to 1.5 kHz)"
                        }
                        HumFilterMode::SingleNotch60Hz => "Single 60 Hz Notch (Sub-bass hum only)",
                        HumFilterMode::SingleNotch50Hz => "Single 50 Hz Notch (Sub-bass hum only)",
                        HumFilterMode::Custom => "Custom Frequency Notch",
                        HumFilterMode::Disabled => "Disabled",
                    };
                    egui::ComboBox::from_id_source("audio_filter_hum_mode")
                        .selected_text(hum_label)
                        .show_ui(ui, |ui| {
                            let modes = [
                                (
                                    HumFilterMode::HarmonicNotch60Hz,
                                    "Harmonic Buzz Filter (59.94 Hz NTSC / Dreamcast Harmonics to 1.5 kHz - 24 surgical notches)",
                                ),
                                (
                                    HumFilterMode::HarmonicNotch50Hz,
                                    "Harmonic Buzz Filter (50.00 Hz PAL Harmonics to 1.5 kHz - 30 surgical notches)",
                                ),
                                (
                                    HumFilterMode::SingleNotch60Hz,
                                    "Single 60 Hz Notch (Sub-bass hum only)",
                                ),
                                (
                                    HumFilterMode::SingleNotch50Hz,
                                    "Single 50 Hz Notch (Sub-bass hum only)",
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

            if let Some(spec) = filter.build_filter_spec() {
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(format!("Active pipeline: {}", spec))
                        .small()
                        .monospace()
                        .color(egui::Color32::from_rgb(180, 180, 180)),
                );
            }
    });

    if changed {
        replay.sync_audio_filter();
    }
    changed
}

#[allow(dead_code)]
pub fn draw_audio_filter(replay: &mut Replay, ui: &mut egui::Ui) -> bool {
    draw_audio_filter_group(replay, ui)
}
