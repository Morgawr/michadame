//! "Twitch" settings tab.

use super::controls::{percent_slider_range, slider_item, sub_heading, technical_group};
use crate::app::AppState;
use crate::twitch::{normalize_channel, AuthFlow, ConnectionStatus, StreamStatus};
use eframe::egui;
use std::time::Instant;

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
        ConnectionStatus::Disabled if !tw.config.chat_overlay_enabled => {
            (MUTED, "Status: idle (enable the chat overlay to connect)".into())
        }
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
