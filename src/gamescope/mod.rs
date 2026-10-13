pub mod audio;
pub mod input;
pub mod video;

pub struct GamescopeHookState {
    pub is_active: bool,
    pub passthrough_enabled: bool,
    pub video_handle: Option<video::GamescopeVideoHandle>,
    pub audio_handle: Option<audio::GamescopeAudioHandle>,
    pub input_client: Option<input::GamescopeInputClient>,
    pub prev_modifiers: eframe::egui::Modifiers,
}

impl Default for GamescopeHookState {
    fn default() -> Self {
        Self {
            is_active: false,
            passthrough_enabled: true,
            video_handle: None,
            audio_handle: None,
            input_client: None,
            prev_modifiers: eframe::egui::Modifiers::default(),
        }
    }
}

impl GamescopeHookState {
    pub fn resolution(&self) -> (u32, u32) {
        if let Some(video) = &self.video_handle {
            (video.width, video.height)
        } else {
            (
                video::GAMESCOPE_WIDTH,
                video::GAMESCOPE_HEIGHT,
            )
        }
    }

    pub fn toggle_passthrough(&mut self) -> bool {
        self.passthrough_enabled = !self.passthrough_enabled;
        if !self.passthrough_enabled {
            self.release_all_inputs();
        }
        self.passthrough_enabled
    }

    pub fn release_all_inputs(&mut self) {
        if let Some(client) = &mut self.input_client {
            if self.prev_modifiers.shift {
                client.send_key(42, false);
            }
            if self.prev_modifiers.ctrl {
                client.send_key(29, false);
            }
            if self.prev_modifiers.alt {
                client.send_key(56, false);
            }
            if self.prev_modifiers.mac_cmd {
                client.send_key(125, false);
            }
            client.send_button(input::BTN_LEFT, false);
            client.send_button(input::BTN_RIGHT, false);
            client.send_button(input::BTN_MIDDLE, false);
        }
        self.prev_modifiers = eframe::egui::Modifiers::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gamescope_hook_state_defaults_and_toggle() {
        let mut state = GamescopeHookState::default();
        assert!(!state.is_active);
        assert!(state.passthrough_enabled);

        let toggled = state.toggle_passthrough();
        assert!(!toggled);
        assert!(!state.passthrough_enabled);

        let toggled_back = state.toggle_passthrough();
        assert!(toggled_back);
        assert!(state.passthrough_enabled);
    }
}


