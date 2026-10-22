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
        Key::Colon => Some(RETROK_COLON),
        Key::Comma => Some(RETROK_COMMA),
        Key::Backslash => Some(RETROK_BACKSLASH),
        Key::Slash => Some(RETROK_SLASH),
        Key::Minus => Some(RETROK_MINUS),
        Key::Period => Some(RETROK_PERIOD),
        Key::Plus => Some(RETROK_PLUS),
        Key::Equals => Some(RETROK_EQUALS),
        Key::Semicolon => Some(RETROK_SEMICOLON),
        Key::OpenBracket => Some(RETROK_LEFTBRACKET),
        Key::CloseBracket => Some(RETROK_RIGHTBRACKET),
        Key::Backtick => Some(RETROK_BACKQUOTE),
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
                pressed,
                modifiers,
                ..
            } => {
                // Shift+Tab is reserved for toggling Michadame passthrough
                if key == egui::Key::Tab && modifiers.shift {
                    continue;
                }
                if let Some(code) = egui_key_to_retrok(key) {
                    if (code as usize) < state.keys.len() {
                        state.keys[code as usize] = pressed;
                    }
                }
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
    }
}
