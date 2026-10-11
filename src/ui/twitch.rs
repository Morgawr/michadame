//! "Twitch" settings tab.

use super::controls::{percent_slider_range, slider_item, sub_heading, technical_group};
use crate::app::AppState;
use crate::config::models::{StreamingAudioEncoder, StreamingVideoEncoder};
use crate::twitch::{normalize_channel, AuthFlow, BroadcastStatus, ConnectionStatus, StreamStatus};
use eframe::egui;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const MUTED: egui::Color32 = egui::Color32::from_rgb(119, 119, 119);
const LABEL: egui::Color32 = egui::Color32::from_rgb(153, 153, 153);

fn label(ui: &mut egui::Ui, text: &str) {
    ui.add_sized(
        [130.0, 18.0],
        egui::Label::new(egui::RichText::new(text).monospace().size(11.0).color(LABEL)),
    );
}

fn note(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(egui::RichText::new(text.into()).size(10.5).color(MUTED));
}

pub fn draw_twitch_tab(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;
    let mut save_now = false;

    technical_group(ui, "Twitch Channel:", |ui| {
        ui.horizontal(|ui| {
            label(ui, "Channel:");
            let edit = ui.add(
                egui::TextEdit::singleline(&mut state.twitch.channel_input)
                    .hint_text("your_channel_name")
                    .desired_width(240.0),
            );
            let apply = ui.button("Apply").clicked() || edit.lost_focus();
            if apply {
                let normalized = normalize_channel(&state.twitch.channel_input);
                state.twitch.channel_input = normalized.clone();
                if normalized != state.twitch.config.channel {
                    state.twitch.config.channel = normalized;
                    save_now = true;
                }
            }
        });
        note(ui, "Your Twitch login name or channel URL (e.g. twitch.tv/name).");
        ui.add_space(2.0);
        let (color, text) = status_line(state);
        ui.label(egui::RichText::new(text).monospace().size(11.0).color(color));
        if state.twitch.config.chat_overlay_enabled && !state.twitch.config.channel.is_empty() {
            let (color, text) = match &state.twitch.stream_status {
                StreamStatus::Live => (egui::Color32::from_rgb(80, 200, 120), "Stream: LIVE".to_string()),
                StreamStatus::Offline => (egui::Color32::from_rgb(220, 70, 70), "Stream: offline".to_string()),
                StreamStatus::Unknown(reason) if reason.is_empty() => (MUTED, "Stream: unknown".to_string()),
                StreamStatus::Unknown(reason) => (MUTED, format!("Stream: unknown ({reason})")),
            };
            ui.label(egui::RichText::new(text).monospace().size(11.0).color(color));
        }
    });

    ui.add_space(4.0);

    changed |= draw_broadcast_group(ui, state, &mut save_now);

    ui.add_space(4.0);

    technical_group(ui, "Chat Overlay:", |ui| {
        if ui
            .checkbox(
                &mut state.twitch.config.chat_overlay_enabled,
                "Show Twitch chat overlay on the video window",
            )
            .on_hover_text(
                "Shows live chat along the right edge of the video window, above the CRT frame and bezel.\nPress T in the video window to hide/show it.",
            )
            .changed()
        {
            if state.twitch.config.chat_overlay_enabled {
                state.twitch.overlay_hidden = false;
            }
            save_now = true;
        }
        if state.twitch.config.chat_overlay_enabled && state.twitch.overlay_hidden {
            ui.horizontal(|ui| {
                note(ui, "The overlay is hidden from the live view.");
                if ui.small_button("Show overlay").clicked() {
                    state.twitch.overlay_hidden = false;
                }
            });
        }

        ui.add_space(4.0);
        let cfg = &mut state.twitch.config;
        let mut dirty = false;
        dirty |= slider_item(
            ui,
            "Width:",
            percent_slider_range(&mut cfg.overlay_width_pct, 0.10..=0.50, 1.0),
            Some("Overlay width as a share of the video window width."),
        )
        .changed();
        dirty |= slider_item(
            ui,
            "Background:",
            percent_slider_range(&mut cfg.overlay_opacity, 0.0..=1.0, 1.0),
            Some("Opacity of the dark overlay background."),
        )
        .changed();
        dirty |= slider_item(
            ui,
            "Font Size:",
            egui::Slider::new(&mut cfg.font_size, 10.0..=28.0).step_by(1.0).suffix(" pt"),
            None,
        )
        .changed();
        dirty |= slider_item(
            ui,
            "Message Lifetime:",
            egui::Slider::new(&mut cfg.message_lifetime_secs, 0..=600).custom_formatter(|n, _| {
                if n < 1.0 {
                    "Never".to_string()
                } else {
                    format!("{n:.0}s")
                }
            }),
            Some("Messages fade out and disappear after this long.\n• 0: keep messages until pushed out by newer ones"),
        )
        .changed();
        dirty |= slider_item(
            ui,
            "Max Messages:",
            egui::Slider::new(&mut cfg.max_messages, 20..=500),
            Some("Oldest messages are dropped beyond this count."),
        )
        .changed();
        if dirty {
            state.twitch.config_dirty = true;
            changed = true;
        }
    });

    ui.add_space(4.0);

    technical_group(ui, "Niconico Comments:", |ui| {
        if ui
            .checkbox(
                &mut state.twitch.config.niconico_enabled,
                "Fly chat messages across the video (niconico style)",
            )
            .on_hover_text(
                "Every new chat message scrolls across the picture from right to left.\nComments are drawn under the CRT effects and are included in replays and mining screenshots.",
            )
            .changed()
        {
            save_now = true;
        }
        ui.add_space(4.0);
        let cfg = &mut state.twitch.config;
        let mut dirty = false;
        dirty |= slider_item(
            ui,
            "Text Size:",
            percent_slider_range(&mut cfg.niconico_size_pct, 0.03..=0.15, 1.0),
            Some("Comment height as a share of the picture height."),
        )
        .changed();
        dirty |= slider_item(
            ui,
            "Crossing Time:",
            egui::Slider::new(&mut cfg.niconico_duration_secs, 3.0..=15.0)
                .step_by(0.5)
                .suffix(" s"),
            Some("Average time for a comment to cross the screen. Each comment gets a random speed around this."),
        )
        .changed();
        dirty |= slider_item(
            ui,
            "Max On Screen:",
            egui::Slider::new(&mut cfg.niconico_max_comments, 0..=500).custom_formatter(|n, _| {
                if n < 1.0 {
                    "Unlimited".to_string()
                } else {
                    format!("{n:.0}")
                }
            }),
            Some("Most comments flying at once; the oldest are dropped beyond this.\n• 0: unlimited"),
        )
        .changed();
        if dirty {
            state.twitch.config_dirty = true;
            changed = true;
        }
    });

    ui.add_space(4.0);

    technical_group(ui, "Twitch Account:", |ui| {
        let ctx = ui.ctx().clone();
        if let Some(token) = &state.twitch.token {
            let login = token.login.clone();
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("Logged in as {login}"))
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(120, 200, 140)),
                );
                if ui.button("Log out").clicked() {
                    state.twitch.logout();
                    changed = true;
                }
            });
            note(ui, "You can send chat messages from the overlay's input box.");
        } else {
            match state.twitch.auth_flow.clone() {
                AuthFlow::Idle => {
                    if ui.button("Log in with Twitch").clicked() {
                        state.twitch.start_login(&ctx);
                        changed = true;
                    }
                    note(
                        ui,
                        "Needed only to send messages. Reading chat works without logging in.",
                    );
                }
                AuthFlow::Requesting => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Contacting Twitch…");
                        if ui.small_button("Cancel").clicked() {
                            state.twitch.cancel_login();
                        }
                    });
                }
                AuthFlow::AwaitingUser {
                    user_code,
                    verification_uri,
                    expires_at,
                } => {
                    ui.label("Approve Michadame in your browser and confirm this code:");
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(&user_code)
                                .monospace()
                                .size(20.0)
                                .strong()
                                .color(egui::Color32::from_rgb(190, 150, 255)),
                        );
                        if ui.small_button("Copy").clicked() {
                            ui.output_mut(|o| o.copied_text = user_code.clone());
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Open activation page").clicked() {
                            crate::twitch::open_in_browser(&ctx, &verification_uri);
                        }
                        if ui.button("Cancel").clicked() {
                            state.twitch.cancel_login();
                        }
                        ui.spinner();
                    });
                    let left = expires_at.saturating_duration_since(Instant::now()).as_secs();
                    note(ui, format!("Waiting for approval… code expires in {}:{:02}", left / 60, left % 60));
                    ctx.request_repaint_after(std::time::Duration::from_secs(1));
                }
            }
        }

        ui.add_space(4.0);
        egui::CollapsingHeader::new(egui::RichText::new("Advanced").size(11.0).color(LABEL))
            .id_source("twitch-advanced")
            .show(ui, |ui| {
                sub_heading(ui, "Twitch Application Client ID");
                ui.horizontal(|ui| {
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut state.twitch.client_id_input)
                            .hint_text(crate::twitch::auth::DEFAULT_CLIENT_ID)
                            .desired_width(280.0),
                    );
                    if edit.lost_focus() {
                        let id = state.twitch.client_id_input.trim().to_string();
                        if id != state.twitch.config.client_id {
                            state.twitch.config.client_id = id;
                            save_now = true;
                        }
                    }
                });
                note(ui, "Leave empty to use Michadame's built-in application. Changing it requires logging in again.");
            });
    });

    // Persist slider changes once the drag is released to avoid writing every frame.
    if state.twitch.config_dirty && !ui.input(|i| i.pointer.any_down()) {
        save_now = true;
    }
    if save_now {
        state.twitch.config_dirty = false;
        crate::config::save_config(state);
        changed = true;
    }
    changed
}

fn status_line(state: &AppState) -> (egui::Color32, String) {
    let tw = &state.twitch;
    if tw.config.channel.is_empty() {
        return (MUTED, "Status: no channel set".into());
    }
    match &tw.status {
        ConnectionStatus::Disabled if !tw.config.chat_overlay_enabled && !tw.config.niconico_enabled => (
            MUTED,
            "Status: idle (enable the chat overlay or niconico comments to connect)".into(),
        ),
        ConnectionStatus::Disabled => (MUTED, "Status: disconnected".into()),
        ConnectionStatus::Connecting => (
            egui::Color32::from_rgb(230, 190, 60),
            format!("Status: connecting to #{}…", tw.config.channel),
        ),
        ConnectionStatus::Connected { authenticated } => (
            egui::Color32::from_rgb(120, 200, 140),
            format!(
                "Status: connected to #{} ({})",
                tw.config.channel,
                if *authenticated { "can chat" } else { "read-only" }
            ),
        ),
        ConnectionStatus::Reconnecting { secs, reason } => (
            egui::Color32::from_rgb(230, 120, 60),
            format!("Status: {reason}; retrying in {secs}s"),
        ),
    }
}

fn draw_broadcast_group(ui: &mut egui::Ui, state: &mut AppState, save_now: &mut bool) -> bool {
    let mut changed = false;
    technical_group(ui, "Live Broadcast:", |ui| {
        let status = state.twitch.broadcast_status();
        let is_live = matches!(status, BroadcastStatus::Live { .. });
        let is_connecting = matches!(status, BroadcastStatus::Connecting);
        let is_broadcasting = is_live || is_connecting;

        ui.horizontal(|ui| {
            if is_broadcasting {
                let btn = egui::Button::new(
                    egui::RichText::new("■ Stop Streaming")
                        .monospace()
                        .strong()
                        .size(12.0)
                        .color(egui::Color32::WHITE),
                )
                .fill(egui::Color32::from_rgb(204, 51, 51));
                if ui.add_sized([160.0, 26.0], btn).clicked() {
                    state.twitch.stop_broadcast();
                    changed = true;
                }
            } else {
                let btn = egui::Button::new(
                    egui::RichText::new("● Go Live on Twitch")
                        .monospace()
                        .strong()
                        .size(12.0)
                        .color(egui::Color32::WHITE),
                )
                .fill(egui::Color32::from_rgb(145, 70, 255));
                if ui.add_sized([160.0, 26.0], btn).clicked() {
                    let audio_tap = state.replay.audio.clone();
                    let render_dev = state.replay.config.render_device.clone();
                    if let Err(_) = state.twitch.start_broadcast(audio_tap, &render_dev) {
                        // Error notification is dispatched inside start_broadcast
                    }
                    changed = true;
                }
            }

            match &status {
                BroadcastStatus::Live { started_at } => {
                    let elapsed = started_at.elapsed().as_secs();
                    let hours = elapsed / 3600;
                    let mins = (elapsed % 3600) / 60;
                    let secs = elapsed % 60;
                    let (fps, kbps, vdrop, adrop) = if let Some(m) = state.twitch.broadcast_metrics() {
                        let fps = m.current_fps.load(Ordering::Relaxed) as f32 / 10.0;
                        let kbps = m.current_bitrate_kbps.load(Ordering::Relaxed);
                        let vdrop = m.video_dropped.load(Ordering::Relaxed);
                        let adrop = m.audio_dropped.load(Ordering::Relaxed);
                        (fps, kbps, vdrop, adrop)
                    } else {
                        (0.0, 0, 0, 0)
                    };
                    ui.label(
                        egui::RichText::new(format!(
                            "● LIVE {hours:02}:{mins:02}:{secs:02}  |  {fps:.1} FPS  |  {kbps} kbps  |  Drops: V {vdrop} / A {adrop}"
                        ))
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(80, 220, 120)),
                    );
                    ui.ctx().request_repaint_after(Duration::from_millis(500));
                }
                BroadcastStatus::Connecting => {
                    ui.spinner();
                    ui.label(
                        egui::RichText::new("Connecting to Twitch ingest…")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(230, 190, 60)),
                    );
                    ui.ctx().request_repaint_after(Duration::from_millis(250));
                }
                BroadcastStatus::Error(err) => {
                    ui.label(
                        egui::RichText::new(format!("Error: {err}"))
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(240, 70, 70)),
                    );
                }
                BroadcastStatus::Offline => {
                    ui.label(
                        egui::RichText::new("Status: Offline")
                            .monospace()
                            .size(11.0)
                            .color(MUTED),
                    );
                }
            }
        });

        ui.add_space(4.0);

        // Encoder / Stream configuration
        ui.add_enabled_ui(!is_broadcasting, |ui| {
            // Video & Audio Bitrate
            ui.horizontal(|ui| {
                label(ui, "Video Bitrate:");
                let mut v_kbps = state.twitch.config.video_bitrate_kbps;
                if ui
                    .add(
                        egui::DragValue::new(&mut v_kbps)
                            .clamp_range(500..=12000)
                            .speed(50)
                            .suffix(" kbps"),
                    )
                    .on_hover_text("Target video bitrate for Twitch streaming (6000 kbps recommended for 1080p60)")
                    .changed()
                {
                    state.twitch.config.video_bitrate_kbps = v_kbps;
                    *save_now = true;
                    changed = true;
                }

                ui.add_space(16.0);

                label(ui, "Audio Bitrate:");
                let cur_ab = state.twitch.config.audio_bitrate_kbps;
                let bitrates = [64, 96, 128, 160, 192, 256, 320];
                egui::ComboBox::from_id_source("twitch_audio_bitrate")
                    .selected_text(format!("{} kbps", cur_ab))
                    .show_ui(ui, |ui| {
                        for &br in &bitrates {
                            if ui
                                .selectable_value(
                                    &mut state.twitch.config.audio_bitrate_kbps,
                                    br,
                                    format!("{br} kbps"),
                                )
                                .clicked()
                            {
                                *save_now = true;
                                changed = true;
                            }
                        }
                    });
            });

            ui.add_space(3.0);

            // Video Encoder & Preset
            ui.horizontal(|ui| {
                label(ui, "Video Encoder:");
                let enc_text = match state.twitch.config.video_encoder {
                    StreamingVideoEncoder::SoftwareX264 => "Software (x264)",
                    StreamingVideoEncoder::HardwareVaapi => "Hardware (VAAPI H.264)",
                };
                egui::ComboBox::from_id_source("twitch_video_encoder")
                    .selected_text(enc_text)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_value(
                                &mut state.twitch.config.video_encoder,
                                StreamingVideoEncoder::SoftwareX264,
                                "Software (x264)",
                            )
                            .clicked()
                        {
                            *save_now = true;
                            changed = true;
                        }
                        if ui
                            .selectable_value(
                                &mut state.twitch.config.video_encoder,
                                StreamingVideoEncoder::HardwareVaapi,
                                "Hardware (VAAPI H.264)",
                            )
                            .clicked()
                        {
                            *save_now = true;
                            changed = true;
                        }
                    });

                if state.twitch.config.video_encoder == StreamingVideoEncoder::SoftwareX264 {
                    ui.add_space(16.0);
                    label(ui, "Encoder Preset:");
                    let presets = ["ultrafast", "superfast", "veryfast", "faster", "fast", "medium", "slow"];
                    egui::ComboBox::from_id_source("twitch_encoder_preset")
                        .selected_text(&state.twitch.config.encoder_preset)
                        .show_ui(ui, |ui| {
                            for &p in &presets {
                                if ui
                                    .selectable_value(
                                        &mut state.twitch.config.encoder_preset,
                                        p.to_string(),
                                        p,
                                    )
                                    .clicked()
                                {
                                    *save_now = true;
                                    changed = true;
                                }
                            }
                        });
                }
            });

            ui.add_space(3.0);

            // Audio Encoder
            ui.horizontal(|ui| {
                label(ui, "Audio Encoder:");
                egui::ComboBox::from_id_source("twitch_audio_encoder")
                    .selected_text("AAC (Default)")
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut state.twitch.config.audio_encoder,
                            StreamingAudioEncoder::Aac,
                            "AAC (Default)",
                        );
                    });
            });

            ui.add_space(3.0);

            // Ingest Server
            ui.horizontal(|ui| {
                label(ui, "Ingest Server:");
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut state.twitch.config.ingest_server)
                        .hint_text("rtmps://live.twitch.tv:443/app/")
                        .desired_width(260.0),
                );
                if edit.lost_focus() {
                    if state.twitch.config.ingest_server.trim().is_empty() {
                        state.twitch.config.ingest_server =
                            "rtmps://live.twitch.tv:443/app/".to_string();
                    }
                    *save_now = true;
                    changed = true;
                }
                if ui
                    .small_button("Reset")
                    .on_hover_text("Reset to default secure RTMPS server")
                    .clicked()
                {
                    state.twitch.config.ingest_server =
                        "rtmps://live.twitch.tv:443/app/".to_string();
                    *save_now = true;
                    changed = true;
                }
            });

            ui.add_space(3.0);

            // Stream Key
            ui.horizontal(|ui| {
                label(ui, "Stream Key:");
                let show = state.twitch.show_stream_key;
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut state.twitch.stream_key_input)
                        .password(!show)
                        .hint_text("live_...")
                        .desired_width(210.0),
                );
                if edit.lost_focus() || edit.changed() {
                    let trimmed = state.twitch.stream_key_input.trim().to_string();
                    if trimmed != state.twitch.config.stream_key {
                        state.twitch.config.stream_key = trimmed;
                        *save_now = true;
                        changed = true;
                    }
                }
                let toggle_label = if show { "Hide" } else { "Show" };
                if ui.small_button(toggle_label).clicked() {
                    state.twitch.show_stream_key = !show;
                }

                let has_token = state.twitch.token.is_some();
                let fetch_btn = ui.add_enabled(
                    has_token,
                    egui::Button::new(egui::RichText::new("Fetch from Twitch").size(11.0)),
                );
                if fetch_btn.clicked() {
                    state.twitch.try_fetch_stream_key();
                    *save_now = true;
                    changed = true;
                }
                if !has_token {
                    fetch_btn.on_hover_text("Log in to Twitch below under 'Twitch Account' to auto-fetch your stream key");
                }
            });
        });

        note(
            ui,
            "Stream output is clamped to 1080p max. Excludes the Twitch chat overlay and debug window.",
        );
    });
    changed
}

