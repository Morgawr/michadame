//! Opt-in Steam Deck state. Desktop startup never constructs this state.
use crate::{app::AppState, config};
use eframe::egui;
use std::collections::HashSet;
pub mod capture;
pub mod gamepad;
pub mod launch;
pub mod logging;
#[cfg(test)]
mod tests;

pub const UI_ZOOM: f32 = 0.5;

pub fn frame_interval(running: bool) -> std::time::Duration {
    std::time::Duration::from_nanos(1_000_000_000 / if running { 60 } else { 30 })
}

/// Keep the dictionary's physical size independent of the compact setup UI.
pub fn dictionary_scale(ctx: &egui::Context) -> f32 {
    0.2 / ctx.zoom_factor()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    Bank,
    #[default]
    Game,
    Settings,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    #[default]
    Steam,
    Pc88,
    Pc98,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Settings {
    #[default]
    Primary,
    Setup,
    Shaders,
    Geometry,
    Effects,
    Replay,
    Audio,
    Ocr,
    RetroPc,
    Controls,
}

impl Settings {
    /// The on-screen tabs and rear-button navigation share this order.
    pub const TABS: [(Self, &'static str); 10] = [
        (Self::Primary, "Primary"),
        (Self::Setup, "Setup & presets"),
        (Self::Shaders, "Shaders"),
        (Self::Geometry, "Geometry & bezel"),
        (Self::Effects, "Special FX"),
        (Self::Replay, "Replay"),
        (Self::Audio, "Game audio"),
        (Self::Ocr, "OCR & dictionary"),
        (Self::RetroPc, "PC-88 / PC-98"),
        (Self::Controls, "Controls"),
    ];
}

#[derive(Default)]
pub struct SteamDeckState {
    pub tab: Tab,
    pub settings: Settings,
    pub source: Source,
    pub pinned_dictionary: bool,
    pub reserved_keys: HashSet<egui::Key>,
    pub session: Option<std::path::PathBuf>,
    pub viewer_lease: Option<std::fs::File>,
    pub file_picker: Option<crate::ui::file_picker::FilePicker>,
    pub pitch_picker: Option<crossbeam_channel::Receiver<std::path::PathBuf>>,
    pub preset_picker: Option<crossbeam_channel::Receiver<std::path::PathBuf>>,
    pub selected_preset: String,
    pub preset_name: String,
    pub capture_target: Option<String>,
    pub game_display: Option<String>,
    pub launch_started: Option<std::time::Instant>,
    pub forwarded_keys: HashSet<egui::Key>,
    pub mouse_buttons: [bool; 3],
    pub ui_touch: bool,
    pub gamepad: Option<gamepad::Gamepad>,
    pub gamepad_error: Option<String>,
    pub last_launch_poll: Option<std::time::Instant>,
    pub original_appid: Option<u32>,
    pub game_appid: Option<u32>,
    pub focus_display: Option<String>,
    pub focus_handoff_pending: bool,
    pub menu_focus_override: u32,
    pub capture_scan: Option<crossbeam_channel::Receiver<Option<String>>>,
    pub video_capture: Option<capture::Capture>,
    pub capture_error: Option<String>,
    pub capture_failures: u32,
    pub last_frame_stats_log: Option<std::time::Instant>,
}

impl SteamDeckState {
    fn step_tab(&mut self, right: bool) {
        match (self.tab, right) {
            (Tab::Bank, true) => self.tab = Tab::Game,
            (Tab::Game, false) => self.tab = Tab::Bank,
            (Tab::Game, true) => {
                self.tab = Tab::Settings;
                self.settings = Settings::Primary;
            }
            (Tab::Settings, right) => {
                let current = Settings::TABS
                    .iter()
                    .position(|(tab, _)| *tab == self.settings)
                    .expect("Every settings section has a tab");
                let next = if right {
                    Some(current + 1)
                } else {
                    current.checked_sub(1)
                };
                if let Some((tab, _)) = next.and_then(|index| Settings::TABS.get(index)) {
                    self.settings = *tab;
                } else if !right {
                    self.tab = Tab::Game;
                }
            }
            _ => {}
        }
    }
}

pub fn configure(state: &mut AppState, ctx: &egui::Context) {
    enforce_restrictions(state);
    state.ui.is_fullscreen = true;
    state.ui.control_window_open = false;
    ctx.set_zoom_factor(UI_ZOOM);
    let mut style = (*ctx.style()).clone();
    style.spacing.interact_size.y = 40.0;
    style.spacing.button_padding = egui::vec2(12.0, 10.0);
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.scroll.bar_width = 14.0;
    style.spacing.scroll.floating = false;
    style.interaction.tooltip_delay = f32::INFINITY;
    ctx.set_style(style);
    config::apply_profile_to_state(state, &state.gamescope_profile.clone());
}

pub fn enforce_restrictions(state: &mut AppState) {
    state.ui.show_first_run_dialog = false;
    state.ui.reset_usb_on_startup = false;
    state.ui.studio_window_open = false;
    state.mic.config.enabled = false;
    state.webcam.config.enabled = false;
    state.twitch.apply_config(Default::default());
    state.twitch.config.chat_overlay_enabled = false;
    state.twitch.config.niconico_enabled = false;
    state.twitch.config.alerts_enabled = false;
}

pub fn change_source(state: &mut AppState, source: Source) {
    if state.steam_deck.as_ref().unwrap().source == source {
        return;
    }
    config::save_config(state);
    state.steam_deck.as_mut().unwrap().source = source;
    match source {
        Source::Pc88 => state.retro_pc_system = crate::pc88::RetroPcSystem::Pc88,
        Source::Pc98 => state.retro_pc_system = crate::pc88::RetroPcSystem::Pc98,
        Source::Steam => {}
    }
    let profile = match source {
        Source::Steam => &state.gamescope_profile,
        Source::Pc88 => &state.pc88_profile,
        Source::Pc98 => &state.pc98_profile,
    }
    .clone();
    config::apply_profile_to_state(state, &profile);
}

/// Consume only Deck actions, including their releases, before any game/UI sees them.
/// Ordinary keyboard/gamepad mappings never become desktop application shortcuts.
pub fn shortcuts(state: &mut AppState, ctx: &egui::Context) {
    let mut actions = Vec::new();
    let typing = ctx.wants_keyboard_input();
    let deck = state.steam_deck.as_mut().unwrap();
    ctx.input_mut(|input| {
        if !input.focused {
            deck.reserved_keys.clear();
        }
        input.events.retain(|event| {
            let egui::Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } = event
            else {
                return true;
            };
            let action = if modifiers.ctrl && modifiers.shift && !modifiers.alt {
                match key {
                    egui::Key::ArrowLeft => Some(0),
                    egui::Key::ArrowRight => Some(1),
                    _ => None,
                }
            } else if *key == egui::Key::H && modifiers.is_none() && !typing {
                Some(3)
            } else if *key == egui::Key::Space
                && modifiers.ctrl
                && modifiers.alt
                && !modifiers.shift
            {
                Some(2)
            } else {
                None
            };
            if *pressed {
                if let Some(action) = action {
                    deck.reserved_keys.insert(*key);
                    if !repeat {
                        actions.push(action);
                    }
                    return false;
                }
            }
            if !pressed && deck.reserved_keys.remove(key) {
                return false;
            }
            action.is_none()
        });
    });
    for action in actions {
        match action {
            0 | 1 => {
                let deck = state.steam_deck.as_mut().unwrap();
                deck.step_tab(action == 1);
                state.dict.popup = None;
                deck.pinned_dictionary = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::None));
                ctx.memory_mut(|m| {
                    if let Some(id) = m.focused() {
                        m.surrender_focus(id);
                    }
                });
            }
            2 => toggle_cursor(state, ctx),
            3 => {
                state.steam_deck.as_mut().unwrap().tab = Tab::Game;
                state.whiteboard.toggle();
            }
            _ => unreachable!(),
        }
    }
}

pub fn toggle_cursor(state: &mut AppState, ctx: &egui::Context) {
    if let Some(hook) = &mut state.gamescope_hook {
        hook.toggle_passthrough();
    } else if state.pc88.is_active {
        state.pc88.toggle_passthrough();
    } else if state.pc98.is_active {
        state.pc98.toggle_passthrough();
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::None));
    ctx.memory_mut(|m| {
        m.stop_text_input();
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
}

/// The Deck's gamepad remains owned by Steam/the game. This path forwards only
/// keyboard mappings and the hooked mouse, without stealing game buttons for UI.
pub fn forward_inputs(state: &mut AppState, ctx: &egui::Context, video: egui::Rect) {
    use crate::pc98::input as retro;
    let events = ctx.input(|i| i.events.clone());
    let focused = ctx.input(|i| i.focused);
    let typing = ctx.wants_keyboard_input();
    let deck = state.steam_deck.as_ref().unwrap();
    let in_game = deck.tab == Tab::Game;
    let pos = ctx.input(|i| i.pointer.latest_pos());
    let on_ocr = pos.is_some_and(|p| {
        state.ocr.boxes.iter().any(|b| {
            egui::Rect::from_center_size(
                video.min + egui::vec2(b.center_x * video.width(), b.center_y * video.height()),
                egui::vec2(b.width * video.width(), b.height * video.height()),
            )
            .contains(p)
        })
    });
    let in_popup = pos.is_some_and(|p| {
        state
            .dict
            .popup
            .as_ref()
            .and_then(|p| p.popup_rect)
            .is_some_and(|r| r.contains(p))
    });
    let pointer_ui =
        !in_game || state.whiteboard.open || in_popup || on_ocr || state.ui.show_quit_dialog;
    let deck = state.steam_deck.as_mut().unwrap();
    if ctx.input(|i| i.pointer.any_pressed()) {
        deck.ui_touch = pointer_ui || state.dict.popup.is_some();
    }
    let mouse_enabled = focused && !pointer_ui && !deck.ui_touch;
    let hooked = state
        .gamescope_hook
        .as_ref()
        .map(|h| h.passthrough_enabled)
        .unwrap_or(if state.pc88.is_active {
            state.pc88.passthrough_enabled
        } else {
            state.pc98.passthrough_enabled
        });
    let mouse_enabled = mouse_enabled && hooked;
    let touch = ctx.input(|i| {
        i.any_touches()
            || i.events
                .iter()
                .any(|e| matches!(e, egui::Event::Touch { .. }))
    });
    let relative = !touch
        && state
            .gamescope_hook
            .as_ref()
            .is_some_and(|h| h.is_cursor_hidden());
    let lock_cursor =
        mouse_enabled && !touch && (relative || state.pc88.is_active || state.pc98.is_active);
    ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(if lock_cursor {
        // Deck mode uses Xwayland, where Winit cannot lock the cursor. A
        // confined grab still supplies raw relative motion for hooked games.
        egui::CursorGrab::Confined
    } else {
        egui::CursorGrab::None
    }));

    if let Some(hook) = &mut state.gamescope_hook {
        if let Some(client) = &mut hook.input_client {
            client.dispatch();
            if !focused {
                for key in deck.forwarded_keys.drain() {
                    if let Some(code) = crate::gamescope::input::egui_key_to_evdev(key) {
                        client.send_key(code, false);
                    }
                }
                for code in [29, 42, 56, 125] {
                    client.send_key(code, false);
                }
            }
            for event in &events {
                if let egui::Event::Key {
                    key,
                    pressed,
                    modifiers,
                    ..
                } = event
                {
                    if (*pressed && (!focused || typing))
                        || (!pressed && !deck.forwarded_keys.contains(key))
                    {
                        continue;
                    }
                    if state.whiteboard.open
                        && modifiers.ctrl
                        && matches!(key, egui::Key::Z | egui::Key::Y)
                    {
                        continue;
                    }
                    if let Some(code) = crate::gamescope::input::egui_key_to_evdev(*key) {
                        for (code, down) in [
                            (29, modifiers.ctrl),
                            (42, modifiers.shift),
                            (56, modifiers.alt),
                            (125, modifiers.mac_cmd),
                        ] {
                            client.send_key(code, down);
                        }
                        client.send_key(code, *pressed);
                        if *pressed {
                            deck.forwarded_keys.insert(*key);
                        } else {
                            deck.forwarded_keys.remove(key);
                        }
                    }
                }
            }
            if deck.forwarded_keys.is_empty() {
                for code in [29, 42, 56, 125] {
                    client.send_key(code, false);
                }
            }
            if mouse_enabled {
                if relative {
                    let delta = ctx.input(|i| i.pointer.motion().unwrap_or(i.pointer.delta()));
                    client.send_pointer_motion(delta.x as f64, delta.y as f64);
                } else if let Some(pos) = pos.filter(|p| video.contains(*p)) {
                    let resolution = hook
                        .current_dmabuf_frame
                        .as_ref()
                        .map(|f| (f.width, f.height))
                        .unwrap_or(hook.initial_resolution);
                    client.send_pointer_motion_absolute(
                        ((pos.x - video.left()) / video.width()) as f64 * resolution.0 as f64,
                        ((pos.y - video.top()) / video.height()) as f64 * resolution.1 as f64,
                    );
                }
            }
            for (idx, code) in [
                crate::gamescope::input::BTN_LEFT,
                crate::gamescope::input::BTN_RIGHT,
                crate::gamescope::input::BTN_MIDDLE,
            ]
            .into_iter()
            .enumerate()
            {
                let down = mouse_enabled
                    && ctx.input(|i| {
                        i.pointer.button_down(
                            [
                                egui::PointerButton::Primary,
                                egui::PointerButton::Secondary,
                                egui::PointerButton::Middle,
                            ][idx],
                        )
                    });
                if down != deck.mouse_buttons[idx] {
                    client.send_button(code, down);
                    deck.mouse_buttons[idx] = down;
                }
            }
            if mouse_enabled {
                for event in &events {
                    if let egui::Event::MouseWheel { unit, delta, .. } = event {
                        let scale = match unit {
                            egui::MouseWheelUnit::Point => 1.0 / 50.0,
                            egui::MouseWheelUnit::Line => 1.0,
                            egui::MouseWheelUnit::Page => 5.0,
                        };
                        client.send_scroll(delta.x as f64 * scale, -delta.y as f64 * scale);
                    }
                }
            }
            client.dispatch();
        }
    } else {
        if deck.gamepad.is_none() && deck.gamepad_error.is_none() {
            match gamepad::Gamepad::new() {
                Ok(pad) => deck.gamepad = Some(pad),
                Err(error) => {
                    tracing::warn!("{error:#}");
                    deck.gamepad_error = Some(error.to_string());
                }
            }
        }
        let (buttons, axes) = if focused {
            deck.gamepad
                .as_mut()
                .map(|p| p.snapshot())
                .unwrap_or_default()
        } else {
            Default::default()
        };
        let (shared, numpad, sensitivity) = if state.pc88.is_active {
            (
                &state.pc88.input_state,
                state.pc88.config.numpad_arrow_keys,
                state.pc88.config.mouse_sensitivity,
            )
        } else {
            (
                &state.pc98.input_state,
                state.pc98.config.numpad_arrow_keys,
                state.pc98.config.mouse_sensitivity,
            )
        };
        if let Ok(mut input) = shared.lock() {
            if !focused {
                input.release_all();
                deck.forwarded_keys.clear();
            }
            input.joypad_buttons = buttons;
            input.analog_axes = axes;
            for event in &events {
                if let egui::Event::Key {
                    key,
                    physical_key,
                    pressed,
                    modifiers,
                    ..
                } = event
                {
                    if (*pressed && (!focused || typing))
                        || (!pressed && !deck.forwarded_keys.contains(key))
                    {
                        continue;
                    }
                    if state.whiteboard.open
                        && modifiers.ctrl
                        && matches!(key, egui::Key::Z | egui::Key::Y)
                    {
                        continue;
                    }
                    retro::apply_pc98_key_event(
                        &mut input.keys,
                        *key,
                        *physical_key,
                        *pressed,
                        *modifiers,
                        numpad,
                    );
                    input.keys[retro::RETROK_LCTRL as usize] = modifiers.ctrl;
                    input.keys[retro::RETROK_LSHIFT as usize] = modifiers.shift;
                    input.keys[retro::RETROK_LALT as usize] = modifiers.alt;
                    if *pressed {
                        deck.forwarded_keys.insert(*key);
                    } else {
                        deck.forwarded_keys.remove(key);
                    }
                }
            }
            if deck.forwarded_keys.is_empty() {
                for key in [
                    retro::RETROK_LCTRL,
                    retro::RETROK_LSHIFT,
                    retro::RETROK_LALT,
                ] {
                    input.keys[key as usize] = false;
                }
            }
            if mouse_enabled {
                let delta = ctx.input(|i| i.pointer.motion().unwrap_or(i.pointer.delta()))
                    * sensitivity.clamp(0.05, 5.0);
                input.mouse_dx = input.mouse_dx.saturating_add(delta.x as i32);
                input.mouse_dy = input.mouse_dy.saturating_add(delta.y as i32);
            }
            input.mouse_left = mouse_enabled && ctx.input(|i| i.pointer.primary_down());
            input.mouse_right = mouse_enabled && ctx.input(|i| i.pointer.secondary_down());
            input.mouse_middle =
                mouse_enabled && ctx.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
        }
    }
    if !ctx.input(|i| i.pointer.any_down()) {
        deck.ui_touch = false;
    }
    // Keep editable fields usable. Otherwise mapped game controls cannot move widget focus.
    if !typing {
        ctx.input_mut(|input| {
            input.events.retain(|event| match event {
                egui::Event::Key { key, modifiers, .. } => {
                    state.whiteboard.open
                        && modifiers.ctrl
                        && matches!(key, egui::Key::Z | egui::Key::Y)
                }
                egui::Event::Text(_) => false,
                _ => true,
            })
        });
    }
}
