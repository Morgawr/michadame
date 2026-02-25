use crate::{app::AppState, devices::filter_type::CrtFilter};
use eframe::egui;

pub fn draw_filters(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;

    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Appearance:");
        ui.label("Filter:");
        let current_filter = state.crt_filter.load(std::sync::atomic::Ordering::Relaxed);
        let selected_text = CrtFilter::from_u8(current_filter).to_string();

        egui::ComboBox::from_id_source("filter_selector")
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_value(&mut current_filter.clone(), 0, "None")
                    .clicked()
                {
                    state
                        .crt_filter
                        .store(0, std::sync::atomic::Ordering::Relaxed);
                    changed = true;
                }
                if ui
                    .selectable_value(&mut current_filter.clone(), 1, "Lottes")
                    .clicked()
                {
                    state
                        .crt_filter
                        .store(1, std::sync::atomic::Ordering::Relaxed);
                    changed = true;
                }
            });

        ui.label("Scaler:");
        let current_scaler = state
            .scaler_filter
            .load(std::sync::atomic::Ordering::Relaxed);
        let scaler_text = crate::video::types::ScalerFilter::from_u8(current_scaler).to_string();
        egui::ComboBox::from_id_source("scaler_selector")
            .selected_text(scaler_text)
            .show_ui(ui, |ui| {
                for i in 0..=4 {
                    let text = crate::video::types::ScalerFilter::from_u8(i).to_string();
                    if ui
                        .selectable_value(&mut current_scaler.clone(), i, text)
                        .clicked()
                    {
                        state
                            .scaler_filter
                            .store(i, std::sync::atomic::Ordering::Relaxed);
                        changed = true;
                    }
                }
            });
        
        ui.label("Range:");
        let current_range = state.color_range.load(std::sync::atomic::Ordering::Relaxed);
        let range_text = crate::video::types::ColorRange::from_u8(current_range).to_string();
        egui::ComboBox::from_id_source("range_selector")
            .selected_text(range_text)
            .show_ui(ui, |ui| {
                if ui.selectable_value(&mut current_range.clone(), 0, "Full (PC)").clicked() {
                    state.color_range.store(0, std::sync::atomic::Ordering::Relaxed);
                    changed = true;
                }
                if ui.selectable_value(&mut current_range.clone(), 1, "Limited (TV)").clicked() {
                    state.color_range.store(1, std::sync::atomic::Ordering::Relaxed);
                    changed = true;
                }
            });

        if ui
            .checkbox(&mut state.video.pixelate_filter_enabled, "Pixelate")
            .changed()
        {
            changed = true;
        }
        if ui
            .checkbox(&mut state.video.median_filter_enabled, "Median Filter 3x1")
            .changed()
        {
            changed = true;
        }
    });

    ui.group(|ui| {
        ui.label("Visual Tweaks:");

        if ui
            .add(
                egui::Slider::new(&mut state.video.vibrance, 0.0..=3.0)
                    .text("Vibrance (Saturation)")
                    .custom_formatter(|n, _| format!("{:.0}%", n * 100.0)),
            )
            .changed()
        {
            changed = true;
        }

        ui.horizontal(|ui| {
            if ui
                .checkbox(&mut state.video.use_magenta_background, "Magenta Background")
                .on_hover_text(
                    "Uses a magenta background around the video stream instead of black.",
                )
                .changed()
            {
                changed = true;
            }
            if ui
                .add(
                    egui::Slider::new(&mut state.video.horizontal_stretch, 1.0..=1.5)
                        .text("Horizontal Stretch")
                        .step_by(0.001)
                        .custom_formatter(|n, _| format!("{:.1}%", n * 100.0)),
                )
                .changed()
            {
                changed = true;
            }
        });

        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Slider::new(&mut state.video.overscan_x, -0.2..=0.2)
                        .text("Overscan X")
                        .step_by(0.001)
                        .custom_formatter(|n, _| format!("{:.1}%", n * 100.0)),
                )
                .changed()
            {
                changed = true;
            }
            if ui
                .add(
                    egui::Slider::new(&mut state.video.overscan_y, -0.2..=0.2)
                        .text("Overscan Y")
                        .step_by(0.001)
                        .custom_formatter(|n, _| format!("{:.1}%", n * 100.0)),
                )
                .changed()
            {
                changed = true;
            }
        });
    });

    let current_filter =
        CrtFilter::from_u8(state.crt_filter.load(std::sync::atomic::Ordering::Relaxed));

    if current_filter == CrtFilter::Lottes {
        ui.group(|ui| {
            ui.label("Lottes CRT Parameters:");

            let mut scan = state.crt.hard_scan as f32;
            let mut pix = state.crt.hard_pix as f32;
            let mut bright = state.crt.brightboost as f32;
            let mut warp_x = state.crt.warp_x as f32;
            let mut warp_y = state.crt.warp_y as f32;
            let mut mask = state.crt.shadow_mask as f32;
            let mut bloom_pix = state.crt.hard_bloom_pix as f32;
            let mut bloom_scan = state.crt.hard_bloom_scan as f32;
            let mut bloom_amount = state.crt.bloom_amount as f32;
            let mut shape = state.crt.shape as f32;

            ui.horizontal(|ui| {
                if ui
                    .add(egui::Slider::new(&mut scan, -20.0..=0.0).text("HardScan"))
                    .changed()
                {
                    state.crt.hard_scan = scan;
                    changed = true;
                }
                if ui
                    .add(egui::Slider::new(&mut pix, -20.0..=0.0).text("HardPix"))
                    .changed()
                {
                    state.crt.hard_pix = pix;
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Slider::new(&mut bright, 0.5..=2.0).text("Brightboost"))
                    .changed()
                {
                    state.crt.brightboost = bright;
                    changed = true;
                }
                if ui
                    .add(egui::Slider::new(&mut bloom_amount, 0.0..=1.0).text("Bloom Amount"))
                    .changed()
                {
                    state.crt.bloom_amount = bloom_amount;
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Slider::new(&mut warp_x, 0.0..=0.125).text("WarpX"))
                    .changed()
                {
                    state.crt.warp_x = warp_x;
                    changed = true;
                }
                if ui
                    .add(egui::Slider::new(&mut warp_y, 0.0..=0.125).text("WarpY"))
                    .changed()
                {
                    state.crt.warp_y = warp_y;
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Slider::new(&mut mask, 0.0..=4.0).text("ShadowMask"))
                    .on_hover_text("0=None, 1=Compressed TV, 2=Aperture, 3=VGA, 4=VGA (lighter)")
                    .changed()
                {
                    state.crt.shadow_mask = mask.round(); // Assuming integer values desired for enum
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Slider::new(&mut shape, 0.0..=4.0).text("Shape"))
                    .on_hover_text("0=Linear, 1=Gaussian, 2=Sinc, etc.")
                    .changed()
                {
                    state.crt.shape = shape.round();
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Slider::new(&mut bloom_pix, -2.0..=2.0).text("BloomPix"))
                    .changed()
                {
                    state.crt.hard_bloom_pix = bloom_pix;
                    changed = true;
                }
                if ui
                    .add(egui::Slider::new(&mut bloom_scan, -2.0..=2.0).text("BloomScan"))
                    .changed()
                {
                    state.crt.hard_bloom_scan = bloom_scan;
                    changed = true;
                }
            });
            if ui.button("Reset Defaults").clicked() {
                state.crt.hard_scan = -8.0;
                state.crt.hard_pix = -3.0;
                state.crt.brightboost = 1.0;
                state.crt.warp_x = 0.031;
                state.crt.warp_y = 0.041;
                state.crt.shadow_mask = 1.0;
                state.crt.hard_bloom_pix = -1.5;
                state.crt.hard_bloom_scan = -2.0;
                state.crt.bloom_amount = 0.15;
                state.crt.shape = 2.0;

                state.video.vibrance = 1.0;
                state.video.use_magenta_background = false;
                state.video.horizontal_stretch = 1.0;
                state.video.pixelate_filter_enabled = false;
                state.video.median_filter_enabled = false;
                state.video.overscan_x = 0.0;
                state.video.overscan_y = 0.0;
                changed = true;
            }
        });
    }

    changed
}
