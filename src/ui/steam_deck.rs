//! Single native viewport, used only with --steam-deck.
use crate::app::AppState;
use crate::devices::filter_type::CrtFilter;
use crate::steam_deck::{self, Settings, Source, Tab};
use crate::{config, ui};
use eframe::egui;
use std::sync::atomic::Ordering;

pub fn draw(state: &mut AppState, ctx: &egui::Context) -> bool {
    let was_running = state.ui.video_window_open;
    steam_deck::shortcuts(state, ctx);
    let selected_pitch = state
        .steam_deck
        .as_ref()
        .unwrap()
        .pitch_picker
        .as_ref()
        .and_then(|rx| rx.try_recv().ok());
    if let Some(path) = selected_pitch {
        state.steam_deck.as_mut().unwrap().pitch_picker = None;
        state.dict.trigger_pitch_import(path);
    }
    let selected_presets = state
        .steam_deck
        .as_ref()
        .unwrap()
        .preset_picker
        .as_ref()
        .and_then(|rx| rx.try_recv().ok());
    if let Some(path) = selected_presets {
        state.steam_deck.as_mut().unwrap().preset_picker = None;
        if let Err(error) = import_presets(state, &path) {
            state.error(error.to_string());
        }
    }
    let tab = state.steam_deck.as_ref().unwrap().tab;
    let launching = state.ui.video_window_open
        && state.gamescope_hook.is_some()
        && state.latest_frame.is_none();
    state.ui.control_window_open = tab == Tab::Settings;
    if tab != Tab::Game {
        state.dict.popup = None;
        state.steam_deck.as_mut().unwrap().pinned_dictionary = false;
        if let Some(renderer) = &state.crt_renderer {
            renderer.lock().unwrap().popup_primitives.clear();
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::None));
        ctx.set_cursor_icon(egui::CursorIcon::Default);
    }
    if (tab != Tab::Game || launching) && state.ui.video_window_open {
        // Render/capture the live game first, then cover it with the active tab.
        // This keeps replay and OCR capture running without creating another window.
        let rect = ctx.available_rect();
        let mut background = egui::Ui::new(
            ctx.clone(),
            egui::LayerId::background(),
            egui::Id::new("deck_background_game"),
            rect,
            rect,
        );
        ui::draw_video_player(state, &mut background, ctx);
    }
    match tab {
        Tab::Bank => ui::bank::draw_contents(state, ctx),
        Tab::Game if !state.ui.video_window_open || launching => {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::both()
                    .id_source("deck_launcher_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| launcher(state, ui));
            });
        }
        Tab::Game => {
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    ui::draw_video_player(state, ui, ctx);
                });
        }
        Tab::Settings => {
            egui::CentralPanel::default().show(ctx, |ui| {
                settings(state, ui);
            });
        }
    }
    if !was_running && state.ui.video_window_open {
        state.steam_deck.as_mut().unwrap().tab = Tab::Game;
    }
    if let Some(picker) = state.steam_deck.as_mut().unwrap().file_picker.as_mut() {
        if picker.show(ctx) {
            state.steam_deck.as_mut().unwrap().file_picker = None;
            ctx.request_repaint();
        }
    }
    // L4/R4 or a touched settings tab can hide the tag field before it gets a
    // frame to report lost focus. Preserve those edits just as a normal blur does.
    if state.bank.current_tag != state.bank.saved_current_tag && !ctx.wants_keyboard_input() {
        ui::bank::commit_tag_setting(state);
    }
    false
}

fn launcher(state: &mut AppState, ui: &mut egui::Ui) {
    if state.ui.video_window_open && state.gamescope_hook.is_some() {
        ui.horizontal_wrapped(|ui| {
            ui.spinner();
            if ui.button("Back to launcher").clicked() {
                state.stop_stream(ui.ctx());
            }
        });
        return;
    }
    ui.heading("Michadame · Steam Deck");
    ui.label(
        "Choose a game. Use L4 / R4 to move through Word bank, Main game and each settings tab.",
    );
    let mut source = state.steam_deck.as_ref().unwrap().source;
    ui.horizontal_wrapped(|ui| {
        for (value, label) in [
            (Source::Steam, "Steam games"),
            (Source::Pc88, "PC-88"),
            (Source::Pc98, "PC-98"),
        ] {
            ui.selectable_value(&mut source, value, label);
        }
    });
    steam_deck::change_source(state, source);
    if source != Source::Steam {
        state.retro_pc_system = if source == Source::Pc88 {
            crate::pc88::RetroPcSystem::Pc88
        } else {
            crate::pc88::RetroPcSystem::Pc98
        };
        ui::pc98::draw_pc98_tab(ui, state);
        return;
    }
    let selected = state
        .steam_games
        .iter()
        .find(|game| Some(&game.appid) == state.selected_steam_game.as_ref())
        .map(|g| g.name.as_str())
        .unwrap_or("Select an installed Steam game");
    egui::ComboBox::from_id_source("deck_steam_games")
        .selected_text(selected)
        .width((ui.available_width() - 20.0).max(100.0))
        .show_ui(ui, |ui| {
            let mut changed = false;
            for game in &state.steam_games {
                changed |= ui
                    .selectable_value(
                        &mut state.selected_steam_game,
                        Some(game.appid.clone()),
                        &game.name,
                    )
                    .changed();
            }
            if changed {
                config::save_config(state);
            }
        });
    if state.steam_games.is_empty() {
        ui.label("No installed Steam games found. Mount your Steam library and refresh.");
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                state.selected_steam_game.is_some(),
                egui::Button::new("Launch game").min_size(egui::vec2(220.0, 56.0)),
            )
            .clicked()
        {
            state.start_gamescope_hook(ui.ctx());
        }
        if ui.button("Refresh games").clicked() {
            state.refresh_steam_games();
        }
    });
}

fn settings(state: &mut AppState, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        let deck = state.steam_deck.as_mut().unwrap();
        for (tab, name) in Settings::TABS {
            ui.selectable_value(&mut deck.settings, tab, name);
        }
    });
    ui.separator();
    let selected_settings = state.steam_deck.as_ref().unwrap().settings;
    egui::ScrollArea::both().id_source(("deck_settings_scroll", selected_settings as u8)).auto_shrink([false,false]).show(ui, |ui| {
    let mut changed = false;
    match selected_settings {
        Settings::Primary => primary(state, ui),
        Settings::Setup => setup(state, ui),
        Settings::Shaders => {
            changed |= ui::filters::draw_shaders_tab(ui, state);
        }
        Settings::Geometry => {
            changed |= ui::filters::draw_geometry_bezel_tab(ui, state);
        }
        Settings::Effects => {
            changed |= ui::filters::draw_special_fx_tab(ui, state);
        }
        Settings::Replay => {
            changed |= crate::replay::ui::draw_replay_buffer_group(
                &mut state.replay,
                ui,
                state.ui.video_window_open,
            );
        }
        Settings::Audio => {
            changed |= ui
                .add(
                    egui::Slider::new(&mut state.hardware.master_volume, 0.0..=1.0)
                        .text("Game volume"),
                )
                .changed();
            if changed {
                state.set_master_volume(state.hardware.master_volume);
            }
            changed |= crate::replay::ui::draw_audio_filter_contents(&mut state.replay, ui);
        }
        Settings::Ocr => ui::controls::draw_ocr_dict_tab(ui, state, &mut changed),
        Settings::RetroPc => {
            changed |= ui::pc98::draw_pc98_tab(ui, state);
        }
        Settings::Controls => {
            ui.heading("Rear buttons (Steam Input)");
            for line in [
                "L4   Ctrl+Shift+Left — previous tab",
                "R4   Ctrl+Shift+Right — next tab",
                "L5   Ctrl+Alt+Space — hook / unhook cursor",
                "R5   H — open / close whiteboard",
            ] {
                ui.label(line);
            }
            ui.label("Keep the regular controls mapped to the game. Use the touchscreen for Michadame and tap an OCR word to open its dictionary popup.");
            ui.label("Configure the four rear bindings in the Steam Input layouts used by Michadame and the launched game.");
        }
    }
    if changed {
        config::save_config(state);
    }
    });
}

pub(crate) fn import_presets(state: &mut AppState, path: &std::path::Path) -> anyhow::Result<()> {
    let configuration: config::MichadameConfig = confy::load_path(path)?;
    let mut profiles = configuration.profiles;
    for (name, profile) in [
        ("Steam", configuration.gamescope_profile),
        ("PC-88", configuration.pc88_profile),
        ("PC-98", configuration.pc98_profile),
    ] {
        if let Some(profile) = profile {
            profiles.insert(name.into(), profile);
        }
    }
    anyhow::ensure!(
        !profiles.is_empty(),
        "No presets found in {}",
        path.display()
    );
    for (name, mut profile) in profiles {
        profile.video_format_fourcc = None;
        profile.video_resolution = None;
        profile.video_framerate = None;
        profile.audio_source = None;
        let mut imported = name.clone();
        let mut number = 2;
        while state.profiles.contains_key(&imported) {
            imported = format!("{name} ({number})");
            number += 1;
        }
        state.profiles.insert(imported, profile);
    }
    config::save_config(state);
    Ok(())
}

fn setup(state: &mut AppState, ui: &mut egui::Ui) {
    ui.heading("Mining bank");
    ui::bank::draw_tag_setting(ui, state);
    if ui.button("Save mining tag").clicked() {
        ui::bank::commit_tag_setting(state);
    }
    ui.label("This tag is applied to newly mined words. Leave it empty to mine without a tag.");
    ui.separator();
    ui.heading("Presets");
    ui.add_enabled_ui(!state.ui.video_window_open, |ui| {
        let mut source = state.steam_deck.as_ref().unwrap().source;
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut source, Source::Steam, "Steam game profile");
            ui.selectable_value(&mut source, Source::Pc88, "PC-88 profile");
            ui.selectable_value(&mut source, Source::Pc98, "PC-98 profile");
        });
        steam_deck::change_source(state, source);
    });
    let presets: Vec<_> = state.profiles.keys().cloned().collect();
    let deck = state.steam_deck.as_mut().unwrap();
    egui::ComboBox::from_id_source("deck_preset_selector")
        .selected_text(if deck.selected_preset.is_empty() {
            "Select a preset"
        } else {
            &deck.selected_preset
        })
        .width(240.0)
        .show_ui(ui, |ui| {
            for name in presets {
                ui.selectable_value(&mut deck.selected_preset, name.clone(), name);
            }
        });
    if ui.button("Apply preset to this profile").clicked() {
        let name = state.steam_deck.as_ref().unwrap().selected_preset.clone();
        if let Some(profile) = state.profiles.get(&name).cloned() {
            config::apply_profile_to_state(state, &profile);
            config::save_config(state);
        }
    }
    ui.add_sized(
        [ui.available_width().min(320.0), 40.0],
        egui::TextEdit::singleline(&mut state.steam_deck.as_mut().unwrap().preset_name)
            .hint_text("Preset name"),
    );
    if ui.button("Save current settings as preset").clicked() {
        let name = state
            .steam_deck
            .as_ref()
            .unwrap()
            .preset_name
            .trim()
            .to_owned();
        if !name.is_empty() {
            state
                .profiles
                .insert(name.clone(), config::build_profile_from_state(state));
            state.steam_deck.as_mut().unwrap().selected_preset = name;
            config::save_config(state);
        }
    }
    if ui.button("Import presets from Michadame config…").clicked() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let deck = state.steam_deck.as_mut().unwrap();
        deck.preset_picker = Some(rx);
        deck.file_picker = Some(ui::file_picker::FilePicker::new(
            "Import presets from TOML",
            std::path::Path::new(""),
            false,
            Some("toml"),
            move |path| {
                let _ = tx.send(path);
            },
        ));
    }
}

fn toggle(ui: &mut egui::Ui, label: &str, enabled: bool) -> bool {
    ui.add_sized(
        [ui.available_width(), 64.0],
        egui::Button::new(format!("{label}: {}", if enabled { "ON" } else { "OFF" }))
            .selected(enabled),
    )
    .clicked()
}

fn primary(state: &mut AppState, ui: &mut egui::Ui) {
    let mut changed = false;
    ui.columns(2, |cols| {
        let current = CrtFilter::from_u8(state.crt_filter.load(Ordering::Relaxed));
        if toggle(&mut cols[0], "CRT", current != CrtFilter::Off) {
            let next = if current != CrtFilter::Off {
                CrtFilter::Off
            } else if state.selected_crt_filter != CrtFilter::Off {
                state.selected_crt_filter
            } else {
                CrtFilter::Lottes
            };
            state.crt_filter.store(next as u8, Ordering::Relaxed);
            changed = true;
        }
        if toggle(&mut cols[1], "OCR boxes", !state.ocr.hide_overlay) {
            state.ocr.hide_overlay = !state.ocr.hide_overlay;
            changed = true;
        }
        if toggle(&mut cols[0], "Lights", !state.video.lights_off_night_mode) {
            state.toggle_lights_off_night_mode();
            changed = true;
        }
        if cols[1]
            .add_enabled_ui(
                state.ui.video_window_open || state.replay.runtime.is_some(),
                |ui| toggle(ui, "Replay buffer", state.replay.runtime.is_some()),
            )
            .inner
        {
            if state.replay.runtime.is_some() {
                state.replay.disable();
            } else if let Err(error) = state.replay.enable() {
                state.error(format!("Could not enable replay: {error}"));
            }
        }
    });
    if ui
        .add_enabled(
            state.replay.runtime.is_some(),
            egui::Button::new("Save last 30 seconds to disk")
                .min_size(egui::vec2(ui.available_width(), 64.0)),
        )
        .clicked()
    {
        if let Err(error) = state.replay.save(30) {
            state.error(format!("Could not save replay: {error}"));
        }
    }
    if ui.button("Capture OCR now").clicked() && state.ui.video_window_open {
        state.ocr.capture_requested.store(true, Ordering::Release);
    }
    if ui.button("Hook / unhook cursor (L5)").clicked() {
        steam_deck::toggle_cursor(state, ui.ctx());
    }
    if state.ui.video_window_open && ui.button("Return to game launcher").clicked() {
        state.stop_stream(ui.ctx());
        state.steam_deck.as_mut().unwrap().tab = Tab::Game;
    }
    if changed {
        config::save_config(state);
    }
}
