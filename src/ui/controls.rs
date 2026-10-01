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

    if state.ocr.is_processing.load(Ordering::Relaxed) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Processing OCR with Google Lens...");
        });
    }

    changed
}
