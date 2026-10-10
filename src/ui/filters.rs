use crate::{
    app::AppState,
    devices::filter_type::CrtFilter,
    ui::controls::{
        mult_slider, percent_slider, percent_slider_range, slider_item, sub_heading,
        technical_group, technical_separator, two_columns,
    },
};
use eframe::egui;

pub fn draw_shaders_tab(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;

    // Appearance Group
    technical_group(ui, "Appearance:", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("Filter:")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            );
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
                        crate::config::save_config(state);
                        changed = true;
                    }
                    if ui
                        .selectable_value(&mut current_filter.clone(), 1, "Lottes")
                        .clicked()
                    {
                        state.selected_crt_filter = CrtFilter::Lottes;
                        state
                            .crt_filter
                            .store(1, std::sync::atomic::Ordering::Relaxed);
                        crate::config::save_config(state);
                        changed = true;
                    }
                    if ui
                        .selectable_value(&mut current_filter.clone(), 2, "Halo")
                        .clicked()
                    {
                        state.selected_crt_filter = CrtFilter::Halo;
                        state
                            .crt_filter
                            .store(2, std::sync::atomic::Ordering::Relaxed);
                        crate::config::save_config(state);
                        changed = true;
                    }
                });

            ui.add_space(8.0);

            ui.label(
                egui::RichText::new("Scaler:")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            );
            let current_scaler = state
                .scaler_filter
                .load(std::sync::atomic::Ordering::Relaxed);
            let scaler_text =
                crate::video::types::ScalerFilter::from_u8(current_scaler).to_string();
            egui::ComboBox::from_id_source("scaler_selector")
                .selected_text(scaler_text)
                .show_ui(ui, |ui| {
                    for &scaler in &crate::video::types::ScalerFilter::ALL {
                        let i = scaler as u8;
                        let text = scaler.to_string();
                        if ui
                            .selectable_value(&mut current_scaler.clone(), i, text)
                            .clicked()
                        {
                            state
                                .scaler_filter
                                .store(i, std::sync::atomic::Ordering::Relaxed);
                            crate::config::save_config(state);
                            changed = true;
                        }
                    }
                });

            ui.add_space(8.0);

            ui.label(
                egui::RichText::new("Range:")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(170, 170, 170)),
            );
            let current_range = state.color_range.load(std::sync::atomic::Ordering::Relaxed);
            let range_text = crate::video::types::ColorRange::from_u8(current_range).to_string();
            egui::ComboBox::from_id_source("range_selector")
                .selected_text(range_text)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_value(&mut current_range.clone(), 0, "Full (PC)")
                        .clicked()
                    {
                        state
                            .color_range
                            .store(0, std::sync::atomic::Ordering::Relaxed);
                        crate::config::save_config(state);
                        changed = true;
                    }
                    if ui
                        .selectable_value(&mut current_range.clone(), 1, "Limited (TV)")
                        .clicked()
                    {
                        state
                            .color_range
                            .store(1, std::sync::atomic::Ordering::Relaxed);
                        crate::config::save_config(state);
                        changed = true;
                    }
                });
        });

        technical_separator(ui);

        ui.horizontal_wrapped(|ui| {
            if ui
                .checkbox(&mut state.video.pixelate_filter_enabled, "Pixelate")
                .changed()
            {
                crate::config::save_config(state);
                changed = true;
            }

            ui.add_space(6.0);

            if ui
                .checkbox(&mut state.video.median_filter_enabled, "Median Filter 3x1")
                .changed()
            {
                crate::config::save_config(state);
                changed = true;
            }

            if state.video.median_filter_enabled {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Intensity:")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(153, 153, 153)),
                );
                let slider = percent_slider(&mut state.video.median_mix, 0.0..=1.0);
                if ui.add(slider).changed() {
                    crate::config::save_config(state);
                    changed = true;
                }
            }
        });

        ui.add_space(2.0);

        ui.horizontal_wrapped(|ui| {
            if ui
                .checkbox(&mut state.video.fft_filter_enabled, "FFT Mask Filter")
                .changed()
            {
                crate::config::save_config(state);
                changed = true;
            }

            if state.video.fft_filter_enabled {
                if ui.button("Edit Mask…").clicked() {
                    state.video.fft_mask_window_open = true;
                }

                let (fft_w, fft_h) = state.fft_mask_resolution;
                let has_frame = fft_w > 0 && fft_h > 0;
                let stream_res = state
                    .latest_frame
                    .as_ref()
                    .map(|f| (f.width, f.height))
                    .unwrap_or((0, 0));

                if has_frame && stream_res.0 > 0 {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Mask:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 170, 170)),
                    );
                    ui.text_edit_singleline(&mut state.fft_mask_save_name);
                    if ui.button("Save").clicked() && !state.fft_mask_save_name.is_empty() {
                        match crate::config::fft_masks::save_mask(
                            &state.fft_mask_save_name,
                            stream_res,
                            (fft_w, fft_h),
                            &state.fft_mask_data,
                            state.fft_mask_threshold,
                            state.fft_black_threshold,
                        ) {
                            Ok(()) => {
                                state.info(format!("Saved FFT mask '{}'", state.fft_mask_save_name));
                                state.fft_available_masks =
                                    crate::config::fft_masks::list_masks_for_resolution(stream_res);
                            }
                            Err(e) => state.error(format!("Failed to save mask: {}", e)),
                        }
                    }

                    if state.fft_available_masks.is_empty() {
                        state.fft_available_masks =
                            crate::config::fft_masks::list_masks_for_resolution(stream_res);
                    }

                    if !state.fft_available_masks.is_empty() {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Load:")
                                .monospace()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(170, 170, 170)),
                        );
                        for mask_name in state.fft_available_masks.clone() {
                            if ui.button(&mask_name).clicked() {
                                match crate::config::fft_masks::load_mask(&mask_name, stream_res) {
                                    Ok((data, fft_res, mask_thresh, black_thresh)) => {
                                        if fft_res == (fft_w, fft_h) {
                                            state.fft_mask_data = data;
                                            state.fft_mask_threshold = mask_thresh;
                                            state.fft_black_threshold = black_thresh;
                                            state.fft_mask_save_name = mask_name.clone();
                                            state.fft_mask_dirty = true;
                                            changed = true;
                                            state.info(format!("Loaded FFT mask '{}'", mask_name));
                                        } else {
                                            state.error(format!(
                                                "FFT size mismatch: mask is {}x{} but current is {}x{}",
                                                fft_res.0, fft_res.1, fft_w, fft_h
                                            ));
                                        }
                                    }
                                    Err(e) => state.error(format!("Failed to load mask: {}", e)),
                                }
                            }
                        }
                    }
                }
            }
        });
    });

    ui.add_space(4.0);

    let current_filter =
        CrtFilter::from_u8(state.crt_filter.load(std::sync::atomic::Ordering::Relaxed));

    if current_filter == CrtFilter::Halo {
        technical_group(ui, "", |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Halo CRT Parameters:")
                        .monospace()
                        .size(11.0)
                        .strong()
                        .color(egui::Color32::from_rgb(224, 224, 224)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Save as Defaults").clicked() {
                        state.halo_defaults = state.halo.clone();
                        let current_profile_data = crate::config::build_profile_from_state(state);
                        state
                            .profiles
                            .insert(state.active_profile.clone(), current_profile_data);
                        crate::config::save_config(state);
                        state.info("Saved current settings as default");
                        changed = true;
                    }
                    if ui.button("Reset Defaults").clicked() {
                        state.halo = state.halo_defaults.clone();
                        changed = true;
                    }
                });
            });
            ui.add_space(4.0);

            sub_heading(ui, "Beam Dynamics & Brightness:");
                two_columns(ui, |c1, c2| {
                    if slider_item(
                        c1,
                        "Brightboost (Dark):",
                        egui::Slider::new(&mut state.halo.brightboost, 0.5..=3.0).step_by(0.05),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                    if slider_item(
                        c2,
                        "Brightboost (Bright):",
                        egui::Slider::new(&mut state.halo.brightboost1, 0.5..=3.0).step_by(0.05),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                });

                two_columns(ui, |c1, c2| {
                    if slider_item(
                        c1,
                        "Beam Min:",
                        egui::Slider::new(&mut state.halo.beam_min, 0.5..=3.0).step_by(0.05),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                    if slider_item(
                        c2,
                        "Beam Max:",
                        egui::Slider::new(&mut state.halo.beam_max, 0.2..=2.0).step_by(0.05),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                });

                two_columns(ui, |c1, c2| {
                    if slider_item(
                        c1,
                        "Beam Size:",
                        egui::Slider::new(&mut state.halo.beam_size, 0.0..=2.0).step_by(0.05),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                    if slider_item(
                        c2,
                        "Sharpness:",
                        egui::Slider::new(&mut state.halo.h_sharp, 1.0..=10.0).step_by(0.1),
                        Some("Horizontal sharpness (1.0 = soft analog CRT, 3.5 = Trinitron, 10.0 = razor-sharp PVM)"),
                    )
                    .changed()
                    {
                        changed = true;
                    }
                });

                technical_separator(ui);

                sub_heading(ui, "Shadow Mask:");
                two_columns(ui, |c1, c2| {
                    // Left column: Shadow mask selector dropdown
                    c1.horizontal(|ui| {
                        ui.add_sized(
                            [130.0, 18.0],
                            egui::Label::new(
                                egui::RichText::new("Shadow Mask:")
                                    .monospace()
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(153, 153, 153)),
                            ),
                        );
                        let mask_names = [
                            (0.0, "0: None"),
                            (1.0, "1: CGWG"),
                            (2.0, "2: Lottes"),
                            (3.0, "3: Stretched"),
                            (4.0, "4: VGA"),
                            (5.0, "5: Fine"),
                            (6.0, "6: Trinitron"),
                        ];
                        let cur_val = state.halo.shadow_mask.round();
                        let cur_text = mask_names
                            .iter()
                            .find(|(v, _)| *v == cur_val)
                            .map(|(_, t)| *t)
                            .unwrap_or("Custom");
                        let avail_w = (ui.available_width() - 4.0).max(60.0);
                        egui::ComboBox::from_id_source("halo_shadow_mask_combo")
                            .selected_text(cur_text)
                            .width(avail_w)
                            .show_ui(ui, |ui| {
                                for (val, text) in mask_names {
                                    if ui.selectable_value(&mut state.halo.shadow_mask, val, text).clicked() {
                                        changed = true;
                                    }
                                }
                            });
                    });

                    // Right column: Mask Strength
                    if slider_item(
                        c2,
                        "Mask Strength:",
                        percent_slider(&mut state.halo.maskstr, 0.0..=1.0),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                });

                two_columns(ui, |c1, c2| {
                    if slider_item(
                        c1,
                        "Mask Size:",
                        egui::Slider::new(&mut state.halo.masksize, 1.0..=4.0).step_by(1.0),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                    if slider_item(
                        c2,
                        "Mask Cutoff:",
                        percent_slider(&mut state.halo.mcut, 0.0..=1.0),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                });

                two_columns(ui, |c1, c2| {
                    if slider_item(
                        c1,
                        "Slot Mask (Bright):",
                        percent_slider(&mut state.halo.slotmask, 0.0..=1.0),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                    if slider_item(
                        c2,
                        "Slot Mask (Dark):",
                        percent_slider(&mut state.halo.slotmask1, 0.0..=1.0),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                });

                two_columns(ui, |c1, c2| {
                    if slider_item(
                        c1,
                        "Double Slot:",
                        egui::Slider::new(&mut state.halo.double_slot, 1.0..=4.0).step_by(1.0),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                    if slider_item(
                        c2,
                        "Smooth Mask:",
                        egui::Slider::new(&mut state.halo.smoothmask, 0.0..=2.0).step_by(0.05),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                });

                technical_separator(ui);

                sub_heading(ui, "Glow & Curvature:");
                two_columns(ui, |c1, c2| {
                    if slider_item(
                        c1,
                        "Glow:",
                        percent_slider(&mut state.halo.glow, 0.0..=1.0),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                    if slider_item(
                        c2,
                        "Bloom:",
                        percent_slider(&mut state.halo.bloom, 0.0..=1.0),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                });

                two_columns(ui, |c1, c2| {
                    if slider_item(
                        c1,
                        "Halation:",
                        percent_slider(&mut state.halo.halation, 0.0..=0.5),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                    if slider_item(
                        c2,
                        "Halo Glow:",
                        egui::Slider::new(&mut state.halo.halo_intensity, 0.0..=2.0).step_by(0.05),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                });

                two_columns(ui, |c1, c2| {
                    if slider_item(
                        c1,
                        "Screen Scale %:",
                        percent_slider_range(&mut state.halo.halo_zoom, 50.0..=100.0, 100.0),
                        Some("Scales the CRT screen within the viewport to create bezel room for the halo glow."),
                    )
                    .changed()
                    {
                        changed = true;
                    }
                    if slider_item(
                        c2,
                        "Corner Size:",
                        egui::Slider::new(&mut state.halo.corner_size, 0.0..=0.1).step_by(0.005),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                });

                ui.add_space(3.0);
                if ui.checkbox(&mut state.halo.curvature, "Curvature").changed() {
                    changed = true;
                }
            });
        ui.add_space(4.0);
    } else if current_filter == CrtFilter::Lottes {
        technical_group(ui, "", |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Lottes CRT Parameters:")
                        .monospace()
                        .size(11.0)
                        .strong()
                        .color(egui::Color32::from_rgb(224, 224, 224)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Reset Defaults").clicked() {
                        state.crt.hard_scan = -8.0;
                        state.crt.hard_pix = -3.0;
                        state.crt.brightboost = 1.0;
                        state.crt.warp_x = 0.031;
                        state.crt.warp_y = 0.041;
                        state.crt.shadow_mask = 3.0;
                        state.crt.hard_bloom_pix = -1.5;
                        state.crt.hard_bloom_scan = -2.0;
                        state.crt.bloom_amount = 0.15;
                        state.crt.shape = 2.0;
                        changed = true;
                    }
                });
            });
            ui.add_space(4.0);

            two_columns(ui, |c1, c2| {
                    if slider_item(
                        c1,
                        "HardScan:",
                        egui::Slider::new(&mut state.crt.hard_scan, -20.0..=0.0).step_by(0.5),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                    if slider_item(
                        c2,
                        "HardPix:",
                        egui::Slider::new(&mut state.crt.hard_pix, -20.0..=0.0).step_by(0.5),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                });

                two_columns(ui, |c1, c2| {
                    if slider_item(
                        c1,
                        "Brightboost:",
                        egui::Slider::new(&mut state.crt.brightboost, 0.5..=2.0).step_by(0.05),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                    if slider_item(
                        c2,
                        "Bloom Amount:",
                        egui::Slider::new(&mut state.crt.bloom_amount, 0.0..=1.0).step_by(0.01),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                });

                two_columns(ui, |c1, c2| {
                    if slider_item(
                        c1,
                        "WarpX:",
                        egui::Slider::new(&mut state.crt.warp_x, 0.0..=0.125).step_by(0.001),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                    if slider_item(
                        c2,
                        "WarpY:",
                        egui::Slider::new(&mut state.crt.warp_y, 0.0..=0.125).step_by(0.001),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                });

                two_columns(ui, |c1, c2| {
                    if slider_item(
                        c1,
                        "ShadowMask:",
                        egui::Slider::new(&mut state.crt.shadow_mask, 0.0..=4.0).step_by(1.0),
                        Some("0=None, 1=Compressed TV, 2=Aperture-grille, 3=Stretched VGA, 4=VGA"),
                    )
                    .changed()
                    {
                        changed = true;
                    }
                    if slider_item(
                        c2,
                        "Shape:",
                        egui::Slider::new(&mut state.crt.shape, 0.0..=10.0).step_by(0.05),
                        Some("Kernel exponent. The original Lottes default is 2.0."),
                    )
                    .changed()
                    {
                        changed = true;
                    }
                });

                two_columns(ui, |c1, c2| {
                    if slider_item(
                        c1,
                        "BloomPix:",
                        egui::Slider::new(&mut state.crt.hard_bloom_pix, -2.0..=-0.5).step_by(0.05),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                    if slider_item(
                        c2,
                        "BloomScan:",
                        egui::Slider::new(&mut state.crt.hard_bloom_scan, -4.0..=-1.0).step_by(0.05),
                        None,
                    )
                    .changed()
                    {
                        changed = true;
                    }
                });
            });
        ui.add_space(4.0);
    } else {
        technical_group(ui, "", |ui| {
            ui.label(
                egui::RichText::new("Filter is set to None. CRT post-processing is disabled.")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(102, 102, 102)),
            );
        });
        ui.add_space(4.0);
    }

    changed
}

pub fn draw_geometry_tab(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;

    // Group 1: Deinterlace Filter
    technical_group(ui, "Deinterlace Filter:", |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui
                .checkbox(&mut state.video.deinterlace_filter_enabled, "Deinterlace Filter")
                .changed()
            {
                crate::config::save_config(state);
                changed = true;
            }

            if state.video.deinterlace_filter_enabled {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Mode:")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(170, 170, 170)),
                );
                egui::ComboBox::from_id_source("deinterlace_mode")
                    .selected_text(match state.video.deinterlace_mode {
                        0 => "0: Motion Adaptive",
                        1 => "1: Bob Dejitter (50% Weave)",
                        2 => "2: Vertical FIR",
                        3 => "3: Vertical Median",
                        4 => "4: Motion Map (Debug)",
                        _ => "Unknown",
                    })
                    .show_ui(ui, |ui| {
                        let modes: [(u8, &str, &str); 5] = [
                            (0, "0: Motion Adaptive", "Same-parity 3-frame motion adaptive weave; completely stops bob flicker on static edges while filtering motion"),
                            (1, "1: Bob Dejitter (50% Weave)", "Pure 50% inter-frame weave; 100% cure for 30/60Hz bob jitter everywhere"),
                            (2, "2: Vertical FIR", "3-tap lowpass vertical blur on current frame to reduce scanline comb artifacts"),
                            (3, "3: Vertical Median", "3x1 vertical median filter on current frame to remove single-line artifacts"),
                            (4, "4: Motion Map (Debug)", "Highlights static fields (green) vs detected motion (magenta/red)"),
                        ];
                        for (mode_idx, label, tip) in modes {
                            let resp = ui.selectable_value(
                                &mut state.video.deinterlace_mode,
                                mode_idx,
                                label,
                            );
                            resp.clone().on_hover_text(tip);
                            if resp.changed() {
                                crate::config::save_config(state);
                                changed = true;
                            }
                        }
                    });
            }
        });

        if state.video.deinterlace_filter_enabled {
            ui.add_space(4.0);
            match state.video.deinterlace_mode {
                0 => {
                    // Motion Adaptive: Blend, Motion, Spatial, Pitch
                    two_columns(ui, |c1, c2| {
                        let slider = percent_slider(&mut state.video.deinterlace_blend, 0.0..=1.0);
                        if slider_item(
                            c1,
                            "Blend Amount:",
                            slider,
                            Some("Temporal mix between fields (50% is mathematical weave, completely cures bob bounce)"),
                        )
                        .changed()
                        {
                            crate::config::save_config(state);
                            changed = true;
                        }

                        let slider = egui::Slider::new(&mut state.video.deinterlace_motion_threshold, 0.01..=0.5)
                            .step_by(0.01)
                            .custom_formatter(|n, _| format!("{:.2}", n));
                        if slider_item(
                            c2,
                            "Motion Sensitivity:",
                            slider,
                            Some("Threshold to distinguish moving objects from static field jitter (higher = more areas treated as static weave)"),
                        )
                        .changed()
                        {
                            crate::config::save_config(state);
                            changed = true;
                        }
                    });

                    two_columns(ui, |c1, c2| {
                        let slider = percent_slider(&mut state.video.deinterlace_spatial_mix, 0.0..=1.0);
                        if slider_item(
                            c1,
                            "Spatial Strength (Moving):",
                            slider,
                            Some("Strength of vertical FIR/median filter in moving areas to eliminate comb lines"),
                        )
                        .changed()
                        {
                            crate::config::save_config(state);
                            changed = true;
                        }

                        let slider = mult_slider(&mut state.video.deinterlace_line_spacing, 0.5..=3.0);
                        if slider_item(
                            c2,
                            "Line Pitch Scale:",
                            slider,
                            Some("Scale factor for scanline pitch (1.0x automatically targets 1 full 480i scanline)"),
                        )
                        .changed()
                        {
                            crate::config::save_config(state);
                            changed = true;
                        }
                    });
                }
                1 => {
                    // Bob Dejitter: Blend only
                    two_columns(ui, |c1, _| {
                        let slider = percent_slider(&mut state.video.deinterlace_blend, 0.0..=1.0);
                        if slider_item(
                            c1,
                            "Blend Amount:",
                            slider,
                            Some("Temporal mix between fields (50% is mathematical weave, completely cures bob bounce)"),
                        )
                        .changed()
                        {
                            crate::config::save_config(state);
                            changed = true;
                        }
                    });
                }
                2 | 3 => {
                    // FIR or Median: Spatial & Pitch
                    two_columns(ui, |c1, c2| {
                        let slider = percent_slider(&mut state.video.deinterlace_spatial_mix, 0.0..=1.0);
                        if slider_item(
                            c1,
                            "Spatial Strength:",
                            slider,
                            Some("Strength of vertical FIR/median filter"),
                        )
                        .changed()
                        {
                            crate::config::save_config(state);
                            changed = true;
                        }

                        let slider = mult_slider(&mut state.video.deinterlace_line_spacing, 0.5..=3.0);
                        if slider_item(
                            c2,
                            "Line Pitch Scale:",
                            slider,
                            Some("Scale factor for scanline pitch"),
                        )
                        .changed()
                        {
                            crate::config::save_config(state);
                            changed = true;
                        }
                    });
                }
                4 => {
                    // Motion Map: Motion Sensitivity only
                    two_columns(ui, |c1, _| {
                        let slider = egui::Slider::new(&mut state.video.deinterlace_motion_threshold, 0.01..=0.5)
                            .step_by(0.01)
                            .custom_formatter(|n, _| format!("{:.2}", n));
                        if slider_item(
                            c1,
                            "Motion Sensitivity:",
                            slider,
                            Some("Threshold to distinguish moving objects from static field jitter"),
                        )
                        .changed()
                        {
                            crate::config::save_config(state);
                            changed = true;
                        }
                    });
                }
                _ => {}
            }
        }
    });

    ui.add_space(4.0);

    // Group 2: Aspect & Scaling
    technical_group(ui, "Aspect & Scaling:", |ui| {
        two_columns(ui, |c1, c2| {
            let slider = percent_slider_range(&mut state.video.vibrance, 0.0..=3.0, 3.0);
            if slider_item(c1, "Vibrance (Saturation):", slider, None).changed() {
                crate::config::save_config(state);
                changed = true;
            }

            let slider = egui::Slider::new(&mut state.video.horizontal_stretch, 0.5..=1.5)
                .step_by(0.001)
                .custom_formatter(|n, _| format!("{:.1}%", n * 100.0))
                .custom_parser(|s| {
                    let s = s.trim().trim_end_matches('%').trim();
                    s.parse::<f64>().ok().map(|v| if v > 1.5 { (v / 100.0) as f64 } else { v })
                });
            if slider_item(c2, "Horizontal Stretch:", slider, None).changed() {
                crate::config::save_config(state);
                changed = true;
            }
        });

        two_columns(ui, |c1, c2| {
            let slider = egui::Slider::new(&mut state.video.overscan_x, -0.2..=0.2)
                .step_by(0.0005)
                .custom_formatter(|n, _| format!("{:.1}%", n * 100.0))
                .custom_parser(|s| {
                    let s = s.trim().trim_end_matches('%').trim();
                    s.parse::<f64>().ok().map(|v| if v.abs() > 0.2 { (v / 100.0) as f64 } else { v })
                });
            if slider_item(c1, "Overscan X:", slider, None).changed() {
                crate::config::save_config(state);
                changed = true;
            }

            let slider = egui::Slider::new(&mut state.video.overscan_y, -0.2..=0.2)
                .step_by(0.001)
                .custom_formatter(|n, _| format!("{:.1}%", n * 100.0))
                .custom_parser(|s| {
                    let s = s.trim().trim_end_matches('%').trim();
                    s.parse::<f64>().ok().map(|v| if v.abs() > 0.2 { (v / 100.0) as f64 } else { v })
                });
            if slider_item(c2, "Overscan Y:", slider, None).changed() {
                crate::config::save_config(state);
                changed = true;
            }
        });

        two_columns(ui, |c1, c2| {
            let slider = egui::Slider::new(&mut state.video.underscan_x, -0.2..=0.3)
                .step_by(0.0005)
                .custom_formatter(|n, _| format!("{:.1}%", (1.0 + n) * 100.0))
                .custom_parser(|s| {
                    let s = s.trim().trim_end_matches('%').trim();
                    s.parse::<f64>().ok().map(|p| if p > 50.0 { (p / 100.0 - 1.0) as f64 } else { p })
                });
            if slider_item(
                c1,
                "Underscan Stretch X:",
                slider,
                Some("Stretches the image raster horizontally inside the rendering surface without changing screen boundaries, cutting off excess edges."),
            )
            .changed()
            {
                crate::config::save_config(state);
                changed = true;
            }

            let slider = egui::Slider::new(&mut state.video.underscan_y, -0.2..=0.3)
                .step_by(0.001)
                .custom_formatter(|n, _| format!("{:.1}%", (1.0 + n) * 100.0))
                .custom_parser(|s| {
                    let s = s.trim().trim_end_matches('%').trim();
                    s.parse::<f64>().ok().map(|p| if p > 50.0 { (p / 100.0 - 1.0) as f64 } else { p })
                });
            if slider_item(
                c2,
                "Underscan Stretch Y:",
                slider,
                Some("Stretches the image raster vertically inside the rendering surface without changing screen boundaries, cutting off excess edges."),
            )
            .changed()
            {
                crate::config::save_config(state);
                changed = true;
            }
        });

        technical_separator(ui);

        sub_heading(ui, "Border Cut-off (Pillowing Mask):");
        two_columns(ui, |c1, c2| {
            let slider = egui::Slider::new(&mut state.video.border_crop_top, 0.0..=0.2)
                .step_by(0.001)
                .custom_formatter(|n, _| format!("{:.1}%", n * 100.0))
                .custom_parser(|s| {
                    let s = s.trim().trim_end_matches('%').trim();
                    s.parse::<f64>().ok().map(|v| if v > 0.2 { (v / 100.0) as f64 } else { v })
                });
            if slider_item(c1, "Top Cut-off:", slider, None).changed() {
                crate::config::save_config(state);
                changed = true;
            }

            let slider = egui::Slider::new(&mut state.video.border_crop_bottom, 0.0..=0.2)
                .step_by(0.001)
                .custom_formatter(|n, _| format!("{:.1}%", n * 100.0))
                .custom_parser(|s| {
                    let s = s.trim().trim_end_matches('%').trim();
                    s.parse::<f64>().ok().map(|v| if v > 0.2 { (v / 100.0) as f64 } else { v })
                });
            if slider_item(c2, "Bottom Cut-off:", slider, None).changed() {
                crate::config::save_config(state);
                changed = true;
            }
        });

        two_columns(ui, |c1, c2| {
            let slider = egui::Slider::new(&mut state.video.border_crop_left, 0.0..=0.2)
                .step_by(0.001)
                .custom_formatter(|n, _| format!("{:.1}%", n * 100.0))
                .custom_parser(|s| {
                    let s = s.trim().trim_end_matches('%').trim();
                    s.parse::<f64>().ok().map(|v| if v > 0.2 { (v / 100.0) as f64 } else { v })
                });
            if slider_item(c1, "Left Cut-off:", slider, None).changed() {
                crate::config::save_config(state);
                changed = true;
            }

            let slider = egui::Slider::new(&mut state.video.border_crop_right, 0.0..=0.2)
                .step_by(0.001)
                .custom_formatter(|n, _| format!("{:.1}%", n * 100.0))
                .custom_parser(|s| {
                    let s = s.trim().trim_end_matches('%').trim();
                    s.parse::<f64>().ok().map(|v| if v > 0.2 { (v / 100.0) as f64 } else { v })
                });
            if slider_item(c2, "Right Cut-off:", slider, None).changed() {
                crate::config::save_config(state);
                changed = true;
            }
        });
    });

    changed
}

pub fn draw_effects_tab(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;

    // Group 1: Visual Tweaks
    technical_group(ui, "Visual Tweaks:", |ui| {
        if ui
            .checkbox(
                &mut state.video.use_magenta_background,
                "Magenta Background",
            )
            .on_hover_text("Uses a magenta background around the video stream instead of black.")
            .changed()
        {
            crate::config::save_config(state);
            changed = true;
        }

        technical_separator(ui);

        if ui
            .checkbox(
                &mut state.video.retro_pc_frame,
                "Retro PC Monitor Frame",
            )
            .on_hover_text(
                "Displays a vintage NEC PC-98 CRT monitor casing in empty areas around the screen (requires CRT shader).",
            )
            .changed()
        {
            crate::config::save_config(state);
            changed = true;
        }

        if state.video.retro_pc_frame {
            ui.add_space(3.0);
            two_columns(ui, |c1, c2| {
                let slider = egui::Slider::new(&mut state.video.retro_pc_ambient_glow, 0.0..=1.0)
                    .step_by(0.05)
                    .custom_formatter(|n, _| {
                        if n <= 0.001 {
                            "Off".to_string()
                        } else {
                            format!("{:.0}%", n * 100.0)
                        }
                    })
                    .custom_parser(|s| {
                        let s = s.trim().trim_end_matches('%').trim();
                        if s.eq_ignore_ascii_case("off") {
                            Some(0.0)
                        } else {
                            s.parse::<f64>().ok().map(|v| if v > 1.0 { (v / 100.0) as f64 } else { v })
                        }
                    });
                if slider_item(
                    c1,
                    "Bezel Ambient Glow:",
                    slider,
                    Some("Controls the intensity of the real-time diffuse halo glow reflecting from the active video feed onto the deep inner bezel sides (0% = Off)."),
                )
                .changed()
                {
                    crate::config::save_config(state);
                    changed = true;
                }

                c2.horizontal(|ui| {
                    if ui
                        .checkbox(
                            &mut state.video.retro_pc_frame_dark_mode,
                            "Dark Room Bezel",
                        )
                        .on_hover_text(
                            "Tones down bezel brightness as if room lights are turned off (automatically enabled during Lights Off Night Mode).",
                        )
                        .changed()
                    {
                        crate::config::save_config(state);
                        changed = true;
                    }
                });
            });
        }

        technical_separator(ui);

        if ui
            .checkbox(
                &mut state.video.crt_glass_enabled,
                "Glossy Screen Glass Effect",
            )
            .on_hover_text(
                "Simulates a photorealistic curved CRT glass patina, refractions, and reflections reacting to screen light (requires CRT shader).",
            )
            .changed()
        {
            crate::config::save_config(state);
            changed = true;
        }

        if state.video.crt_glass_enabled {
            ui.add_space(3.0);
            two_columns(ui, |c1, c2| {
                let slider = percent_slider(&mut state.video.crt_glass_intensity, 0.0..=1.0);
                if slider_item(
                    c1,
                    "Glass Intensity:",
                    slider,
                    Some("Controls the intensity of the glass patina, refractions, and reflections."),
                )
                .changed()
                {
                    crate::config::save_config(state);
                    changed = true;
                }

                let slider = percent_slider(&mut state.video.crt_glass_glossiness, 0.0..=1.0);
                if slider_item(
                    c2,
                    "Glass Glossiness:",
                    slider,
                    Some("Controls specular sharpness and surface glossiness of the glass patina."),
                )
                .changed()
                {
                    crate::config::save_config(state);
                    changed = true;
                }
            });

            ui.add_space(3.0);
            ui.horizontal(|ui| {
                if ui
                    .checkbox(
                        &mut state.video.crt_glass_ceiling_light_enabled,
                        "Ceiling Light Reflections",
                    )
                    .on_hover_text(
                        "Simulates overhead twin-tube fluorescent office light reflections on the glass surface.",
                    )
                    .changed()
                {
                    crate::config::save_config(state);
                    changed = true;
                }
            });

            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if ui
                    .checkbox(
                        &mut state.video.crt_glass_photographer_enabled,
                        "Photographer Reflection",
                    )
                    .on_hover_text(
                        "Simulates an anonymous, diffuse dark silhouette reflection of a person taking a photo with a smartphone.",
                    )
                    .changed()
                {
                    crate::config::save_config(state);
                    changed = true;
                }

                if state.video.crt_glass_photographer_enabled {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("Photographer Intensity:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    );
                    let slider = percent_slider(&mut state.video.crt_glass_photographer_intensity, 0.0..=1.0);
                    if ui.add(slider).changed() {
                        crate::config::save_config(state);
                        changed = true;
                    }
                }
            });

            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if ui
                    .checkbox(
                        &mut state.video.crt_glass_flash_enabled,
                        "Camera Flash Reflection",
                    )
                    .on_hover_text(
                        "Simulates a warm diffuse camera flash reflection on the lower-right area of the screen.",
                    )
                    .changed()
                {
                    crate::config::save_config(state);
                    changed = true;
                }

                if state.video.crt_glass_flash_enabled {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("Flash Intensity:")
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(153, 153, 153)),
                    );
                    let slider = percent_slider(&mut state.video.crt_glass_flash_intensity, 0.0..=1.0);
                    if ui.add(slider).changed() {
                        crate::config::save_config(state);
                        changed = true;
                    }
                }
            });

            if state.video.lights_off_night_mode {
                ui.label(
                    egui::RichText::new(
                        "🌙 Ceiling & camera reflections are suspended by Lights Off Night Mode.",
                    )
                    .weak()
                    .small(),
                );
            }
        }

        technical_separator(ui);

        ui.horizontal(|ui| {
            if ui
                .checkbox(
                    &mut state.video.lights_off_night_mode,
                    "Lights Off Night Mode (K)",
                )
                .on_hover_text(
                    "Simulates turning off room lights: darkens CRT bezels, enhances ambient glow reflections, and disables ceiling/camera flash reflections (press 'K').",
                )
                .changed()
            {
                crate::config::save_config(state);
                changed = true;
            }

            if state.video.lights_off_night_mode {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Night Mode Halo Glow:")
                        .monospace()
                        .size(11.0)
                        .color(egui::Color32::from_rgb(153, 153, 153)),
                );
                let slider = egui::Slider::new(&mut state.video.night_mode_glow_intensity, 0.0..=1.0)
                    .step_by(0.05)
                    .custom_formatter(|n, _| {
                        if n <= 0.001 {
                            "Off".to_string()
                        } else {
                            format!("{:.0}%", n * 100.0)
                        }
                    })
                    .custom_parser(|s| {
                        let s = s.trim().trim_end_matches('%').trim();
                        if s.eq_ignore_ascii_case("off") {
                            Some(0.0)
                        } else {
                            s.parse::<f64>().ok().map(|v| if v > 1.0 { (v / 100.0) as f64 } else { v })
                        }
                    });
                if ui.add(slider).changed() {
                    crate::config::save_config(state);
                    changed = true;
                }
            }
        });
    });

    ui.add_space(4.0);

    // Group 2: Cathode Glow & Interference
    technical_group(ui, "", |ui| {
        ui.horizontal(|ui| {
            if ui
                .checkbox(
                    &mut state.cathode_interference.enabled,
                    "Cathode Glow & Interference",
                )
                .on_hover_text(
                    "Simulates CRT cathode ray flickering, electrical bloom, and subtle analog RF interference.",
                )
                .changed()
            {
                changed = true;
            }

            if state.cathode_interference.enabled {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Save as Defaults").clicked() {
                        state.cathode_interference_defaults = state.cathode_interference.clone();
                        let current_profile_data = crate::config::build_profile_from_state(state);
                        state
                            .profiles
                            .insert(state.active_profile.clone(), current_profile_data);
                        crate::config::save_config(state);
                        state.info("Saved current cathode settings as default");
                        changed = true;
                    }
                    if ui.button("Reset Defaults").clicked() {
                        state.cathode_interference = state.cathode_interference_defaults.clone();
                        changed = true;
                    }
                });
            }
        });

        if state.cathode_interference.enabled {
            ui.add_space(4.0);
            two_columns(ui, |c1, c2| {
                let slider = percent_slider(&mut state.cathode_interference.intensity, 0.0..=1.0);
                if slider_item(
                    c1,
                    "Intensity:",
                    slider,
                    Some("Master intensity of cathode glow, flicker, and interference."),
                )
                .changed()
                {
                    changed = true;
                }

                let slider = mult_slider(&mut state.cathode_interference.frequency, 0.1..=5.0);
                if slider_item(
                    c2,
                    "Frequency:",
                    slider,
                    Some("Speed of AC hum ripple, flicker rate, and glitch frequency."),
                )
                .changed()
                {
                    changed = true;
                }
            });

            two_columns(ui, |c1, c2| {
                let slider = percent_slider(&mut state.cathode_interference.randomization, 0.0..=1.0);
                if slider_item(
                    c1,
                    "Randomization:",
                    slider,
                    Some("Unpredictability and jitteriness of flicker and micro-glitches."),
                )
                .changed()
                {
                    changed = true;
                }

                let slider = percent_slider(&mut state.cathode_interference.electricity_glow, 0.0..=1.0);
                if slider_item(
                    c2,
                    "Electricity Glow:",
                    slider,
                    Some("Lightbulb/cathode bloom that selectively energizes highlights and dynamically modulates dark areas."),
                )
                .changed()
                {
                    changed = true;
                }
            });

            two_columns(ui, |c1, c2| {
                let slider = percent_slider(&mut state.cathode_interference.flicker_depth, 0.0..=1.0);
                if slider_item(
                    c1,
                    "Flicker Depth:",
                    slider,
                    Some("Depth of rolling AC power hum and high-frequency cathode phosphor flutter."),
                )
                .changed()
                {
                    changed = true;
                }

                let slider = percent_slider(&mut state.cathode_interference.interference, 0.0..=1.0);
                if slider_item(
                    c2,
                    "Interference / Noise:",
                    slider,
                    Some("Analog RF static noise and intermittent horizontal scanline micro-glitches."),
                )
                .changed()
                {
                    changed = true;
                }
            });

            two_columns(ui, |c1, _| {
                let slider = percent_slider(&mut state.cathode_interference.lightbulb_effect, 0.0..=1.0);
                if slider_item(
                    c1,
                    "Lightbulb Effect:",
                    slider,
                    Some("Pulsating lightbulb breathing that makes the brightness of bright areas and darkness of dark areas glow up and down."),
                )
                .changed()
                {
                    changed = true;
                }
            });
        }
    });

    changed
}

#[allow(dead_code)]
pub fn draw_filters(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;
    changed |= draw_shaders_tab(ui, state);
    changed |= draw_geometry_tab(ui, state);
    changed |= draw_effects_tab(ui, state);
    changed
}
