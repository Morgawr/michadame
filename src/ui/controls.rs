use crate::app::AppState;
use eframe::egui;
use std::sync::atomic::Ordering;

use crate::ui::{devices, filters, profiles};

pub fn layout_top_ui(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;

    ui.horizontal(|ui| {
        if let Some(logo) = &state.logo_texture {
            ui.add(egui::Image::new(logo).max_height(160.0));
        }
        ui.heading("Michadame Viewer");
    });
    ui.separator();

    ui.horizontal(|ui| {
        if state.ui.video_window_open {
            if ui.button("🛑 Stop Stream").clicked() {
                state.stop_stream(ui.ctx());
                state.info("Stream stopped.");
            }

            // Audio Level Meter
            let peak = state
                .hardware
                .audio_peak_amplitude
                .swap(0, Ordering::Relaxed) as f32
                / 1000.0;
            let color = if peak > 0.9 {
                egui::Color32::RED
            } else if peak > 0.7 {
                egui::Color32::YELLOW
            } else {
                egui::Color32::GREEN
            };
            ui.add(
                egui::ProgressBar::new(peak.min(1.0))
                    .text(format!("Audio: {:.0}%", peak * 100.0))
                    .fill(color),
            );
        } else {
            let can_stream = !state.hardware.selected_video_device.is_empty()
                && state.hardware.selected_resolution.0 > 0;
            if ui
                .add_enabled(can_stream, egui::Button::new("▶ Start Stream"))
                .clicked()
            {
                state.start_stream(ui.ctx());
                state.info("Stream starting...");
            }
            if !can_stream {
                ui.label("Select Video Format/Resolution first.");
            }
        }
    });

    ui.separator();

    crate::replay::ui::draw_toggle(&mut state.replay, ui, state.ui.video_window_open);
    changed |= profiles::draw_profile_management(ui, state);
    changed |= devices::draw_device_selectors(ui, state);
    changed |= filters::draw_filters(ui, state);

    if crate::replay::ui::draw(&mut state.replay, ui) {
        changed = true;
        if let Err(error) = crate::config::save_replay_config(&state.replay.config) {
            state.error(format!("Could not save replay settings: {error}"));
        }
    }

    ui.separator();
    ui.label(egui::RichText::new("Quick OCR (Google Lens)").strong());
    ui.horizontal(|ui| {
        ui.label("Sticky Box Distance:");
        let slider = egui::Slider::new(&mut state.ocr.sticky_distance, 0.0..=2.0)
            .step_by(0.05)
            .suffix("x");
        if ui
            .add(slider)
            .on_hover_text(
                "Maximum vertical distance between adjacent lines to merge into one block (as a multiple of line height).\n• 0.0: Keep all lines separate\n• 0.6: Default (tight, dialogue lines only)\n• 1.0+: Looser merging",
            )
            .changed()
        {
            state.ocr.recompute_boxes();
            crate::config::save_config(state);
            changed = true;
        }
    });
    ui.label(
        egui::RichText::new("Space = capture OCR, shift + space = clear OCR boxes").weak(),
    );

    if ui
        .checkbox(&mut state.ocr.hide_overlay, "Hide OCR boxes overlay")
        .on_hover_text(
            "Hides the blue bounding boxes and replacement text overlay on the video feed.\nDictionary lookup on hover and popup behavior will continue to work normally.",
        )
        .changed()
    {
        crate::config::save_config(state);
        changed = true;
    }

    if state.ocr.is_processing.load(Ordering::Relaxed) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Processing OCR with Google Lens...");
        });
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Japanese Dictionary (Jitendex)").strong());
        if state.dict.update_available {
            ui.label(
                egui::RichText::new("(!)")
                    .color(egui::Color32::from_rgb(251, 191, 36))
                    .strong(),
            )
            .on_hover_text("An update is available for the Jitendex dictionary!");
        }
    });

    if let Some(meta) = &state.dict.installed_metadata {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "Installed: {} entries (rev {})",
                    meta.total_entries, meta.revision
                ))
                .small()
                .weak(),
            );
        });
    } else {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Status: Not installed - click Sync to download")
                    .small()
                    .color(egui::Color32::from_rgb(251, 191, 36)),
            );
        });
    }

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
                "⟳ Sync Jitendex Dictionary (!)"
            } else {
                "⟳ Sync Jitendex Dictionary"
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
                ui.label(egui::RichText::new("Checking for updates...").weak().small());
            } else if ui.small_button("Check Updates").clicked() {
                state.dict.trigger_check_version();
            }
        });
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Frequency Dictionary (Jiten Global)").strong());
        if state.dict.freq_update_available {
            ui.label(
                egui::RichText::new("(!)")
                    .color(egui::Color32::from_rgb(251, 191, 36))
                    .strong(),
            )
            .on_hover_text("An update is available for the Jiten frequency dictionary!");
        }
    });

    if let Some(meta) = &state.dict.installed_freq_metadata {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "Installed: {} entries (rev {})",
                    meta.total_entries, meta.revision
                ))
                .small()
                .weak(),
            );
        });
    } else {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Status: Not installed - click Sync to download")
                    .small()
                    .color(egui::Color32::from_rgb(251, 191, 36)),
            );
        });
    }

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
                "⟳ Sync Jiten Frequency (!)"
            } else {
                "⟳ Sync Jiten Frequency"
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
                ui.label(egui::RichText::new("Checking for updates...").weak().small());
            } else if ui.small_button("Check Updates").clicked() {
                state.dict.trigger_check_freq_version();
            }
        });
    }

    changed
}
