use crate::{app::AppState, config};
use eframe::egui;

pub fn draw_profile_management(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label("Profile:");
            let pre_selected = state.active_profile.clone();
            // Try to avoid allocations/sorting on every frame by just pulling from keys directly if possible,
            // or simply using a BTreeMap in AppState/Config instead. Leaving as Vec/sort since it's small,
            // but we can optimize this later if it's a measurable hotspot.
            let mut combo_changed = false;
            egui::ComboBox::from_id_source("profile_selector")
                .selected_text(&state.active_profile)
                .show_ui(ui, |ui| {
                    for key in state.profiles.keys() {
                        combo_changed |= ui
                            .selectable_value(&mut state.active_profile, key.clone(), key)
                            .changed();
                    }
                });


            if combo_changed && pre_selected != state.active_profile {
                let profile_to_apply = state.profiles.get(&state.active_profile).cloned();
                if let Some(profile) = profile_to_apply {
                    config::apply_profile_to_state(state, &profile);
                }
                config::save_config(state);
                state.info(format!("Switched to profile: {}", state.active_profile));
                changed = true;
            }

            if ui.button("Save Config").clicked() {
                let current_profile_data = config::build_profile_from_state(state);
                state
                    .profiles
                    .insert(state.active_profile.clone(), current_profile_data);
                config::save_config(state);
                state.info(format!("Saved configuration to profile: {}", state.active_profile));
                changed = true;
            }

            ui.separator();
            ui.label("New Profile:");
            ui.add(egui::TextEdit::singleline(&mut state.new_profile_name).desired_width(100.0));
            if ui
                .add_enabled(
                    !state.new_profile_name.trim().is_empty()
                        && !state.profiles.contains_key(state.new_profile_name.trim()),
                    egui::Button::new("Add"),
                )
                .clicked()
            {
                let new_profile_name = state.new_profile_name.trim().to_string();
                let current_profile_data = config::build_profile_from_state(state);
                state
                    .profiles
                    .insert(new_profile_name.clone(), current_profile_data);
                state.active_profile = new_profile_name;
                state.new_profile_name.clear();

                config::save_config(state); // Save immediately on profile creation
                changed = true;
                state.info(format!("Created and switched to profile: {}", state.active_profile));
            }

            if ui
                .add_enabled(
                    state.profiles.len() > 1,
                    egui::Button::new("Delete").fill(egui::Color32::from_rgb(180, 50, 50)),
                )
                .clicked()
            {
                state.profiles.remove(&state.active_profile);
                if state.profiles.contains_key("Default") {
                    state.active_profile = "Default".to_string();
                } else {
                    state.active_profile = state.profiles.keys().next().unwrap().clone();
                }
                let profile_to_apply = state.profiles.get(&state.active_profile).cloned();
                if let Some(profile) = profile_to_apply {
                    config::apply_profile_to_state(state, &profile);
                }

                config::save_config(state); // Save immediately on profile deletion
                changed = true;
                state.info(format!("Deleted profile. Switched to: {}", state.active_profile));
            }
        });
    });
    changed
}
