pub mod audio;
pub mod egl;
pub mod input;
pub mod pipewire_dma;
pub mod video;
pub mod x11_cursor;

pub struct GamescopeHookState {
    pub is_active: bool,
    pub passthrough_enabled: bool,
    pub dma_handle: Option<pipewire_dma::GamescopeDmaBufHandle>,
    pub egl_importer: Option<std::sync::Arc<std::sync::Mutex<egl::EglDmaBufImporter>>>,
    pub current_dmabuf_frame: Option<std::sync::Arc<pipewire_dma::DmaBufFrame>>,
    pub video_handle: Option<video::GamescopeVideoHandle>,
    pub audio_handle: Option<audio::GamescopeAudioHandle>,
    pub input_client: Option<input::GamescopeInputClient>,
    pub x11_cursor_monitor: Option<x11_cursor::GamescopeX11CursorMonitor>,
    pub cursor_texture: Option<(u64, eframe::egui::TextureHandle)>,
    pub prev_modifiers: eframe::egui::Modifiers,
    pub initial_resolution: (u32, u32),
    pub last_cursor_retry: Option<std::time::Instant>,
}

impl Default for GamescopeHookState {
    fn default() -> Self {
        Self {
            is_active: false,
            passthrough_enabled: true,
            dma_handle: None,
            egl_importer: None,
            current_dmabuf_frame: None,
            video_handle: None,
            audio_handle: None,
            input_client: None,
            x11_cursor_monitor: None,
            cursor_texture: None,
            prev_modifiers: eframe::egui::Modifiers::default(),
            initial_resolution: (video::GAMESCOPE_WIDTH, video::GAMESCOPE_HEIGHT),
            last_cursor_retry: None,
        }
    }
}

impl GamescopeHookState {
    pub fn poll_late_connections(&mut self) {
        if !self.is_active || (self.x11_cursor_monitor.is_some() && self.input_client.is_some()) {
            return;
        }
        let now = std::time::Instant::now();
        if let Some(last) = self.last_cursor_retry {
            if now.duration_since(last) < std::time::Duration::from_secs(1) {
                return;
            }
        }
        self.last_cursor_retry = Some(now);

        if self.x11_cursor_monitor.is_none() {
            if let Some(disp) = x11_cursor::find_gamescope_x_display() {
                if let Some(monitor) = x11_cursor::GamescopeX11CursorMonitor::start(&disp) {
                    tracing::info!("Found Gamescope X11 display: {disp}, started cursor monitor");
                    self.x11_cursor_monitor = Some(monitor);
                }
            }
        }
        if self.input_client.is_none() {
            if let Ok(client) = input::GamescopeInputClient::connect() {
                self.input_client = Some(client);
            }
        }
    }

    pub fn is_cursor_hidden(&self) -> bool {
        self.x11_cursor_monitor.as_ref().map_or(false, |m| m.is_cursor_hidden())
    }

    pub fn cursor_image(&self) -> Option<std::sync::Arc<x11_cursor::GameCursorImage>> {
        self.x11_cursor_monitor.as_ref().and_then(|m| m.cursor_image())
    }

    pub fn resolution(&self) -> (u32, u32) {
        if let Some(frame) = &self.current_dmabuf_frame {
            (frame.width, frame.height)
        } else if let Some(video) = &self.video_handle {
            (video.width, video.height)
        } else {
            self.initial_resolution
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


