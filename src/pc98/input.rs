use super::Pc98State;
use eframe::egui;

pub const RETRO_DEVICE_NONE: u32 = 0;
pub const RETRO_DEVICE_JOYPAD: u32 = 1;
pub const RETRO_DEVICE_MOUSE: u32 = 2;
pub const RETRO_DEVICE_KEYBOARD: u32 = 3;
pub const RETRO_DEVICE_ANALOG: u32 = 5;

pub const RETRO_DEVICE_ID_MOUSE_X: u32 = 0;
pub const RETRO_DEVICE_ID_MOUSE_Y: u32 = 1;
pub const RETRO_DEVICE_ID_MOUSE_LEFT: u32 = 2;
pub const RETRO_DEVICE_ID_MOUSE_RIGHT: u32 = 3;
pub const RETRO_DEVICE_ID_MOUSE_WHEELUP: u32 = 4;
pub const RETRO_DEVICE_ID_MOUSE_WHEELDOWN: u32 = 5;
pub const RETRO_DEVICE_ID_MOUSE_MIDDLE: u32 = 6;

pub const RETRO_DEVICE_ID_JOYPAD_B: u32 = 0;
pub const RETRO_DEVICE_ID_JOYPAD_Y: u32 = 1;
pub const RETRO_DEVICE_ID_JOYPAD_SELECT: u32 = 2;
pub const RETRO_DEVICE_ID_JOYPAD_START: u32 = 3;
pub const RETRO_DEVICE_ID_JOYPAD_UP: u32 = 4;
pub const RETRO_DEVICE_ID_JOYPAD_DOWN: u32 = 5;
pub const RETRO_DEVICE_ID_JOYPAD_LEFT: u32 = 6;
pub const RETRO_DEVICE_ID_JOYPAD_RIGHT: u32 = 7;
pub const RETRO_DEVICE_ID_JOYPAD_A: u32 = 8;
pub const RETRO_DEVICE_ID_JOYPAD_X: u32 = 9;

pub const RETROK_BACKSPACE: u32 = 8;
pub const RETROK_TAB: u32 = 9;
pub const RETROK_RETURN: u32 = 13;
pub const RETROK_PAUSE: u32 = 19;
pub const RETROK_ESCAPE: u32 = 27;
pub const RETROK_SPACE: u32 = 32;
pub const RETROK_QUOTE: u32 = 39;
pub const RETROK_PLUS: u32 = 43;
pub const RETROK_COMMA: u32 = 44;
pub const RETROK_MINUS: u32 = 45;
pub const RETROK_PERIOD: u32 = 46;
pub const RETROK_SLASH: u32 = 47;
pub const RETROK_0: u32 = 48;
pub const RETROK_1: u32 = 49;
pub const RETROK_2: u32 = 50;
pub const RETROK_3: u32 = 51;
pub const RETROK_4: u32 = 52;
pub const RETROK_5: u32 = 53;
pub const RETROK_6: u32 = 54;
pub const RETROK_7: u32 = 55;
pub const RETROK_8: u32 = 56;
pub const RETROK_9: u32 = 57;
pub const RETROK_COLON: u32 = 58;
pub const RETROK_SEMICOLON: u32 = 59;
pub const RETROK_EQUALS: u32 = 61;
pub const RETROK_LEFTBRACKET: u32 = 91;
pub const RETROK_BACKSLASH: u32 = 92;
pub const RETROK_RIGHTBRACKET: u32 = 93;
pub const RETROK_BACKQUOTE: u32 = 96;
pub const RETROK_A: u32 = 97;
pub const RETROK_B: u32 = 98;
pub const RETROK_C: u32 = 99;
pub const RETROK_D: u32 = 100;
pub const RETROK_E: u32 = 101;
pub const RETROK_F: u32 = 102;
pub const RETROK_G: u32 = 103;
pub const RETROK_H: u32 = 104;
pub const RETROK_I: u32 = 105;
pub const RETROK_J: u32 = 106;
pub const RETROK_K: u32 = 107;
pub const RETROK_L: u32 = 108;
pub const RETROK_M: u32 = 109;
pub const RETROK_N: u32 = 110;
pub const RETROK_O: u32 = 111;
pub const RETROK_P: u32 = 112;
pub const RETROK_Q: u32 = 113;
pub const RETROK_R: u32 = 114;
pub const RETROK_S: u32 = 115;
pub const RETROK_T: u32 = 116;
pub const RETROK_U: u32 = 117;
pub const RETROK_V: u32 = 118;
pub const RETROK_W: u32 = 119;
pub const RETROK_X: u32 = 120;
pub const RETROK_Y: u32 = 121;
pub const RETROK_Z: u32 = 122;
pub const RETROK_DELETE: u32 = 127;

pub const RETROK_KP0: u32 = 256;
pub const RETROK_KP1: u32 = 257;
pub const RETROK_KP2: u32 = 258;
pub const RETROK_KP3: u32 = 259;
pub const RETROK_KP4: u32 = 260;
pub const RETROK_KP5: u32 = 261;
pub const RETROK_KP6: u32 = 262;
pub const RETROK_KP7: u32 = 263;
pub const RETROK_KP8: u32 = 264;
pub const RETROK_KP9: u32 = 265;
pub const RETROK_KP_PERIOD: u32 = 266;
pub const RETROK_KP_DIVIDE: u32 = 267;
pub const RETROK_KP_MULTIPLY: u32 = 268;
pub const RETROK_KP_MINUS: u32 = 269;
pub const RETROK_KP_PLUS: u32 = 270;
pub const RETROK_KP_ENTER: u32 = 271;
pub const RETROK_KP_EQUALS: u32 = 272;

pub const RETROK_UP: u32 = 273;
pub const RETROK_DOWN: u32 = 274;
pub const RETROK_RIGHT: u32 = 275;
pub const RETROK_LEFT: u32 = 276;
pub const RETROK_INSERT: u32 = 277;
pub const RETROK_HOME: u32 = 278;
pub const RETROK_END: u32 = 279;
pub const RETROK_PAGEUP: u32 = 280;
pub const RETROK_PAGEDOWN: u32 = 281;
pub const RETROK_F1: u32 = 282;
pub const RETROK_F2: u32 = 283;
pub const RETROK_F3: u32 = 284;
pub const RETROK_F4: u32 = 285;
pub const RETROK_F5: u32 = 286;
pub const RETROK_F6: u32 = 287;
pub const RETROK_F7: u32 = 288;
pub const RETROK_F8: u32 = 289;
pub const RETROK_F9: u32 = 290;
pub const RETROK_F10: u32 = 291;
pub const RETROK_F11: u32 = 292;
pub const RETROK_F12: u32 = 293;

pub const RETROK_RSHIFT: u32 = 303;
pub const RETROK_LSHIFT: u32 = 304;
pub const RETROK_RCTRL: u32 = 305;
pub const RETROK_LCTRL: u32 = 306;
pub const RETROK_RALT: u32 = 307;
pub const RETROK_LALT: u32 = 308;
pub const RETROK_LSUPER: u32 = 311;

pub fn numpad_digit_to_retrok(key: egui::Key) -> Option<u32> {
    use egui::Key;
    match key {
        Key::Num0 => Some(RETROK_KP0),
        Key::Num1 => Some(RETROK_KP1),
        Key::Num2 => Some(RETROK_KP2),
        Key::Num3 => Some(RETROK_KP3),
        Key::Num4 => Some(RETROK_KP4),
        Key::Num5 => Some(RETROK_KP5),
        Key::Num6 => Some(RETROK_KP6),
        Key::Num7 => Some(RETROK_KP7),
        Key::Num8 => Some(RETROK_KP8),
        Key::Num9 => Some(RETROK_KP9),
        _ => None,
    }
}

pub fn top_row_digit_to_retrok(key: egui::Key) -> Option<u32> {
    use egui::Key;
    match key {
        Key::Num0 => Some(RETROK_0),
        Key::Num1 => Some(RETROK_1),
        Key::Num2 => Some(RETROK_2),
        Key::Num3 => Some(RETROK_3),
        Key::Num4 => Some(RETROK_4),
        Key::Num5 => Some(RETROK_5),
        Key::Num6 => Some(RETROK_6),
        Key::Num7 => Some(RETROK_7),
        Key::Num8 => Some(RETROK_8),
        Key::Num9 => Some(RETROK_9),
        _ => None,
    }
}

pub fn egui_key_to_retrok(key: egui::Key) -> Option<u32> {
    use egui::Key;
    match key {
        Key::ArrowDown => Some(RETROK_DOWN),
        Key::ArrowLeft => Some(RETROK_LEFT),
        Key::ArrowRight => Some(RETROK_RIGHT),
        Key::ArrowUp => Some(RETROK_UP),
        Key::Escape => Some(RETROK_ESCAPE),
        Key::Tab => Some(RETROK_TAB),
        Key::Backspace => Some(RETROK_BACKSPACE),
        Key::Enter => Some(RETROK_RETURN),
        Key::Space => Some(RETROK_SPACE),
        Key::Insert => Some(RETROK_INSERT),
        Key::Delete => Some(RETROK_DELETE),
        Key::Home => Some(RETROK_HOME),
        Key::End => Some(RETROK_END),
        Key::PageUp => Some(RETROK_PAGEUP),
        Key::PageDown => Some(RETROK_PAGEDOWN),
        Key::Colon => Some(RETROK_QUOTE),
        Key::Comma => Some(RETROK_COMMA),
        Key::Backslash => Some(RETROK_BACKSLASH),
        Key::Slash => Some(RETROK_SLASH),
        Key::Minus => Some(RETROK_MINUS),
        Key::Period => Some(RETROK_PERIOD),
        Key::Plus => Some(RETROK_KP_PLUS),
        Key::Equals => Some(RETROK_EQUALS),
        Key::Semicolon => Some(RETROK_SEMICOLON),
        Key::OpenBracket => Some(RETROK_LEFTBRACKET),
        Key::CloseBracket => Some(RETROK_RIGHTBRACKET),
        Key::Backtick => Some(RETROK_BACKQUOTE),
        Key::Num0 => Some(RETROK_KP0),
        Key::Num1 => Some(RETROK_KP1),
        Key::Num2 => Some(RETROK_KP2),
        Key::Num3 => Some(RETROK_KP3),
        Key::Num4 => Some(RETROK_KP4),
        Key::Num5 => Some(RETROK_KP5),
        Key::Num6 => Some(RETROK_KP6),
        Key::Num7 => Some(RETROK_KP7),
        Key::Num8 => Some(RETROK_KP8),
        Key::Num9 => Some(RETROK_KP9),
        Key::A => Some(RETROK_A),
        Key::B => Some(RETROK_B),
        Key::C => Some(RETROK_C),
        Key::D => Some(RETROK_D),
        Key::E => Some(RETROK_E),
        Key::F => Some(RETROK_F),
        Key::G => Some(RETROK_G),
        Key::H => Some(RETROK_H),
        Key::I => Some(RETROK_I),
        Key::J => Some(RETROK_J),
        Key::K => Some(RETROK_K),
        Key::L => Some(RETROK_L),
        Key::M => Some(RETROK_M),
        Key::N => Some(RETROK_N),
        Key::O => Some(RETROK_O),
        Key::P => Some(RETROK_P),
        Key::Q => Some(RETROK_Q),
        Key::R => Some(RETROK_R),
        Key::S => Some(RETROK_S),
        Key::T => Some(RETROK_T),
        Key::U => Some(RETROK_U),
        Key::V => Some(RETROK_V),
        Key::W => Some(RETROK_W),
        Key::X => Some(RETROK_X),
        Key::Y => Some(RETROK_Y),
        Key::Z => Some(RETROK_Z),
        Key::F1 => Some(RETROK_F1),
        Key::F2 => Some(RETROK_F2),
        Key::F3 => Some(RETROK_F3),
        Key::F4 => Some(RETROK_F4),
        Key::F5 => Some(RETROK_F5),
        Key::F6 => Some(RETROK_F6),
        Key::F7 => Some(RETROK_F7),
        Key::F8 => Some(RETROK_F8),
        Key::F9 => Some(RETROK_F9),
        Key::F10 => Some(RETROK_F10),
        Key::F11 => Some(RETROK_F11),
        Key::F12 => Some(RETROK_F12),
        _ => None,
    }
}

/// Applies an `egui::Event::Key` to the PC-98 key state array, translating:
/// - Physical Numpad keys (even when host NumLock is OFF, where `physical_key` is `Num0..9`
///   and `key` is `Arrow*`/`Home`/`End`/`PageUp`/`PageDown`/`Insert`) to `RETROK_KP0..9`.
/// - Arrow keys (`ArrowUp`/`Down`/`Left`/`Right`) to PC-98 Numpad `8`/`2`/`4`/`6`
///   (`RETROK_KP8`/`KP2`/`KP4`/`KP6`) when `numpad_arrow_keys` is enabled, or dedicated
///   PC-98 cursor keys (`RETROK_UP`/`DOWN`/`LEFT`/`RIGHT`) when disabled.
/// - Unshifted `Num0..9` to `RETROK_KP0..9` (so 8-way Numpad movement works in games while
///   still producing ASCII `'0'..'9'` in DOS), and shifted `Num0..9` to `RETROK_0..9` for
///   top-row punctuation (`! " # $ % & ' ( )`).
pub fn apply_pc98_key_event(
    keys: &mut [bool; 512],
    key: egui::Key,
    physical_key: Option<egui::Key>,
    pressed: bool,
    modifiers: egui::Modifiers,
    numpad_arrow_keys: bool,
) {
    use egui::Key;

    // 1. Physical Numpad with host NumLock OFF (`physical_key == Some(Key::Num0..9)` while
    // logical `key` is a navigation key like `ArrowUp`, `Home`, `Insert`, etc.)
    if let Some(phys) = physical_key {
        if phys != key {
            if let Some(kp_code) = numpad_digit_to_retrok(phys) {
                keys[kp_code as usize] = pressed;
                return;
            }
        }
    }

    // 2. Dedicated Arrow keys
    match key {
        Key::ArrowUp => {
            if pressed {
                if numpad_arrow_keys {
                    keys[RETROK_KP8 as usize] = true;
                } else {
                    keys[RETROK_UP as usize] = true;
                }
            } else {
                keys[RETROK_KP8 as usize] = false;
                keys[RETROK_UP as usize] = false;
            }
            return;
        }
        Key::ArrowDown => {
            if pressed {
                if numpad_arrow_keys {
                    keys[RETROK_KP2 as usize] = true;
                } else {
                    keys[RETROK_DOWN as usize] = true;
                }
            } else {
                keys[RETROK_KP2 as usize] = false;
                keys[RETROK_DOWN as usize] = false;
            }
            return;
        }
        Key::ArrowLeft => {
            if pressed {
                if numpad_arrow_keys {
                    keys[RETROK_KP4 as usize] = true;
                } else {
                    keys[RETROK_LEFT as usize] = true;
                }
            } else {
                keys[RETROK_KP4 as usize] = false;
                keys[RETROK_LEFT as usize] = false;
            }
            return;
        }
        Key::ArrowRight => {
            if pressed {
                if numpad_arrow_keys {
                    keys[RETROK_KP6 as usize] = true;
                } else {
                    keys[RETROK_RIGHT as usize] = true;
                }
            } else {
                keys[RETROK_KP6 as usize] = false;
                keys[RETROK_RIGHT as usize] = false;
            }
            return;
        }
        _ => {}
    }

    // 3. Number keys (`Num0..9`): use top-row `RETROK_0..9` when Shift is held (for `!`..`)`),
    // and PC-98 Tenkey `RETROK_KP0..9` when unshifted.
    if let (Some(kp_code), Some(top_code)) =
        (numpad_digit_to_retrok(key), top_row_digit_to_retrok(key))
    {
        if pressed {
            if modifiers.shift {
                keys[top_code as usize] = true;
            } else {
                keys[kp_code as usize] = true;
            }
        } else {
            keys[kp_code as usize] = false;
            keys[top_code as usize] = false;
        }
        return;
    }

    // 4. All other keys (with fallback to `physical_key` if shifted symbol changed logical `key`)
    if let Some(code) = egui_key_to_retrok(key).or_else(|| physical_key.and_then(egui_key_to_retrok))
    {
        if (code as usize) < keys.len() {
            keys[code as usize] = pressed;
        }
    }
}

/// Forwards keyboard, modifier, and relative mouse inputs to the active PC-98 emulator instance.
pub fn forward_pc98_inputs(
    pc98: &mut Pc98State,
    ctx: &egui::Context,
    _video_rect: egui::Rect,
    _target_res: (u32, u32),
) {
    if !pc98.is_active || !pc98.passthrough_enabled {
        return;
    }

    let Ok(mut state) = pc98.input_state.lock() else {
        return;
    };

    // 1. Relative mouse motion (under CursorGrab::Locked, pointer.motion() provides raw deltas
    // while pointer position is locked inside the window)
    let raw_motion = ctx.input(|i| {
        if let Some(m) = i.pointer.motion() {
            if m.x != 0.0 || m.y != 0.0 {
                return m;
            }
        }
        i.pointer.delta()
    });

    if raw_motion.x != 0.0 || raw_motion.y != 0.0 {
        let sens = pc98.config.mouse_sensitivity.clamp(0.05, 5.0);
        state.mouse_rem_x += raw_motion.x * sens;
        state.mouse_rem_y += raw_motion.y * sens;
        let dx = state.mouse_rem_x.trunc() as i32;
        let dy = state.mouse_rem_y.trunc() as i32;
        state.mouse_rem_x -= dx as f32;
        state.mouse_rem_y -= dy as f32;
        state.mouse_dx = state.mouse_dx.saturating_add(dx);
        state.mouse_dy = state.mouse_dy.saturating_add(dy);
    }

    // 2. Modifier keys
    let mods = ctx.input(|i| i.modifiers);
    state.keys[RETROK_LSHIFT as usize] = mods.shift;
    state.keys[RETROK_LCTRL as usize] = mods.ctrl;
    state.keys[RETROK_LALT as usize] = mods.alt;
    state.keys[RETROK_LSUPER as usize] = mods.mac_cmd;
    pc98.prev_modifiers = mods;

    // 3. Discrete keyboard & mouse button events
    let numpad_arrow_keys = pc98.config.numpad_arrow_keys;
    let events = ctx.input(|i| i.events.clone());
    for event in events {
        match event {
            egui::Event::PointerButton {
                button,
                pressed,
                ..
            } => {
                match button {
                    egui::PointerButton::Primary => state.mouse_left = pressed,
                    egui::PointerButton::Secondary => state.mouse_right = pressed,
                    egui::PointerButton::Middle => state.mouse_middle = pressed,
                    _ => {}
                }
            }
            egui::Event::Key {
                key,
                physical_key,
                pressed,
                modifiers,
                ..
            } => {
                // Shift+Tab is reserved for toggling Michadame passthrough
                if key == egui::Key::Tab && modifiers.shift {
                    continue;
                }
                apply_pc98_key_event(
                    &mut state.keys,
                    key,
                    physical_key,
                    pressed,
                    modifiers,
                    numpad_arrow_keys,
                );
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_egui_key_to_retrok_mapping() {
        assert_eq!(egui_key_to_retrok(egui::Key::Enter), Some(RETROK_RETURN));
        assert_eq!(egui_key_to_retrok(egui::Key::Escape), Some(RETROK_ESCAPE));
        assert_eq!(egui_key_to_retrok(egui::Key::F12), Some(RETROK_F12));
        assert_eq!(egui_key_to_retrok(egui::Key::Z), Some(RETROK_Z));
        assert_eq!(egui_key_to_retrok(egui::Key::X), Some(RETROK_X));
        assert_eq!(egui_key_to_retrok(egui::Key::ArrowUp), Some(RETROK_UP));
        assert_eq!(egui_key_to_retrok(egui::Key::Num8), Some(RETROK_KP8));
    }

    #[test]
    fn test_arrow_keys_map_to_numpad_by_default() {
        let mut keys = [false; 512];
        let mods = egui::Modifiers::default();

        apply_pc98_key_event(
            &mut keys,
            egui::Key::ArrowUp,
            Some(egui::Key::ArrowUp),
            true,
            mods,
            true,
        );
        assert!(keys[RETROK_KP8 as usize]);
        assert!(!keys[RETROK_UP as usize]);

        apply_pc98_key_event(
            &mut keys,
            egui::Key::ArrowDown,
            Some(egui::Key::ArrowDown),
            true,
            mods,
            true,
        );
        assert!(keys[RETROK_KP2 as usize]);

        apply_pc98_key_event(
            &mut keys,
            egui::Key::ArrowLeft,
            Some(egui::Key::ArrowLeft),
            true,
            mods,
            true,
        );
        assert!(keys[RETROK_KP4 as usize]);

        apply_pc98_key_event(
            &mut keys,
            egui::Key::ArrowRight,
            Some(egui::Key::ArrowRight),
            true,
            mods,
            true,
        );
        assert!(keys[RETROK_KP6 as usize]);

        // Releasing clears both KP and cursor arrow slots
        apply_pc98_key_event(
            &mut keys,
            egui::Key::ArrowUp,
            Some(egui::Key::ArrowUp),
            false,
            mods,
            false,
        );
        assert!(!keys[RETROK_KP8 as usize]);
        assert!(!keys[RETROK_UP as usize]);
    }

    #[test]
    fn test_physical_numpad_with_numlock_off_maps_to_kp() {
        let mut keys = [false; 512];
        let mods = egui::Modifiers::default();

        // Even when numpad_arrow_keys is false, physical Numpad 7 (Home when NumLock off)
        // and Numpad 8 (ArrowUp when NumLock off) map to RETROK_KP7 and RETROK_KP8.
        apply_pc98_key_event(
            &mut keys,
            egui::Key::Home,
            Some(egui::Key::Num7),
            true,
            mods,
            false,
        );
        assert!(keys[RETROK_KP7 as usize]);
        assert!(!keys[RETROK_HOME as usize]);

        apply_pc98_key_event(
            &mut keys,
            egui::Key::ArrowUp,
            Some(egui::Key::Num8),
            true,
            mods,
            false,
        );
        assert!(keys[RETROK_KP8 as usize]);
        assert!(!keys[RETROK_UP as usize]);
    }
}
