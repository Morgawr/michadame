//! PS5 Remote Play Settings Tab & Video-Window Overlays (Virtual Keyboard + Login PIN).

use std::sync::Arc;

use eframe::egui;

use crate::app::AppState;
use crate::config;
use crate::ps5::{
    ffi, Ps5Codec, Ps5Command, Ps5ConnectionMode, Ps5Fps, Ps5Resolution,
    PS5_CONTROLLER_REQUIRED_ERROR, PS5_MAX_BITRATE_KBPS, PS5_MIN_BITRATE_KBPS,
};
use crate::ui::controls::{sub_heading, technical_group, technical_separator, two_columns};

fn repaint_cb(ctx: &egui::Context) -> Option<Arc<dyn Fn() + Send + Sync>> {
    let c = ctx.clone();
    Some(Arc::new(move || c.request_repaint()))
}

pub fn draw_ps5_console_dropdown(
    ui: &mut egui::Ui,
    state: &mut AppState,
    combo_id: &str,
    width: f32,
) -> bool {
    let mut changed = false;
    let selected_label = state
        .ps5
        .selected_console()
        .map(|c| c.display_name())
        .unwrap_or_else(|| "No PS5 Configured".to_string());

    egui::ComboBox::from_id_source(combo_id)
        .selected_text(selected_label)
        .width(width)
        .show_ui(ui, |ui| {
            if state.ps5.config.consoles.is_empty() {
                ui.label(
                    egui::RichText::new("Discover or register a PS5 in the PS5 tab.")
                        .weak()
                        .small(),
                );
            } else {
                for console in &state.ps5.config.consoles {
                    let is_sel = state.ps5.config.selected_console_id == console.id;
                    if ui
                        .selectable_label(is_sel, console.display_name())
                        .clicked()
                    {
                        state.ps5.config.selected_console_id = console.id.clone();
                        changed = true;
                    }
                }
            }
        });

    if changed {
        config::save_config(state);
    }
    changed
}

pub fn draw_ps5_tab(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;
    let ctx = ui.ctx().clone();

    if state.ps5.config.auto_discover_on_tab_open
        && !state.ps5.discovery_auto_triggered
        && !state.ps5.discovery_in_progress
        && ffi::is_backend_compiled()
    {
        state.ps5.discovery_auto_triggered = true;
        state.ps5.start_discovery(repaint_cb(&ctx));
    }

    if !ffi::is_backend_compiled() {
        egui::Frame::none()
            .fill(egui::Color32::from_rgb(65, 22, 22))
            .stroke(egui::Stroke::new(
                1.0_f32,
                egui::Color32::from_rgb(170, 60, 60),
            ))
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .rounding(egui::Rounding::ZERO)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(
                        "PS5 Remote Play backend (libchiaki) was disabled at compile time. Rebuild with default features to enable.",
                    )
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(255, 195, 195)),
                );
            });
        ui.add_space(4.0);
    }

    // 1. Controller & Session Header
    technical_group(
        ui,
        "PS5 Remote Play — DualSense Controller & Connection:",
        |ui| {
            let controller_connected = state.ps5.is_controller_connected();
            ui.horizontal_wrapped(|ui| {
                if controller_connected {
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgb(20, 56, 30))
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            egui::Color32::from_rgb(50, 145, 76),
                        ))
                        .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                        .rounding(egui::Rounding::ZERO)
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "PS5 Controller: {}",
                                    state.ps5.controller_status.short_label()
                                ))
                                .monospace()
                                .size(11.0)
                                .strong()
                                .color(egui::Color32::from_rgb(175, 255, 190)),
                            );
                        });
                } else {
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgb(72, 24, 24))
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            egui::Color32::from_rgb(185, 65, 65),
                        ))
                        .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                        .rounding(egui::Rounding::ZERO)
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(format!("⚠ {PS5_CONTROLLER_REQUIRED_ERROR}"))
                                    .monospace()
                                    .size(11.0)
                                    .strong()
                                    .color(egui::Color32::from_rgb(255, 190, 190)),
                            );
                        });
                }
            });

            ui.add_space(6.0);

            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new("PS5 Console:")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(180, 180, 180)),
                );
                changed |= draw_ps5_console_dropdown(ui, state, "ps5_tab_console_selector", 260.0);

                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Mode:")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(180, 180, 180)),
                );
                egui::ComboBox::from_id_source("ps5_connection_mode")
                    .selected_text(state.ps5.config.connection_mode.label())
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for mode in Ps5ConnectionMode::ALL {
                            if ui
                                .selectable_value(
                                    &mut state.ps5.config.connection_mode,
                                    mode,
                                    mode.label(),
                                )
                                .changed()
                            {
                                config::save_config(state);
                                changed = true;
                            }
                        }
                    });

                ui.add_space(8.0);

                if state.ps5.is_active {
                    let stop_btn = egui::Button::new(
                        egui::RichText::new("Disconnect PS5")
                            .monospace()
                            .size(11.0)
                            .strong()
                            .color(egui::Color32::from_rgb(255, 204, 204)),
                    )
                    .fill(egui::Color32::from_rgb(90, 24, 24))
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        egui::Color32::from_rgb(138, 46, 46),
                    ))
                    .rounding(egui::Rounding::ZERO);
                    if ui.add(stop_btn).clicked() {
                        state.stop_stream(&ctx);
                        changed = true;
                    }

                    if ui
                        .button("PS Home")
                        .on_hover_text("Open PlayStation Home screen")
                        .clicked()
                    {
                        state.ps5.send_command(Ps5Command::GoHome);
                    }
                    if ui
                        .button("Sleep PS5")
                        .on_hover_text("Put the connected PS5 into Rest Mode")
                        .clicked()
                    {
                        state.ps5.send_command(Ps5Command::GotoBed);
                    }
                    if ui
                        .button("Request Keyframe")
                        .on_hover_text("Force PS5 encoder to send a fresh IDR keyframe")
                        .clicked()
                    {
                        state.ps5.send_command(Ps5Command::RequestIdr);
                    }
                } else {
                    let connect_btn = egui::Button::new(
                        egui::RichText::new("Connect to PS5")
                            .monospace()
                            .size(11.0)
                            .strong()
                            .color(if controller_connected {
                                egui::Color32::from_rgb(208, 232, 255)
                            } else {
                                egui::Color32::from_rgb(130, 130, 130)
                            }),
                    )
                    .fill(if controller_connected {
                        egui::Color32::from_rgb(20, 56, 108)
                    } else {
                        egui::Color32::from_rgb(38, 38, 38)
                    })
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        if controller_connected {
                            egui::Color32::from_rgb(58, 120, 210)
                        } else {
                            egui::Color32::from_rgb(65, 65, 65)
                        },
                    ))
                    .rounding(egui::Rounding::ZERO);

                    let resp = ui.add_enabled(
                        controller_connected && ffi::is_backend_compiled(),
                        connect_btn,
                    );
                    let resp = if !controller_connected {
                        resp.on_disabled_hover_text(PS5_CONTROLLER_REQUIRED_ERROR)
                    } else {
                        resp.on_hover_text(
                            "Start PS5 Remote Play session with dedicated PS5 rendering profile",
                        )
                    };
                    if resp.clicked() {
                        let _ = state.start_ps5_mode(&ctx);
                        changed = true;
                    }

                    let selected_can_wake = state
                        .ps5
                        .selected_console()
                        .is_some_and(|c| c.has_local_registration());
                    let wake_resp = ui
                        .add_enabled(
                            selected_can_wake
                                && !state.ps5.wakeup_in_progress
                                && ffi::is_backend_compiled(),
                            egui::Button::new("Wake Up"),
                        )
                        .on_hover_text("Send local LAN Wake-Up packet to wake PS5 from Rest Mode");
                    if wake_resp.clicked() {
                        let id = state
                            .ps5
                            .selected_console()
                            .map(|c| c.id.clone())
                            .unwrap_or_default();
                        if let Err(err) = state.ps5.start_wakeup(&id, repaint_cb(&ctx)) {
                            state.error(err);
                        }
                    }
                }
            });

            if !state.ps5.status_message.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!("Status: {}", state.ps5.status_message))
                        .monospace()
                        .size(10.5)
                        .color(egui::Color32::from_rgb(165, 200, 235)),
                );
            }
        },
    );

    ui.add_space(4.0);

    // 2. Streaming Video & Input Settings
    technical_group(ui, "Streaming Resolution, Bitrate & Input:", |ui| {
        two_columns(ui, |left, right| {
            let ui = left;
            {
                sub_heading(ui, "Video Stream");
                egui::Grid::new("ps5_video_grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Resolution:");
                        egui::ComboBox::from_id_source("ps5_resolution")
                            .selected_text(state.ps5.config.resolution.label())
                            .width(175.0)
                            .show_ui(ui, |ui| {
                                for res in Ps5Resolution::ALL {
                                    if ui
                                        .selectable_value(
                                            &mut state.ps5.config.resolution,
                                            res,
                                            res.label(),
                                        )
                                        .changed()
                                    {
                                        config::save_config(state);
                                        changed = true;
                                    }
                                }
                            });
                        ui.end_row();

                        ui.label("Framerate:");
                        egui::ComboBox::from_id_source("ps5_fps")
                            .selected_text(state.ps5.config.fps.label())
                            .width(175.0)
                            .show_ui(ui, |ui| {
                                for fps in Ps5Fps::ALL {
                                    let selected = state.ps5.config.fps == fps;
                                    if ui.selectable_label(selected, fps.label()).clicked()
                                        && !selected
                                    {
                                        state.ps5.config.set_fps(fps);
                                        if fps != Ps5Fps::Fps30 {
                                            state.ps5_framegen_pacer.reset();
                                        }
                                        config::save_config(state);
                                        changed = true;
                                    }
                                }
                            });
                        ui.end_row();

                        if state.steam_deck.is_none() {
                            state.ps5.config.normalize_framegen();
                            let is_30fps = state.ps5.config.fps == Ps5Fps::Fps30;
                            let beam_crt_enabled = state.effective_ps5_crt_filter()
                                == crate::devices::filter_type::CrtFilter::Beam;
                            let can_toggle = is_30fps && !beam_crt_enabled;

                            ui.label("Frame Gen:");
                            let mut enabled_val = if can_toggle {
                                state.ps5.config.framegen_enabled
                            } else {
                                false
                            };
                            let checkbox = ui.add_enabled(
                                can_toggle,
                                egui::Checkbox::new(
                                    &mut enabled_val,
                                    "AMD FidelityFX (30 → 60 FPS)",
                                ),
                            );
                            let checkbox = if !is_30fps {
                                checkbox.on_disabled_hover_text(
                                    "Only available when PS5 stream framerate is set to 30 FPS (automatically disabled at 60 FPS).",
                                )
                            } else if beam_crt_enabled {
                                checkbox.on_disabled_hover_text(
                                    "Disabled while the CRT Beam filter is enabled. Switch CRT filter to Off, Lottes, or Halo to use AMD FidelityFX Frame Gen.",
                                )
                            } else {
                                checkbox.on_hover_text(
                                    "Uses AMD FidelityFX Optical Flow v5 on the GPU to interpolate explicit 30 FPS PS5 streams to 60 FPS (works with CRT Off, Lottes, or Halo; adds ~17 ms latency; disabled when CRT Beam filter is enabled).",
                                )
                            };
                            if can_toggle && checkbox.changed() {
                                state.ps5.config.framegen_enabled = enabled_val;
                                if !enabled_val {
                                    state.ps5_framegen_pacer.reset();
                                }
                                config::save_config(state);
                                changed = true;
                            }
                            ui.end_row();

                            if can_toggle && state.ps5.config.framegen_enabled {
                                ui.label("Artifact Guard:");
                                if ui
                                    .add(
                                        egui::Slider::new(
                                            &mut state.ps5.config.framegen_artifact_guard,
                                            0.0..=1.0,
                                        )
                                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                                    )
                                    .on_hover_text(
                                        "Strictness of forward/backward optical flow consistency & fallback on unmatchable regions (particles, fast whips, transparency). Higher = cleaner with fewer warp glitches.",
                                    )
                                    .changed()
                                {
                                    config::save_config(state);
                                    changed = true;
                                }
                                ui.end_row();

                                ui.label("Scene Cut Sens.:");
                                if ui
                                    .add(
                                        egui::Slider::new(
                                            &mut state.ps5.config.framegen_scd_sensitivity,
                                            0.0..=1.0,
                                        )
                                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                                    )
                                    .on_hover_text(
                                        "Scene Change Detection sensitivity. Higher values bypass interpolation more readily on camera cuts, menu flashes, or rapid scene whips.",
                                    )
                                    .changed()
                                {
                                    config::save_config(state);
                                    changed = true;
                                }
                                ui.end_row();

                                ui.label("HUD Protection:");
                                ui.horizontal(|ui| {
                                    if ui
                                        .add(
                                            egui::Slider::new(
                                                &mut state.ps5.config.framegen_hud_protection,
                                                0.0..=1.0,
                                            )
                                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                                        )
                                        .on_hover_text(
                                            "Locks stationary UI, subtitles, and Japanese dialogue boxes against warping during camera movement.",
                                        )
                                        .changed()
                                    {
                                        config::save_config(state);
                                        changed = true;
                                    }
                                    if ui
                                        .small_button("Reset")
                                        .on_hover_text("Restore default FidelityFX Frame Gen tuning")
                                        .clicked()
                                    {
                                        state.ps5.config.reset_framegen_params();
                                        config::save_config(state);
                                        changed = true;
                                    }
                                });
                                ui.end_row();
                            }
                        }

                        ui.label("Codec:");
                        egui::ComboBox::from_id_source("ps5_codec")
                            .selected_text(state.ps5.config.codec.label())
                            .width(175.0)
                            .show_ui(ui, |ui| {
                                for codec in Ps5Codec::ALL {
                                    if ui
                                        .selectable_value(
                                            &mut state.ps5.config.codec,
                                            codec,
                                            codec.label(),
                                        )
                                        .changed()
                                    {
                                        config::save_config(state);
                                        changed = true;
                                    }
                                }
                            });
                        ui.end_row();

                        ui.label("Audio Buffer:");
                        if ui
                            .add(
                                egui::Slider::new(
                                    &mut state.ps5.config.audio_buffer_ms,
                                    crate::ps5::PS5_MIN_AUDIO_BUFFER_MS
                                        ..=crate::ps5::PS5_MAX_AUDIO_BUFFER_MS,
                                )
                                .step_by(5.0)
                                .suffix(" ms"),
                            )
                            .on_hover_text(
                                "Target audio jitter buffer (100 ms default). Increase for Wi-Fi/mesh links; updates live while streaming and auto-expands up to +60 ms if underruns occur.",
                            )
                            .changed()
                        {
                            let clamped = state.ps5.config.clamped_audio_buffer_ms();
                            state
                                .ps5
                                .audio_telemetry
                                .target_buffer_ms
                                .store(clamped, std::sync::atomic::Ordering::Relaxed);
                            config::save_config(state);
                            changed = true;
                        }
                        ui.end_row();
                    });
            }
            let ui = right;
            {
                sub_heading(ui, "Bitrate & DualSense");
                ui.horizontal(|ui| {
                    ui.label("Bitrate (kbps):");
                    if ui
                        .add(
                            egui::Slider::new(
                                &mut state.ps5.config.bitrate_kbps,
                                PS5_MIN_BITRATE_KBPS..=PS5_MAX_BITRATE_KBPS,
                            )
                            .step_by(500.0),
                        )
                        .changed()
                    {
                        config::save_config(state);
                        changed = true;
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("Presets:").weak().small());
                    for (label, kbps) in [
                        ("6 Mbps", 6_000u32),
                        ("15 Mbps", 15_000),
                        ("30 Mbps", 30_000),
                        ("50 Mbps", 50_000),
                        (
                            "Auto for Res",
                            state.ps5.config.resolution.recommended_bitrate_kbps(),
                        ),
                    ] {
                        if ui.small_button(label).clicked() {
                            state.ps5.config.bitrate_kbps = kbps;
                            config::save_config(state);
                            changed = true;
                        }
                    }
                });

                ui.add_space(6.0);
                if ui
                    .checkbox(
                        &mut state.ps5.config.enable_keyboard,
                        "Enable PS5 Keyboard Input Prompts",
                    )
                    .on_hover_text(
                        "Opens a live text input overlay whenever the PS5 displays an on-screen keyboard.",
                    )
                    .changed()
                {
                    config::save_config(state);
                    changed = true;
                }
                if ui
                    .checkbox(
                        &mut state.ps5.config.enable_dualsense_haptics,
                        "Enable DualSense Haptics, Rumble & Adaptive Triggers",
                    )
                    .on_hover_text(
                        "Forwards PS5 DualSense USB audio haptics, rumble, and adaptive trigger resistance directly to your DualSense controller (microphone disabled).",
                    )
                    .changed()
                {
                    config::save_config(state);
                    changed = true;
                }
                if ui
                    .checkbox(
                        &mut state.ps5.config.auto_go_home_on_cant_display,
                        "Auto-return to Home Screen on protected content",
                    )
                    .on_hover_text(
                        "Automatically sends the PS5 back to the Home Screen whenever a protected / non-streamable screen is detected (otherwise shows a 'Go to Home Screen' prompt).",
                    )
                    .changed()
                {
                    config::save_config(state);
                    changed = true;
                }
                if ui
                    .checkbox(
                        &mut state.ps5.config.auto_discover_on_tab_open,
                        "Auto-discover local PS5s on tab open",
                    )
                    .changed()
                {
                    config::save_config(state);
                    changed = true;
                }
            }
        });
    });

    ui.add_space(4.0);

    // 3. Local Network Discovery & Console Registration (PIN)
    technical_group(
        ui,
        "Local Network Discovery & Console Registration:",
        |ui| {
            two_columns(ui, |left, right| {
                let ui = left;
                {
                    sub_heading(ui, "Discover Local PS5s (LAN)");
                    ui.horizontal(|ui| {
                        let btn_label = if state.ps5.discovery_in_progress {
                            "Scanning LAN..."
                        } else {
                            "Discover Local PS5s"
                        };
                        if ui
                            .add_enabled(
                                !state.ps5.discovery_in_progress && ffi::is_backend_compiled(),
                                egui::Button::new(btn_label),
                            )
                            .clicked()
                        {
                            state.ps5.start_discovery(repaint_cb(&ctx));
                        }
                    });
                    if !state.ps5.discovered_consoles.is_empty() {
                        ui.add_space(4.0);
                        let discovered = state.ps5.discovered_consoles.clone();
                        for host in &discovered {
                            egui::Frame::none()
                                .fill(egui::Color32::from_rgb(20, 20, 20))
                                .stroke(egui::Stroke::new(
                                    1.0_f32,
                                    egui::Color32::from_rgb(52, 52, 52),
                                ))
                                .inner_margin(egui::Margin::symmetric(8.0, 5.0))
                                .rounding(egui::Rounding::ZERO)
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal_wrapped(|ui| {
                                        let title = if host.host_name.is_empty() {
                                            "PlayStation 5"
                                        } else {
                                            &host.host_name
                                        };
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{title} ({})",
                                                host.host_addr
                                            ))
                                            .monospace()
                                            .strong()
                                            .size(11.0),
                                        );
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "[{}]",
                                                host.state.label()
                                            ))
                                            .monospace()
                                            .size(10.5)
                                            .color(
                                                match host.state {
                                                    crate::ps5::Ps5HostPowerState::Ready => {
                                                        egui::Color32::from_rgb(130, 235, 145)
                                                    }
                                                    crate::ps5::Ps5HostPowerState::Standby => {
                                                        egui::Color32::from_rgb(255, 205, 110)
                                                    }
                                                    crate::ps5::Ps5HostPowerState::Unknown => {
                                                        egui::Color32::from_rgb(160, 160, 160)
                                                    }
                                                },
                                            ),
                                        );
                                    });
                                    if !host.running_app_name.is_empty() {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Running: {}",
                                                host.running_app_name
                                            ))
                                            .weak()
                                            .small(),
                                        );
                                    }
                                    ui.horizontal(|ui| {
                                        if ui.small_button("Use IP for PIN Registration").clicked()
                                        {
                                            state.ps5.regist_host_input = host.host_addr.clone();
                                            state.ps5.regist_broadcast = false;
                                        }
                                        if let Some(saved) = state
                                            .ps5
                                            .config
                                            .consoles
                                            .iter()
                                            .find(|c| c.host.eq_ignore_ascii_case(&host.host_addr))
                                            .map(|c| c.id.clone())
                                        {
                                            if ui.small_button("Select Console").clicked() {
                                                state.ps5.config.selected_console_id = saved;
                                                config::save_config(state);
                                                changed = true;
                                            }
                                        }
                                    });
                                });
                            ui.add_space(3.0);
                        }
                    }
                }
                let ui = right;
                {
                    sub_heading(ui, "Register PS5 via 8-Digit PIN (Local LAN)");

                    egui::Grid::new("ps5_regist_grid")
                        .num_columns(2)
                        .spacing([8.0, 5.0])
                        .show(ui, |ui| {
                            ui.label("PS5 IP / Host:");
                            ui.horizontal(|ui| {
                                ui.add_sized(
                                    [150.0, 20.0],
                                    egui::TextEdit::singleline(&mut state.ps5.regist_host_input)
                                        .hint_text("192.168.1.50"),
                                );
                                ui.checkbox(&mut state.ps5.regist_broadcast, "Broadcast");
                            });
                            ui.end_row();

                            ui.label("8-Digit Link PIN:");
                            ui.add_sized(
                                [150.0, 20.0],
                                egui::TextEdit::singleline(&mut state.ps5.regist_pin_input)
                                    .hint_text("12345678"),
                            )
                            .on_hover_text("On PS5: Settings → System → Remote Play → Link Device");
                            ui.end_row();

                            ui.label("PSN Account-ID:");
                            if ui
                                .add_sized(
                                    [210.0, 20.0],
                                    egui::TextEdit::singleline(
                                        &mut state.ps5.config.psn_account_id_b64,
                                    )
                                    .hint_text("Auto-filled by PSN login (8-byte Base64)"),
                                )
                                .changed()
                            {
                                config::save_config(state);
                                changed = true;
                            }
                            ui.end_row();

                            ui.label("Login PIN (opt.):");
                            ui.add_sized(
                                [150.0, 20.0],
                                egui::TextEdit::singleline(&mut state.ps5.regist_console_pin_input)
                                    .password(true)
                                    .hint_text("4-digit PS5 passcode"),
                            );
                            ui.end_row();
                        });

                    ui.add_space(6.0);
                    let reg_label = if state.ps5.regist_in_progress {
                        "Registering..."
                    } else {
                        "Register PS5 via PIN"
                    };
                    if ui
                        .add_enabled(
                            !state.ps5.regist_in_progress && ffi::is_backend_compiled(),
                            egui::Button::new(reg_label),
                        )
                        .clicked()
                    {
                        if let Err(err) = state.ps5.start_local_registration(repaint_cb(&ctx)) {
                            state.error(err);
                        }
                    }
                }
            });

            technical_separator(ui);

            // Saved / Registered Consoles list + Add manual host
            sub_heading(ui, "Saved / Registered PS5 Consoles");
            if state.ps5.config.consoles.is_empty() {
                ui.label(egui::RichText::new("None").weak().small());
            } else {
                let mut remove_id: Option<String> = None;
                let mut wake_id: Option<String> = None;
                let mut select_id: Option<String> = None;
                let mut console_edited = false;

                for console in &mut state.ps5.config.consoles {
                    let is_selected = state.ps5.config.selected_console_id == console.id;
                    egui::Frame::none()
                        .fill(if is_selected {
                            egui::Color32::from_rgb(24, 34, 48)
                        } else {
                            egui::Color32::from_rgb(20, 20, 20)
                        })
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            if is_selected {
                                egui::Color32::from_rgb(65, 118, 185)
                            } else {
                                egui::Color32::from_rgb(50, 50, 50)
                            },
                        ))
                        .inner_margin(egui::Margin::symmetric(8.0, 5.0))
                        .rounding(egui::Rounding::ZERO)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal_wrapped(|ui| {
                                if ui.selectable_label(is_selected, "Active").clicked() {
                                    select_id = Some(console.id.clone());
                                }
                                ui.label("Name:");
                                if ui
                                    .add_sized(
                                        [140.0, 20.0],
                                        egui::TextEdit::singleline(&mut console.nickname),
                                    )
                                    .changed()
                                {
                                    console_edited = true;
                                }
                                ui.label("LAN IP:");
                                if ui
                                    .add_sized(
                                        [120.0, 20.0],
                                        egui::TextEdit::singleline(&mut console.host)
                                            .hint_text("192.168.x.x"),
                                    )
                                    .changed()
                                {
                                    console_edited = true;
                                }
                                ui.label("PIN:");
                                if ui
                                    .add_sized(
                                        [52.0, 20.0],
                                        egui::TextEdit::singleline(&mut console.console_pin)
                                            .password(true)
                                            .hint_text("0000"),
                                    )
                                    .on_hover_text("Optional 4-digit PS5 user login passcode")
                                    .changed()
                                {
                                    console_edited = true;
                                }

                                if console.has_local_registration() {
                                    ui.label(
                                        egui::RichText::new("[LAN Paired]")
                                            .monospace()
                                            .size(10.0)
                                            .color(egui::Color32::from_rgb(130, 235, 145)),
                                    );
                                }
                                if console.has_psn_target() {
                                    ui.label(
                                        egui::RichText::new("[PSN Linked]")
                                            .monospace()
                                            .size(10.0)
                                            .color(egui::Color32::from_rgb(130, 195, 255)),
                                    );
                                }

                                if console.has_local_registration()
                                    && ui.small_button("Wake").clicked()
                                {
                                    wake_id = Some(console.id.clone());
                                }
                                if ui.small_button("Delete").clicked() {
                                    remove_id = Some(console.id.clone());
                                }
                            });
                        });
                    ui.add_space(3.0);
                }

                if let Some(id) = select_id {
                    state.ps5.config.selected_console_id = id;
                    console_edited = true;
                }
                if let Some(id) = remove_id {
                    state.ps5.remove_console(&id);
                    console_edited = true;
                }
                if let Some(id) = wake_id {
                    if let Err(err) = state.ps5.start_wakeup(&id, repaint_cb(&ctx)) {
                        state.error(err);
                    }
                }
                if console_edited {
                    config::save_config(state);
                    changed = true;
                }
            }

            ui.add_space(4.0);
            ui.collapsing("Add Manual PS5 Host Entry", |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Nickname:");
                    ui.add_sized(
                        [140.0, 20.0],
                        egui::TextEdit::singleline(&mut state.ps5.manual_nickname_input)
                            .hint_text("Living Room PS5"),
                    );
                    ui.label("IP / Hostname:");
                    ui.add_sized(
                        [150.0, 20.0],
                        egui::TextEdit::singleline(&mut state.ps5.manual_host_input)
                            .hint_text("192.168.1.50"),
                    );
                    if ui.button("Add").clicked() {
                        match state.ps5.add_manual_console() {
                            Ok(_) => {
                                config::save_config(state);
                                state.info(
                                    "Added manual PS5 host (register via PIN to stream on LAN).",
                                );
                                changed = true;
                            }
                            Err(err) => state.error(err),
                        }
                    }
                });
            });
        },
    );

    ui.add_space(4.0);

    // 4. PlayStation Network (PSN) Account & Remote Play
    technical_group(
        ui,
        "PlayStation Network (PSN) Account & Remote Play:",
        |ui| {
            if let Some(tok) = state.ps5.psn_token.clone() {
                ui.horizontal_wrapped(|ui| {
                    let who = if !tok.online_id.is_empty() {
                        format!("{} (Account-ID: {})", tok.online_id, tok.account_id_b64)
                    } else {
                        format!(
                            "Account-ID: {} (User ID: {})",
                            tok.account_id_b64, tok.user_id
                        )
                    };
                    ui.label(
                        egui::RichText::new(format!("Signed in to PSN — {who}"))
                            .monospace()
                            .size(11.0)
                            .strong()
                            .color(egui::Color32::from_rgb(145, 220, 255)),
                    );

                    let refresh_label = if state.ps5.psn_devices_in_progress {
                        "Refreshing PSN..."
                    } else {
                        "Refresh PSN Consoles"
                    };
                    if ui
                        .add_enabled(
                            !state.ps5.psn_devices_in_progress && ffi::is_backend_compiled(),
                            egui::Button::new(refresh_label),
                        )
                        .clicked()
                    {
                        if let Err(err) = state.ps5.start_psn_device_refresh(repaint_cb(&ctx)) {
                            state.error(err);
                        }
                    }

                    if ui.button("Sign Out").clicked() {
                        state.ps5.sign_out_psn();
                        state.info("Signed out of PlayStation Network.");
                        changed = true;
                    }
                });

                if !state.ps5.psn_devices.is_empty() {
                    ui.add_space(4.0);
                    let devices = state.ps5.psn_devices.clone();
                    for dev in &devices {
                        egui::Frame::none()
                            .fill(egui::Color32::from_rgb(20, 20, 20))
                            .stroke(egui::Stroke::new(
                                1.0_f32,
                                egui::Color32::from_rgb(52, 52, 52),
                            ))
                            .inner_margin(egui::Margin::symmetric(8.0, 5.0))
                            .rounding(egui::Rounding::ZERO)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal_wrapped(|ui| {
                                    let name = if dev.device_name.is_empty() {
                                        "PlayStation 5"
                                    } else {
                                        &dev.device_name
                                    };
                                    ui.label(
                                        egui::RichText::new(name)
                                            .monospace()
                                            .strong()
                                            .size(11.0),
                                    );
                                    ui.label(
                                        egui::RichText::new(if dev.remoteplay_enabled {
                                            "[Remote Play Enabled]"
                                        } else {
                                            "[Remote Play Disabled]"
                                        })
                                        .monospace()
                                        .size(10.0)
                                        .color(if dev.remoteplay_enabled {
                                            egui::Color32::from_rgb(130, 235, 145)
                                        } else {
                                            egui::Color32::from_rgb(255, 150, 120)
                                        }),
                                    );

                                    if let Some(saved_id) = state
                                        .ps5
                                        .config
                                        .consoles
                                        .iter()
                                        .find(|c| {
                                            c.psn_duid_hex.eq_ignore_ascii_case(&dev.device_uid_hex)
                                        })
                                        .map(|c| c.id.clone())
                                    {
                                        if ui.small_button("Select").clicked() {
                                            state.ps5.config.selected_console_id = saved_id;
                                            config::save_config(state);
                                            changed = true;
                                        }
                                    }

                                    if ui
                                        .add_enabled(
                                            !state.ps5.regist_in_progress
                                                && ffi::is_backend_compiled(),
                                            egui::Button::new("Auto-Register via PSN").small(),
                                        )
                                        .on_hover_text(
                                            "Automatically pair with this PS5 over PSN without entering an 8-digit PIN",
                                        )
                                        .clicked()
                                    {
                                        if let Err(err) = state.ps5.start_psn_auto_registration(
                                            &dev.device_uid_hex,
                                            name,
                                            repaint_cb(&ctx),
                                        ) {
                                            state.error(err);
                                        }
                                    }
                                });
                            });
                        ui.add_space(3.0);
                    }
                }
            } else {
                if state.ps5.psn_login_url.is_empty() {
                    if ui.button("Start PSN Login").clicked() {
                        match state.ps5.start_psn_login_flow() {
                            Ok(url) => {
                                let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
                                state.info(
                                    "Opened Sony PSN login in browser. After signing in, paste the final redirect URL below.",
                                );
                            }
                            Err(err) => state.error(err),
                        }
                    }
                } else {
                    ui.label(
                        egui::RichText::new(
                            "1. Sign in to Sony in your browser until the page displays \"redirect\" (blank/redirect page).",
                        )
                        .monospace()
                        .size(10.5),
                    );
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Open Login Page in Browser").clicked() {
                            let _ = std::process::Command::new("xdg-open")
                                .arg(&state.ps5.psn_login_url)
                                .spawn();
                        }
                        if ui.button("Copy Login URL").clicked() {
                            ctx.output_mut(|o| {
                                o.copied_text = state.ps5.psn_login_url.clone();
                            });
                            state.info("Copied Sony PSN login URL to clipboard.");
                        }
                        if ui.button("Cancel").clicked() {
                            state.ps5.psn_login_url.clear();
                            state.ps5.psn_redirect_input.clear();
                        }
                    });

                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(
                            "2. Copy the ENTIRE redirect URL from your browser's address bar and paste it here:",
                        )
                        .monospace()
                        .size(10.5),
                    );
                    ui.horizontal(|ui| {
                        let avail = (ui.available_width() - 190.0).clamp(120.0, 680.0);
                        ui.add_sized(
                            [avail, 22.0],
                            egui::TextEdit::singleline(&mut state.ps5.psn_redirect_input)
                                .hint_text(
                                    "https://remoteplay.dl.playstation.net/remoteplay/redirect?code=...",
                                ),
                        );
                        let submit_label = if state.ps5.psn_auth_in_progress {
                            "Signing In..."
                        } else {
                            "Complete PSN Sign-In"
                        };
                        if ui
                            .add_enabled(
                                !state.ps5.psn_auth_in_progress
                                    && !state.ps5.psn_redirect_input.trim().is_empty(),
                                egui::Button::new(submit_label),
                            )
                            .clicked()
                        {
                            if let Err(err) = state.ps5.start_psn_code_exchange(repaint_cb(&ctx)) {
                                state.error(err);
                            }
                        }
                    });
                }
            }
        },
    );

    changed
}

/// Renders PS5 Remote Play overlays on top of the active video window:
/// - Connecting status badge (local-only preview)
/// - Controller-disconnected / blocked-scene notices (local-only preview)
/// - PS5 Virtual Keyboard live typing prompt (Enter to submit, Esc to cancel)
/// - PS5 4-digit Login PIN prompt
pub fn draw_ps5_overlays(state: &mut AppState, ctx: &egui::Context) {
    if !state.ps5.is_active {
        return;
    }

    // Connecting banner (before first frames arrive)
    if state.ps5.is_connecting {
        egui::Area::new(egui::Id::new("ps5_connecting_overlay"))
            .order(egui::Order::Debug)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(egui::Color32::from_rgba_unmultiplied(16, 20, 28, 225))
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        egui::Color32::from_rgb(72, 130, 215),
                    ))
                    .inner_margin(egui::Margin::symmetric(16.0, 10.0))
                    .rounding(egui::Rounding::ZERO)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(
                                egui::RichText::new(if state.ps5.status_message.is_empty() {
                                    "Connecting to PS5 Remote Play..."
                                } else {
                                    &state.ps5.status_message
                                })
                                .monospace()
                                .size(13.0)
                                .strong()
                                .color(egui::Color32::from_rgb(220, 235, 255)),
                            );
                        });
                    });
            });
    }

    // Unobtrusive top-center warning badge if the DualSense is unplugged mid-stream
    if !state.ps5.is_controller_connected() {
        egui::Area::new(egui::Id::new("ps5_warning_overlay"))
            .order(egui::Order::Debug)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 14.0))
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(egui::Color32::from_rgba_unmultiplied(80, 22, 22, 225))
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        egui::Color32::from_rgb(200, 72, 72),
                    ))
                    .inner_margin(egui::Margin::symmetric(12.0, 6.0))
                    .rounding(egui::Rounding::ZERO)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(format!("⚠ {PS5_CONTROLLER_REQUIRED_ERROR}"))
                                .monospace()
                                .size(11.5)
                                .strong()
                                .color(egui::Color32::from_rgb(255, 215, 215)),
                        );
                    });
            });
    }

    // Centered Protected Content ("can't display") overlay with "Go to Home Screen" button
    if state.ps5.cant_display_warning && !state.ps5.is_modal_prompt_open() {
        let mut go_home_clicked = false;
        let mut auto_toggle_changed = false;

        if let Some(inner) = egui::Window::new("Protected Content")
            .id(egui::Id::new("ps5_cant_display_modal"))
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .collapsible(false)
            .resizable(false)
            .min_width(420.0)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(
                            "The screen contains content that can't be displayed using Remote Play.",
                        )
                        .monospace()
                        .size(12.5)
                        .strong()
                        .color(egui::Color32::from_rgb(255, 225, 200)),
                    );
                    ui.add_space(10.0);

                    let home_btn = egui::Button::new(
                        egui::RichText::new("Go to Home Screen (Enter)")
                            .monospace()
                            .size(12.0)
                            .strong()
                            .color(egui::Color32::from_rgb(220, 240, 255)),
                    )
                    .fill(egui::Color32::from_rgb(22, 64, 122))
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        egui::Color32::from_rgb(72, 140, 230),
                    ))
                    .rounding(egui::Rounding::ZERO);

                    if ui.add_sized([240.0, 32.0], home_btn).clicked() {
                        go_home_clicked = true;
                    }

                    ui.add_space(8.0);
                    if ui
                        .checkbox(
                            &mut state.ps5.config.auto_go_home_on_cant_display,
                            "Automatically return to Home Screen when protected content appears",
                        )
                        .changed()
                    {
                        auto_toggle_changed = true;
                    }
                    ui.add_space(2.0);
                });

                if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    go_home_clicked = true;
                }
            })
        {
            ctx.move_to_top(inner.response.layer_id);
        }

        if auto_toggle_changed {
            config::save_config(state);
            if state.ps5.config.auto_go_home_on_cant_display {
                go_home_clicked = true;
            }
        }
        if go_home_clicked {
            state.ps5.send_command(Ps5Command::GoHome);
            state.info("Returning PS5 to Home Screen...");
            ctx.request_repaint();
        }
    }

    // PS5 Virtual Keyboard Input Overlay
    if state.ps5.keyboard_prompt_open {
        let mut accept_clicked = false;
        let mut cancel_clicked = false;
        let mut text_changed = false;

        if let Some(inner) = egui::Window::new("PS5 Keyboard Input")
            .id(egui::Id::new("ps5_keyboard_modal"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -36.0))
            .collapsible(false)
            .resizable(false)
            .min_width(440.0)
            .show(ctx, |ui| {
                let edit_resp = ui.add_sized(
                    [420.0, 26.0],
                    egui::TextEdit::singleline(&mut state.ps5.keyboard_text)
                        .hint_text("Enter text for PS5..."),
                );

                if state.ps5.keyboard_needs_focus {
                    edit_resp.request_focus();
                    state.ps5.keyboard_needs_focus = false;
                }
                if edit_resp.changed() {
                    text_changed = true;
                }

                if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    accept_clicked = true;
                } else if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel_clicked = true;
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Confirm (Enter)").clicked() {
                        accept_clicked = true;
                    }
                    if ui.button("Cancel (Esc)").clicked() {
                        cancel_clicked = true;
                    }
                });
            })
        {
            ctx.move_to_top(inner.response.layer_id);
        }

        if text_changed {
            state
                .ps5
                .send_command(Ps5Command::KeyboardSetText(state.ps5.keyboard_text.clone()));
        }
        if accept_clicked {
            state
                .ps5
                .send_command(Ps5Command::KeyboardSetText(state.ps5.keyboard_text.clone()));
            state.ps5.send_command(Ps5Command::KeyboardAccept);
            state.ps5.keyboard_prompt_open = false;
            ctx.request_repaint();
        } else if cancel_clicked {
            state.ps5.send_command(Ps5Command::KeyboardReject);
            state.ps5.keyboard_prompt_open = false;
            ctx.request_repaint();
        }
    }

    // PS5 4-Digit Login Passcode Prompt
    if state.ps5.login_pin_prompt_open {
        let mut submit_pin = false;
        let mut cancel_pin = false;

        if let Some(inner) = egui::Window::new("PS5 User Login PIN")
            .id(egui::Id::new("ps5_login_pin_modal"))
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .collapsible(false)
            .resizable(false)
            .min_width(300.0)
            .show(ctx, |ui| {
                if state.ps5.login_pin_incorrect {
                    ui.label(
                        egui::RichText::new("Incorrect PIN — please try again.")
                            .monospace()
                            .color(egui::Color32::from_rgb(255, 130, 130)),
                    );
                } else {
                    ui.label("Enter your 4-digit PS5 user login passcode:");
                }
                ui.add_space(4.0);

                let edit_resp = ui.add_sized(
                    [160.0, 24.0],
                    egui::TextEdit::singleline(&mut state.ps5.login_pin_input)
                        .password(true)
                        .hint_text("4-digit PIN"),
                );
                if state.ps5.login_pin_needs_focus {
                    edit_resp.request_focus();
                    state.ps5.login_pin_needs_focus = false;
                }

                // Keep only ASCII digits, max 4
                state.ps5.login_pin_input.retain(|c| c.is_ascii_digit());
                if state.ps5.login_pin_input.len() > 4 {
                    state.ps5.login_pin_input.truncate(4);
                }

                if state.ps5.login_pin_input.len() == 4
                    && ctx.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    submit_pin = true;
                } else if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel_pin = true;
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            state.ps5.login_pin_input.len() == 4,
                            egui::Button::new("Submit PIN"),
                        )
                        .clicked()
                    {
                        submit_pin = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel_pin = true;
                    }
                });
            })
        {
            ctx.move_to_top(inner.response.layer_id);
        }

        if submit_pin {
            let pin = state.ps5.login_pin_input.clone();
            state.ps5.send_command(Ps5Command::SetLoginPin(pin));
            state.ps5.login_pin_prompt_open = false;
            state.ps5.login_pin_incorrect = false;
            ctx.request_repaint();
        } else if cancel_pin {
            state.ps5.login_pin_prompt_open = false;
            state.stop_stream(ctx);
        }
    }
}
