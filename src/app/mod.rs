pub mod init;
pub mod models;
pub mod stream;

use crate::devices::filter_type::CrtFilter;
use crate::{config, devices, ui, video};
use eframe::egui;
pub use init::init_app_state;
pub use models::*;
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicU64, AtomicU8, Ordering},
    Arc,
};
use std::time::Instant;

impl Default for AppState {
    fn default() -> Self {
        let replay = crate::replay::Replay::default();
        let mut twitch = crate::twitch::TwitchState::default();
        twitch.audio_tap = Some(replay.audio.clone());
        Self {
            steam_deck: None,
            replay,
            hardware: HardwareState {
                video_queue_drops: Arc::new(AtomicU64::new(0)),
                audio_peak_amplitude: Arc::new(AtomicU64::new(0)),
                audio_latency_ms: Arc::new(AtomicU64::new(0)),
                audio_buffer_size: 1024,
                audio_sample_rate: 48000,
                audio_sample_format: "S16LE".to_string(),
                master_volume: 1.0,
                live_audio_peak: 0.0,
                live_mic_peak: 0.0,
                video_devices: Vec::new(),
                usb_devices: Vec::new(),
                selected_usb_device: None,
                selected_video_device: String::new(),
                audio_sources: Vec::new(),
                selected_audio_source_name: None,
                active_audio_stream: None,
                supported_formats: Vec::new(),
                selected_format_index: 0,
                selected_resolution: (0, 0),
                selected_framerate: 0,
            },
            ui: UiState {
                debug_open: false,
                is_fullscreen: false,
                reset_usb_on_startup: false,
                show_first_run_dialog: false,
                show_quit_dialog: false,
                show_stop_stream_dialog: false,
                video_window_open: false,
                control_window_open: true,
                studio_window_open: false,
                dismissed_config_error: false,
                active_settings_tab: crate::app::models::SettingsTab::default(),
                config_save_pending: false,
                in_egui_frame: false,
                any_pointer_down: false,
                scale: Default::default(),
            },
            crt: CrtSettings {
                hard_scan: -8.0,
                warp_x: 0.031,
                warp_y: 0.041,
                shadow_mask: 3.0,
                brightboost: 1.0,
                hard_bloom_pix: -1.5,
                hard_bloom_scan: -2.0,
                bloom_amount: 0.15,
                shape: 2.0,
                hard_pix: -3.0,
            },
            halo: crate::app::models::HaloSettings::default(),
            halo_defaults: crate::app::models::HaloSettings::default(),
            halo_presets: crate::app::models::HaloSettings::builtin_presets(),
            active_halo_preset: crate::app::models::HALO_DEFAULT_PRESET.to_string(),
            new_halo_preset_name: String::new(),
            beam: crate::app::models::BeamSettings::default(),
            beam_defaults: crate::app::models::BeamSettings::default(),
            beam_presets: crate::app::models::BeamMaskPreset::builtin_presets(),
            active_beam_preset: crate::app::models::BEAM_DEFAULT_PRESET.to_string(),
            new_beam_preset_name: String::new(),
            beam_lockstep: crate::app::models::BeamLockstepState::default(),
            cathode_interference: crate::app::models::CathodeInterferenceSettings::default(),
            cathode_interference_defaults: crate::app::models::CathodeInterferenceSettings::default(
            ),
            audio_reactive: crate::app::models::AudioReactiveConfig::default(),
            audio_reactive_runtime: crate::app::models::AudioReactiveRuntime::default(),
            selected_crt_filter: CrtFilter::Lottes,
            video: VideoSettings {
                pixelate_filter_enabled: false,
                use_magenta_background: false,
                retro_pc_frame: false,
                retro_pc_frame_dark_mode: false,
                retro_pc_ambient_glow: 0.55,
                retro_software_mouse: false,
                lights_off_night_mode: false,
                realistic_light_switch: false,
                light_switch_flicker_start: None,
                night_mode_glow_intensity: 0.0,
                crt_glass_enabled: false,
                crt_glass_intensity: 0.25,
                crt_glass_glossiness: 0.50,
                crt_glass_ceiling_light_enabled: true,
                crt_glass_photographer_enabled: false,
                crt_glass_photographer_intensity: 0.50,
                crt_glass_flash_enabled: false,
                crt_glass_flash_intensity: 0.70,
                horizontal_stretch: 1.0,
                median_filter_enabled: false,
                median_mix: 1.0,
                deinterlace_filter_enabled: false,
                deinterlace_mode: 0,
                deinterlace_blend: 0.5,
                deinterlace_motion_threshold: 0.08,
                deinterlace_line_spacing: 1.0,
                deinterlace_spatial_mix: 0.75,
                vibrance: 1.0,
                brightness: 1.0,
                gamma: 1.0,
                overscan_x: 0.0,
                overscan_y: 0.0,
                underscan_x: 0.0,
                underscan_y: 0.0,
                border_crop_left: 0.0,
                border_crop_right: 0.0,
                border_crop_top: 0.0,
                border_crop_bottom: 0.0,
                fft_filter_enabled: false,
                fft_mask_window_open: false,
            },
            toasts: egui_toast::Toasts::new()
                .anchor(egui::Align2::LEFT_TOP, (10.0, 10.0))
                .direction(egui::Direction::TopDown),
            video_thread: None,
            video_stop_requested: None,
            video_texture: None,
            frame_receiver: None,
            video_status_receiver: None,
            video_player: None,
            video_player_event_receiver: None,
            video_file_dialog_receiver: None,
            pending_audio_stream: None,
            device_scan_receiver: None,
            logo_texture: None,
            gui_fps: 0.0,
            video_fps: 0.0,
            last_fps_check: Instant::now(),
            frames_since_last_check: 0,
            last_video_fps_check: Instant::now(),
            video_frames_since_last_check: 0,
            last_window_title: String::new(),
            next_gui_frame_time: None,
            crt_filter: Arc::new(AtomicU8::new(CrtFilter::Off as u8)),
            scaler_filter: Arc::new(AtomicU8::new(video::types::ScalerFilter::Bicubic as u8)),
            color_range: Arc::new(AtomicU8::new(video::types::ColorRange::Full as u8)),
            crt_renderer: None,
            fullscreen_toggle_frame_count: None,
            latest_frame: None,
            profiles: {
                let mut p = BTreeMap::new();
                p.insert("Default".to_string(), config::Profile::default());
                p
            },
            active_profile: "Default".to_string(),
            new_profile_name: String::new(),
            video_profile: config::Profile::default(),
            gamescope_profile: config::Profile::default(),
            pc98_profile: config::models::default_pc98_profile(),
            pc88_profile: config::models::default_pc88_profile(),
            previous_normal_profile: None,
            previous_gamescope_normal_profile: None,
            previous_pc98_normal_profile: None,
            previous_pc88_normal_profile: None,
            previous_capture_resolution: (0, 0),
            previous_capture_framerate: 0,
            gamescope_hook: None,
            gamescope_launcher_script: String::new(),
            steam_games: Vec::new(),
            selected_steam_game: None,
            gamescope_script_dialog_receiver: None,
            retro_pc_system: crate::pc88::RetroPcSystem::default(),
            pc98: crate::pc98::Pc98State::default(),
            pc88: crate::pc88::Pc88State::default(),
            fft_filter: None,
            fft_mask_data: Vec::new(),
            fft_mask_dirty: false,
            fft_mask_resolution: (0, 0),
            fft_brush_radius: 8.0,
            fft_mask_threshold: 0.0,
            fft_black_threshold: 0.0,
            fft_mask_save_name: String::new(),
            fft_available_masks: Vec::new(),
            ocr: crate::ocr::OcrState::default(),
            dict: crate::dict::DictState::default(),
            bank: crate::bank::BankState::default(),
            twitch,
            mic: crate::app::models::MicState::default(),
            webcam: crate::app::models::WebcamState::default(),
            whiteboard: crate::ui::whiteboard::WhiteboardState::default(),
            screenshot: crate::screenshot::ScreenshotState::default(),
            config_load_error: None,
            config_quarantine_path: None,
        }
    }
}

impl AppState {
    #[cfg(test)]
    pub fn handle_device_scan_result(&mut self, result: devices::DeviceScanResult) -> bool {
        self.handle_device_scan_result_with_ctx(result, None)
    }

    pub fn handle_device_scan_result_with_ctx(
        &mut self,
        result: devices::DeviceScanResult,
        ctx: Option<&egui::Context>,
    ) -> bool {
        let scan_successful = match result {
            Ok((video_devices, audio_sources, usb_devices)) => {
                self.hardware.video_devices = video_devices;
                self.hardware.selected_video_device = self
                    .hardware
                    .video_devices
                    .first()
                    .cloned()
                    .unwrap_or_default();
                self.hardware.audio_sources = audio_sources;
                self.hardware.usb_devices = usb_devices;

                if !cfg!(test) {
                    if let Ok(cfg) = confy::load::<config::MichadameConfig>("michadame", None) {
                        config::apply_hardware_devices_from_config(self, &cfg);
                    }
                }
                self.sync_mic_stream();
                self.sync_webcam_stream(ctx);
                self.info("Devices loaded successfully.");
                true
            }
            Err(e) => {
                self.error(format!("Error: {}", e));
                false
            }
        };
        self.device_scan_receiver = None;
        scan_successful
    }

    pub fn clear_ocr(&mut self) {
        self.ocr.clear();
        self.dict.popup = None;
    }

    pub fn ensure_halo_presets(&mut self) {
        for (name, preset) in crate::app::models::HaloSettings::builtin_presets() {
            self.halo_presets.entry(name).or_insert(preset);
        }
        for preset in self.halo_presets.values_mut() {
            *preset = preset.sanitized();
        }
        if self.active_halo_preset.trim().is_empty()
            || !self.halo_presets.contains_key(&self.active_halo_preset)
        {
            self.active_halo_preset = crate::app::models::HALO_DEFAULT_PRESET.to_string();
        }
    }

    pub fn apply_halo_preset(&mut self, name: &str) -> bool {
        self.ensure_halo_presets();
        if let Some(preset) = self.halo_presets.get(name).cloned() {
            let clean = preset.sanitized();
            self.active_halo_preset = name.to_string();
            self.halo = clean.clone();
            self.halo_defaults = clean.clone();
            self.video.vibrance = clean.vibrance;
            true
        } else {
            false
        }
    }

    pub fn ensure_beam_presets(&mut self) {
        for (name, preset) in crate::app::models::BeamMaskPreset::builtin_presets() {
            self.beam_presets.entry(name).or_insert(preset);
        }
        for preset in self.beam_presets.values_mut() {
            *preset = preset.sanitized();
        }
        if self.active_beam_preset.trim().is_empty()
            || !self.beam_presets.contains_key(&self.active_beam_preset)
        {
            self.active_beam_preset = crate::app::models::BEAM_DEFAULT_PRESET.to_string();
        }
    }

    pub fn apply_beam_preset(&mut self, name: &str) -> bool {
        self.ensure_beam_presets();
        if let Some(preset) = self.beam_presets.get(name).cloned() {
            let clean = preset.sanitized();
            self.active_beam_preset = name.to_string();
            self.beam.apply_mask_preset(&clean);
            self.beam_defaults.apply_mask_preset(&clean);
            true
        } else {
            false
        }
    }

    pub fn request_screenshot(
        &mut self,
        mode: crate::screenshot::ScreenshotMode,
        ctx: &egui::Context,
    ) {
        if !self.ui.video_window_open {
            return;
        }
        let filter = CrtFilter::from_u8(self.crt_filter.load(Ordering::Relaxed));
        let is_crt_on = filter != CrtFilter::Off;
        let night_mode_active = is_crt_on && self.video.effective_lights_off_night_mode();
        let dark_room = night_mode_active
            || self.video.lights_off_night_mode
            || (is_crt_on && self.video.retro_pc_frame_dark_mode);
        let warp = match filter {
            CrtFilter::Lottes => [self.crt.warp_x, self.crt.warp_y],
            CrtFilter::Halo if self.halo.curvature => [0.031, 0.041],
            CrtFilter::Beam if self.beam.curvature => [self.beam.warp_x, self.beam.warp_y],
            _ => [0.0, 0.0],
        };
        let corner_size = match filter {
            CrtFilter::Halo => self.halo.corner_size,
            CrtFilter::Beam => self.beam.corner_size,
            _ => 0.0,
        };
        let scene = crate::screenshot::PhotoSceneContext {
            crt_filter: filter,
            dark_room,
            retro_pc_frame_already_drawn: false,
            crt_glass_already_drawn: self.video.crt_glass_enabled,
            is_fullscreen: self.ui.is_fullscreen,
            window_title: if self.last_window_title.is_empty() {
                "Michadame".to_string()
            } else {
                self.last_window_title.clone()
            },
            warp,
            corner_size,
            border_crop: [
                self.video.border_crop_left,
                self.video.border_crop_right,
                self.video.border_crop_top,
                self.video.border_crop_bottom,
            ],
            halo: self.halo.clone(),
        };
        if self.screenshot.request(mode, scene) {
            ctx.request_repaint();
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    pub(crate) fn update_ocr_projection(
        &mut self,
        projection: crate::video::gpu::geometry::OcrProjection,
    ) {
        let changed = self
            .ocr
            .capture_projection
            .replace(projection)
            .is_some_and(|previous| previous != projection);
        if changed {
            // Reject in-flight results from the old coordinate space, and
            // recapture a pending/manual scan using the new geometry.
            let requested = self.ocr.capture_requested.load(Ordering::Acquire)
                || self.ocr.is_processing.load(Ordering::Acquire);
            self.clear_ocr();
            self.ocr
                .capture_requested
                .store(requested, Ordering::Release);
            if let Some(deck) = &mut self.steam_deck {
                deck.pinned_dictionary = false;
            }
        }
    }

    pub fn toggle_audio_reactive(&mut self) {
        self.audio_reactive.enabled = !self.audio_reactive.enabled;
        self.replay
            .audio
            .set_reactive_listening(self.audio_reactive.enabled);
        config::save_global_hardware_config(self);
        if self.audio_reactive.enabled {
            self.info(format!(
                "Audio-Reactive CRT Interference enabled (Preset: {}).",
                self.audio_reactive.active_preset
            ));
        } else {
            self.info("Audio-Reactive CRT Interference disabled.");
        }
    }

    /// Handles audio and visual transition effects when `lights_off_night_mode` is flipped.
    pub fn on_lights_off_night_mode_changed(&mut self) {
        if self.video.realistic_light_switch {
            let turning_lights_on = !self.video.lights_off_night_mode;
            self.twitch.play_light_switch_sound(turning_lights_on);
            if turning_lights_on {
                // Dark -> Light (lights off -> lights on): trigger ceiling tube ignition flicker
                self.video.light_switch_flicker_start = Some(std::time::Instant::now());
            } else {
                // Light -> Dark (lights on -> lights off): extinguish immediately
                self.video.light_switch_flicker_start = None;
            }
        } else {
            self.video.light_switch_flicker_start = None;
        }
    }

    /// Flips `lights_off_night_mode` and triggers the realistic light switch sound/flicker if enabled.
    pub fn toggle_lights_off_night_mode(&mut self) {
        self.video.lights_off_night_mode = !self.video.lights_off_night_mode;
        self.on_lights_off_night_mode_changed();
    }

    /// If live and configured, shares newly mined vocabulary to Twitch chat as two messages:
    /// - MSG1: `<word> - <sentence>`
    /// - MSG2: `<word> - <jitendex definition>`
    pub fn maybe_share_mined_word_to_twitch(&mut self, entry: &crate::bank::BankEntryMeta) {
        if !self.twitch.config.share_mined_words_to_chat {
            return;
        }
        if !self.twitch.is_stream_live() {
            return;
        }
        if !self.twitch.can_send() {
            return;
        }

        let term_entry =
            crate::bank::entry_from_json(&entry.term, &entry.reading, &entry.definition_json);
        let def = if let Some(e) = term_entry {
            crate::dict::render::compact_definition_for_chat(&e)
        } else {
            entry.definition_text.clone()
        };

        let (msg1, msg2) = crate::dict::render::format_mined_chat_messages(
            &entry.term,
            &entry.reading,
            &entry.sentence,
            &def,
        );

        self.twitch.send_chat_message(&msg1);
        self.twitch.send_chat_message(&msg2);
    }

    /// Checks OCR timeout and clears expired scans when no dictionary popup or whiteboard is open.
    ///
    /// If a dictionary popup is open or the transparent whiteboard is open, the OCR expiration
    /// timer is stopped so scans and popups aren't cleared out from under the user.
    pub fn handle_ocr_timeout(&mut self, ctx: &egui::Context) -> bool {
        let mut repaint_requested = false;
        if self.whiteboard.open {
            let now = Instant::now();
            if let Some(prev_tick) = self.ocr.last_pause_tick {
                let delta = now.saturating_duration_since(prev_tick);
                if let Some(ref mut scan_time) = self.ocr.last_scan_time {
                    *scan_time += delta;
                }
                if let Some(ref mut popup) = self.dict.popup {
                    popup.touch_all_hover_times(now);
                }
            }
            self.ocr.last_pause_tick = Some(now);
            return false;
        }
        self.ocr.last_pause_tick = None;

        if self.ocr.is_constant_scanning_active() {
            return false;
        }

        if self.dict.popup.is_none() {
            if self.ocr.is_expired() {
                self.clear_ocr();
                repaint_requested = true;
            } else if let Some(remaining) = self.ocr.remaining_time() {
                ctx.request_repaint_after(remaining);
            }
        }
        repaint_requested
    }

    /// Drives persistent background OCR scanning when `LocalScreenAi` and `constant_scanning` are enabled.
    ///
    /// Automatically pauses when:
    /// - The dictionary popup is open (`self.dict.popup.is_some()`)
    /// - The cursor is hovering over a visible OCR box (`self.ocr.is_box_hovered && !self.ocr.hide_overlay`)
    /// - The drawing whiteboard is open (`self.whiteboard.open`)
    pub fn handle_constant_ocr_scanning(&mut self, ctx: &egui::Context) -> bool {
        if !self.ocr.is_constant_scanning_active() || !self.ui.video_window_open {
            self.ocr.next_constant_scan_time = None;
            self.ocr.was_constant_scan_paused = false;
            return false;
        }

        // Wait if local model is not installed or is currently syncing
        if self.ocr.screen_ai_installed_metadata.is_none()
            || self.ocr.is_syncing_screen_ai.load(Ordering::Relaxed)
        {
            self.ocr.next_constant_scan_time = None;
            return false;
        }

        let is_paused = self.dict.popup.is_some()
            || (self.ocr.is_box_hovered && !self.ocr.hide_overlay)
            || self.whiteboard.open;
        if is_paused {
            self.ocr.was_constant_scan_paused = true;
            self.ocr.next_constant_scan_time = None;
            return false;
        }

        // If we just resumed from a pause (popup closed / unhovered / whiteboard closed),
        // reset backoff and schedule the next scan after the base interval.
        if self.ocr.was_constant_scan_paused {
            self.ocr.was_constant_scan_paused = false;
            self.ocr.consecutive_same_scans = 0;
            let safe_interval = if self.ocr.scan_interval_secs.is_finite() {
                self.ocr.scan_interval_secs
            } else {
                crate::ocr::models::DEFAULT_SCAN_INTERVAL_SECS
            };
            let delay = std::time::Duration::from_secs_f32(safe_interval.clamp(
                crate::ocr::models::MIN_SCAN_INTERVAL_SECS,
                crate::ocr::models::MAX_SCAN_INTERVAL_SECS,
            ));
            self.ocr.next_constant_scan_time = Some(Instant::now() + delay);
        }

        // Do not schedule or trigger while a capture or worker scan is already in flight
        if self.ocr.is_processing.load(Ordering::Acquire)
            || self.ocr.capture_requested.load(Ordering::Acquire)
        {
            return false;
        }

        let now = Instant::now();
        match self.ocr.next_constant_scan_time {
            Some(target) if now < target => {
                if CrtFilter::from_u8(self.crt_filter.load(Ordering::Relaxed)) != CrtFilter::Beam {
                    ctx.request_repaint_after(target.saturating_duration_since(now));
                }
                false
            }
            _ => {
                self.ocr.next_constant_scan_time = None;
                self.ocr
                    .backend_shared
                    .store(self.ocr.backend.as_u8(), Ordering::Release);
                self.ocr.capture_requested.store(true, Ordering::Release);
                true
            }
        }
    }

    pub fn update_fps_counters(&mut self, ctx: &egui::Context) {
        self.frames_since_last_check += 1;
        let now = Instant::now();
        let elapsed_secs = (now - self.last_fps_check).as_secs_f32();
        let mut updated = false;

        if elapsed_secs >= 1.0 {
            self.gui_fps = self.frames_since_last_check as f32 / elapsed_secs;
            self.last_fps_check = now;
            self.frames_since_last_check = 0;
            updated = true;
        }

        let video_elapsed_secs = (now - self.last_video_fps_check).as_secs_f32();
        if video_elapsed_secs >= 1.0 {
            self.video_fps = self.video_frames_since_last_check as f32 / video_elapsed_secs;
            self.last_video_fps_check = now;
            self.video_frames_since_last_check = 0;
            updated = true;
        }

        if !updated && !self.last_window_title.is_empty() {
            return;
        }

        if let Some(deck) = &mut self.steam_deck {
            if deck
                .last_frame_stats_log
                .is_none_or(|last| now.duration_since(last).as_secs() >= 10)
            {
                deck.last_frame_stats_log = Some(now);
                tracing::info!(
                    "Steam Deck frame rates: UI={:.1} FPS, capture={:.1} FPS, UI limit={} FPS",
                    self.gui_fps,
                    self.video_fps,
                    if self.ui.video_window_open { 60 } else { 30 }
                );
            }
        }

        let audio_latency = self.hardware.audio_latency_ms.load(Ordering::Relaxed);

        let title = if let Some(player) = &self.video_player {
            let filename = player
                .file_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Video File".to_string());
            format!(
                "Michadame Viewer | {} | UI: {:.1} FPS | Video: {:.1} FPS | Audio Latency: {} ms",
                filename, self.gui_fps, self.video_fps, audio_latency
            )
        } else if let Some(hook) = &self.gamescope_hook {
            if hook.is_active {
                let res = hook.resolution();
                format!(
                    "Michadame Viewer | Gamescope ({}x{}) | UI: {:.1} FPS | Video: {:.1} FPS | Audio Latency: {} ms",
                    res.0, res.1, self.gui_fps, self.video_fps, audio_latency
                )
            } else {
                format!(
                    "Michadame Viewer | UI: {:.1} FPS | Video: {:.1} FPS | Audio Latency: {} ms",
                    self.gui_fps, self.video_fps, audio_latency
                )
            }
        } else if self.pc98.is_active {
            format!(
                "Michadame Viewer | NEC PC-98 ({}x{}) | UI: {:.1} FPS | Video: {:.1} FPS | Audio Latency: {} ms",
                self.pc98.resolution.0, self.pc98.resolution.1, self.gui_fps, self.video_fps, audio_latency
            )
        } else if self.pc88.is_active {
            format!(
                "Michadame Viewer | NEC PC-88 ({}x{}) | UI: {:.1} FPS | Video: {:.1} FPS | Audio Latency: {} ms",
                self.pc88.resolution.0, self.pc88.resolution.1, self.gui_fps, self.video_fps, audio_latency
            )
        } else {
            format!(
                "Michadame Viewer | UI: {:.1} FPS | Video: {:.1} FPS | Audio Latency: {} ms",
                self.gui_fps, self.video_fps, audio_latency
            )
        };

        if title != self.last_window_title {
            self.last_window_title = title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }

    pub fn info(&mut self, text: impl Into<String>) {
        let text = text.into();
        if self.steam_deck.is_some() {
            tracing::info!("{text}");
            return;
        }
        self.toasts.add(egui_toast::Toast {
            kind: egui_toast::ToastKind::Info,
            options: egui_toast::ToastOptions::default()
                .duration(std::time::Duration::from_secs(3)),
            text: text.into(),
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        let text = text.into();
        if self.steam_deck.is_some() {
            tracing::error!("{text}");
            return;
        }
        self.toasts.add(egui_toast::Toast {
            kind: egui_toast::ToastKind::Error,
            options: egui_toast::ToastOptions::default()
                .duration(std::time::Duration::from_secs(3)),
            text: text.into(),
        });
    }

    pub fn copy_bank_screenshot(&mut self, id: i64, ctx: &egui::Context) {
        if self.bank.copying_screenshot.is_some() {
            return;
        }
        let Some(bytes) = self.bank.screenshot_bytes(id) else {
            self.error("Failed to load screenshot from database");
            return;
        };
        self.bank.copying_screenshot = Some(id);
        let clipboard = self.replay.clipboard();
        let tx = self.bank.event_sender();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("bank-clipboard-copy".into())
            .spawn(move || {
                let result = clipboard.publish_bytes("screenshot-", ".jpg", &bytes);
                match result {
                    Ok(_) => {
                        let _ = tx.send(crate::bank::BankEvent::ClipboardCopied);
                    }
                    Err(e) => {
                        let _ = tx.send(crate::bank::BankEvent::ClipboardFailed(e.to_string()));
                    }
                }
                ctx.request_repaint();
            });
        if let Err(e) = spawned {
            self.bank.copying_screenshot = None;
            self.error(format!("Failed to start clipboard copy: {e}"));
        }
    }

    /// Applies a worker OCR result if and only if its generation matches the current OCR session.
    pub fn handle_ocr_worker_result(
        &mut self,
        worker_result: crate::ocr::OcrWorkerResult,
        ctx: &egui::Context,
    ) -> bool {
        if !self.ocr.accept_worker_result(&worker_result) {
            return false;
        }
        let model_name = worker_result.backend.short_name();
        let constant_active = worker_result.constant_scan && self.ocr.is_constant_scanning_active();
        let mut repaint_requested = false;
        match worker_result.result {
            Ok(lines) => {
                if constant_active {
                    let is_paused = self.dict.popup.is_some()
                        || (self.ocr.is_box_hovered && !self.ocr.hide_overlay)
                        || self.whiteboard.open;
                    if !is_paused {
                        let updated = self.ocr.apply_constant_scan_lines(lines);
                        let delay = self.ocr.current_constant_scan_delay();
                        self.ocr.next_constant_scan_time = Some(Instant::now() + delay);
                        if CrtFilter::from_u8(self.crt_filter.load(Ordering::Relaxed))
                            != CrtFilter::Beam
                        {
                            ctx.request_repaint_after(delay);
                        }
                        if updated {
                            repaint_requested = true;
                        }
                    } else {
                        self.ocr.was_constant_scan_paused = true;
                        self.ocr.next_constant_scan_time = None;
                    }
                } else {
                    self.ocr.raw_lines = lines;
                    self.ocr.recompute_boxes();
                    let count = self.ocr.boxes.len();
                    self.ocr.last_copied_index = None;
                    self.ocr.last_error = None;
                    if count == 0 {
                        self.ocr.last_scan_time = None;
                        self.info(format!("{}: No Japanese text detected.", model_name));
                    } else {
                        self.ocr.last_scan_time = Some(Instant::now());
                        self.info(format!(
                            "{}: Found {} text box{}.",
                            model_name,
                            count,
                            if count == 1 { "" } else { "es" }
                        ));
                    }
                    repaint_requested = true;
                }
            }
            Err(e) => {
                let is_same_error = self.ocr.last_error.as_deref() == Some(e.as_str());
                self.ocr.last_error = Some(e.clone());
                if !constant_active || !is_same_error {
                    self.error(format!("OCR error ({}): {}", model_name, e));
                }
                if constant_active {
                    self.ocr.consecutive_same_scans =
                        self.ocr.consecutive_same_scans.saturating_add(1);
                    let delay = self.ocr.current_constant_scan_delay();
                    self.ocr.next_constant_scan_time = Some(Instant::now() + delay);
                    ctx.request_repaint_after(delay);
                }
                repaint_requested = true;
            }
        }
        repaint_requested
    }

    pub fn apply_received_video_frame(&mut self, frame: Arc<video::types::RawFrame>) {
        if self.video.fft_filter_enabled {
            let (fft_w, fft_h) =
                crate::video::gpu::FftFilter::fft_dimensions(frame.width, frame.height);
            if self.fft_mask_resolution != (fft_w, fft_h) {
                self.fft_mask_resolution = (fft_w, fft_h);
                self.fft_mask_data = vec![255u8; (fft_w * fft_h) as usize];
            }
        }
        self.latest_frame = Some(frame);
        self.video_frames_since_last_check += 1;
    }

    /// Drains any newly arrived source video frames from the bounded channel during intermediate
    /// CRT sub-frames (`s = 1 .. N-1`) or post-update into `self.beam_lockstep.staged_*` without
    /// mutating `self.latest_frame` or `hook.current_dmabuf_frame` mid-sweep.
    ///
    /// Because `self.frame_receiver` has capacity 1 (`bounded(1)`), draining it multiple times per
    /// sweep prevents the producer thread (`try_send`) from ever dropping frames due to a full
    /// channel while the CRT beam is scanning.
    pub fn stage_source_frames_for_next_sweep(&mut self, just_after_sub0: bool) {
        if let Some(ref mut hook) = self.gamescope_hook {
            if hook.is_active {
                if let Some(ref dma) = hook.dma_handle {
                    let mut drained_timestamps = Vec::new();
                    let mut latest = None;
                    while let Ok(frame) = dma.rx.try_recv() {
                        drained_timestamps.push(frame.captured_at);
                        latest = Some(frame);
                        self.video_frames_since_last_check += 1;
                    }
                    if let Some(frame) = latest {
                        self.beam_lockstep.staged_dmabuf_frame = Some(Arc::new(frame));
                        let now_inst = Instant::now();
                        for ts in drained_timestamps {
                            self.beam_lockstep
                                .record_source_frame(ts, now_inst, just_after_sub0);
                        }
                    }
                    return;
                }
            }
        }

        let mut drained = Vec::new();
        if let Some(ref rx) = self.frame_receiver {
            while let Ok(frame) = rx.try_recv() {
                drained.push(frame);
            }
        }
        if !drained.is_empty() {
            let now_inst = Instant::now();
            for frame in drained {
                let ts = frame.captured_at;
                if self
                    .beam_lockstep
                    .staged_video_frame
                    .replace(frame)
                    .is_some()
                {
                    self.video_frames_since_last_check += 1;
                }
                self.beam_lockstep
                    .record_source_frame(ts, now_inst, just_after_sub0);
            }
        }
    }

    /// Polls or waits up to `timeout` for a new source video frame from either Gamescope DMA-BUF
    /// or `self.frame_receiver`. Returns `(got_frame, is_live_edge)` where `is_live_edge` is true
    /// if the queue was initially empty and the frame arrived during `recv_timeout`.
    pub fn poll_or_wait_source_frame(&mut self, timeout: std::time::Duration) -> (bool, bool) {
        let record_beam_arrival =
            CrtFilter::from_u8(self.crt_filter.load(Ordering::Relaxed)) == CrtFilter::Beam;

        if let Some(ref mut hook) = self.gamescope_hook {
            if hook.is_active {
                if let Some(ref dma) = hook.dma_handle {
                    let mut got_staged = false;
                    if let Some(staged) = self.beam_lockstep.staged_dmabuf_frame.take() {
                        hook.current_dmabuf_frame = Some(staged);
                        self.beam_lockstep.sweeps_since_live_arrival = 0;
                        self.beam_lockstep.phase_locked = true;
                        got_staged = true;
                    }
                    let mut drained_timestamps = Vec::new();
                    let mut latest = None;
                    while let Ok(frame) = dma.rx.try_recv() {
                        drained_timestamps.push(frame.captured_at);
                        latest = Some(frame);
                        self.video_frames_since_last_check += 1;
                    }
                    if let Some(frame) = latest {
                        hook.current_dmabuf_frame = Some(Arc::new(frame));
                        if record_beam_arrival {
                            let now_inst = Instant::now();
                            for ts in drained_timestamps {
                                self.beam_lockstep.record_source_frame(ts, now_inst, false);
                            }
                        }
                        return (true, false);
                    }
                    if got_staged {
                        return (true, false);
                    }
                    if !timeout.is_zero() {
                        if let Ok(frame) = dma.rx.recv_timeout(timeout) {
                            drained_timestamps.push(frame.captured_at);
                            let mut newest = frame;
                            self.video_frames_since_last_check += 1;
                            while let Ok(extra) = dma.rx.try_recv() {
                                drained_timestamps.push(extra.captured_at);
                                newest = extra;
                                self.video_frames_since_last_check += 1;
                            }
                            hook.current_dmabuf_frame = Some(Arc::new(newest));
                            if record_beam_arrival {
                                let now_inst = Instant::now();
                                for ts in drained_timestamps {
                                    self.beam_lockstep.record_source_frame(ts, now_inst, false);
                                }
                            }
                            return (true, true);
                        }
                    }
                    return (false, false);
                }
            }
        }

        let mut got_staged = false;
        if let Some(staged) = self.beam_lockstep.staged_video_frame.take() {
            self.apply_received_video_frame(staged);
            self.beam_lockstep.sweeps_since_live_arrival = 0;
            self.beam_lockstep.phase_locked = true;
            got_staged = true;
        }

        let mut drained = Vec::new();
        let mut live = false;
        if let Some(ref rx) = self.frame_receiver {
            while let Ok(frame) = rx.try_recv() {
                drained.push(frame);
            }
            if drained.is_empty() && !got_staged && !timeout.is_zero() {
                if let Ok(frame) = rx.recv_timeout(timeout) {
                    live = true;
                    drained.push(frame);
                    while let Ok(extra) = rx.try_recv() {
                        drained.push(extra);
                    }
                }
            }
        }
        if drained.is_empty() {
            (got_staged, false)
        } else {
            let now_inst = Instant::now();
            for frame in drained {
                let ts = frame.captured_at;
                self.apply_received_video_frame(frame);
                if record_beam_arrival {
                    self.beam_lockstep.record_source_frame(ts, now_inst, false);
                }
            }
            (true, live)
        }
    }

    /// Deck mode has a real presentation limit even when Xwayland ignores VSync.
    /// Desktop mode retains its existing event-driven scheduling.
    pub fn pace_gui_frame(&mut self) {
        if self.steam_deck.is_some() {
            // Steam's Xwayland may ignore the swap interval. Keep an actual
            // CPU-side limit as well, including the launcher and setup tabs.
            if let Some(next) = self.next_gui_frame_time {
                std::thread::sleep(next.saturating_duration_since(Instant::now()));
            }
            self.next_gui_frame_time =
                Some(Instant::now() + crate::steam_deck::frame_interval(self.ui.video_window_open));
            return;
        }
        if self.ui.video_window_open {
            let active_filter = CrtFilter::from_u8(self.crt_filter.load(Ordering::Relaxed));
            if active_filter == CrtFilter::Beam {
                let beam_params = crate::video::gpu::BeamShaderParams::from_state(self);
                let effective_fph = beam_params.effective_frames_per_hz();
                let fps_divisor = self.beam.fps_divisor.clamp(0.05, 1.0);
                let target_hz = self.beam.effective_target_hz().max(60);

                if (effective_fph - self.beam_lockstep.last_effective_fph).abs() > 1e-4
                    || (fps_divisor - self.beam_lockstep.last_fps_divisor).abs() > 1e-4
                    || target_hz != self.beam_lockstep.last_target_hz
                {
                    self.beam_lockstep.reset(effective_fph, fps_divisor);
                    self.beam_lockstep.last_target_hz = target_hz;
                }

                let base_fph = if self.beam.frames_per_hz.is_finite() {
                    self.beam.frames_per_hz.max(1.0)
                } else {
                    2.0
                };
                let rounded_fph = base_fph.round();
                let is_integer_lockstep =
                    (base_fph - rounded_fph).abs() < 1e-4 && (fps_divisor - 1.0).abs() < 1e-4;
                let n_subframes = (rounded_fph as u32).max(1);

                if is_integer_lockstep {
                    match self.beam_lockstep.sweep_start_time {
                        None => {
                            let (got, _) =
                                self.poll_or_wait_source_frame(std::time::Duration::ZERO);
                            let start = Instant::now();
                            self.beam_lockstep.received_on_last_sweep = got;
                            self.beam_lockstep.sweep_start_time = Some(start);
                            self.beam_lockstep.last_present_time = Some(start);
                            self.beam_lockstep.subframe_index = 0;
                            self.beam_lockstep.current_i_frame =
                                ((self.beam_lockstep.sweep_counter * n_subframes) % 8_000_000)
                                    as i32;
                            self.beam_lockstep
                                .commit_next_sweep_duration(start, got, got);
                        }
                        Some(sweep_start) => {
                            let entry_now = Instant::now();
                            let sweep_dur = self.beam_lockstep.current_sweep_duration;
                            let next_sub = self.beam_lockstep.subframe_index + 1;
                            if next_sub == 1 {
                                let render_dur = entry_now
                                    .saturating_duration_since(sweep_start)
                                    .min(sweep_dur / n_subframes.max(1));
                                if self.beam_lockstep.sub0_render_dur.is_zero() {
                                    self.beam_lockstep.sub0_render_dur = render_dur;
                                } else {
                                    self.beam_lockstep.sub0_render_dur =
                                        self.beam_lockstep.sub0_render_dur.mul_f64(0.75)
                                            + render_dur.mul_f64(0.25);
                                }
                            } else if let Some(prev_wake) = self.beam_lockstep.last_present_time {
                                let render_dur = entry_now
                                    .saturating_duration_since(prev_wake)
                                    .min(sweep_dur / n_subframes.max(1));
                                if self.beam_lockstep.sub_render_ema.is_zero() {
                                    self.beam_lockstep.sub_render_ema = render_dur;
                                } else {
                                    self.beam_lockstep.sub_render_ema =
                                        self.beam_lockstep.sub_render_ema.mul_f64(0.75)
                                            + render_dur.mul_f64(0.25);
                                }
                            }
                            if next_sub < n_subframes {
                                // Intermediate sub-frame (s = 1 .. N-1) of the current CRT sweep:
                                // Stage any newly arrived frame out of the `bounded(1)` channel
                                // without mutating `self.latest_frame` mid-sweep.
                                // Compensate for the render/swap duration asymmetry between
                                // Sub-frame 0 (video upload + 5 preprocess passes) and Sub-frame 1..
                                // (PreprocessCacheSig hit) so `swap_buffers()` lands at the exact
                                // `next_sub / n_subframes` midpoint between Sub-frame 0 swaps.
                                let frac = (next_sub as f64) / (n_subframes as f64);
                                let lead_offset = self
                                    .beam_lockstep
                                    .sub0_render_dur
                                    .saturating_sub(self.beam_lockstep.sub_render_ema)
                                    .min(sweep_dur / (n_subframes * 3).max(1));
                                let target_time =
                                    sweep_start + sweep_dur.mul_f64(frac) + lead_offset;
                                sleep_until_precise(target_time);
                                self.beam_lockstep.last_present_time = Some(Instant::now());
                                self.stage_source_frames_for_next_sweep(false);
                                self.beam_lockstep.subframe_index = next_sub;
                                self.beam_lockstep.current_i_frame =
                                    ((self.beam_lockstep.sweep_counter * n_subframes + next_sub)
                                        % 8_000_000) as i32;
                            } else {
                                // Sub-frame 0 of the next CRT sweep:
                                // Ensure Sub-frame 1 gets at least the same VRR exposure duration
                                // as Sub-frame 0 even when UI work (e.g. open Debug window or
                                // secondary viewport) pushes Sub-frame 0 beyond `sweep_dur / N`.
                                self.beam_lockstep.sweeps_since_live_arrival = self
                                    .beam_lockstep
                                    .sweeps_since_live_arrival
                                    .saturating_add(1);
                                let prev_sub_start =
                                    self.beam_lockstep.last_present_time.unwrap_or(sweep_start);
                                let max_sub_dur =
                                    self.beam_lockstep.max_sweep_duration() / n_subframes;
                                let actual_sub0_dur = prev_sub_start
                                    .saturating_duration_since(sweep_start)
                                    .min(max_sub_dur);
                                let nominal_next_sweep =
                                    (sweep_start + sweep_dur).max(prev_sub_start + actual_sub0_dur);
                                sleep_until_precise(nominal_next_sweep);

                                let (got_frame, _) =
                                    self.poll_or_wait_source_frame(std::time::Duration::ZERO);
                                let post_now = Instant::now();
                                let has_channel = self.frame_receiver.is_some()
                                    || self
                                        .gamescope_hook
                                        .as_ref()
                                        .map_or(false, |h| h.is_active && h.dma_handle.is_some());
                                let stream_recently_active = has_channel
                                    && (got_frame
                                        || self.beam_lockstep.last_source_frame_arrival.map_or(
                                            false,
                                            |t| {
                                                post_now.saturating_duration_since(t)
                                                    < std::time::Duration::from_millis(120)
                                            },
                                        ));

                                let next_sweep_start = nominal_next_sweep.max(post_now);
                                self.beam_lockstep.sweep_start_time = Some(next_sweep_start);
                                self.beam_lockstep.last_present_time = Some(next_sweep_start);
                                self.beam_lockstep.received_on_last_sweep = got_frame;
                                if !got_frame
                                    && self.beam_lockstep.sweeps_since_live_arrival
                                        > self.beam_lockstep.sweeps_per_frame
                                {
                                    self.beam_lockstep.phase_locked = false;
                                }

                                self.beam_lockstep.sweep_counter =
                                    (self.beam_lockstep.sweep_counter + 1)
                                        % (8_000_000 / n_subframes.max(1));
                                self.beam_lockstep.subframe_index = 0;
                                self.beam_lockstep.current_i_frame =
                                    (self.beam_lockstep.sweep_counter * n_subframes) as i32;
                                self.beam_lockstep.commit_next_sweep_duration(
                                    next_sweep_start,
                                    got_frame,
                                    stream_recently_active,
                                );
                            }
                        }
                    }
                    return;
                }

                // Non-integer `frames_per_hz` (e.g. 2.4 for 144Hz Native) or slow-motion `fps_divisor < 1.0`:
                // Drain any arrived source frame before `draw_video_player` and pace at
                // `smoothed_sweep_duration / effective_fph` (or `1 / target_hz` when no stream is measured).
                let _ = self.poll_or_wait_source_frame(std::time::Duration::ZERO);
                let target_duration = if self.beam_lockstep.has_measured_interval
                    && (fps_divisor - 1.0).abs() < 1e-4
                {
                    self.beam_lockstep
                        .smoothed_sweep_duration
                        .div_f64(effective_fph.max(1.0) as f64)
                } else {
                    std::time::Duration::from_nanos(1_000_000_000 / (target_hz as u64))
                };
                if let Some(next_time) = self.beam_lockstep.next_subframe_time {
                    sleep_until_precise(next_time);
                    let post_now = Instant::now();
                    if post_now > next_time + target_duration * 2
                        || next_time > post_now + target_duration * 2
                    {
                        self.beam_lockstep.next_subframe_time = Some(post_now + target_duration);
                    } else {
                        self.beam_lockstep.next_subframe_time = Some(next_time + target_duration);
                    }
                } else {
                    self.beam_lockstep.next_subframe_time = Some(Instant::now() + target_duration);
                }
                self.beam_lockstep.current_i_frame =
                    (self.beam_lockstep.current_i_frame + 1) % 8_000_000;
                return;
            }

            if let Some(staged) = self.beam_lockstep.staged_video_frame.take() {
                self.apply_received_video_frame(staged);
            }
            if let Some(staged_dma) = self.beam_lockstep.staged_dmabuf_frame.take() {
                if let Some(ref mut hook) = self.gamescope_hook {
                    hook.current_dmabuf_frame = Some(staged_dma);
                }
            }
            self.beam_lockstep.deactivate();
            const TARGET_FRAME_DURATION: std::time::Duration =
                std::time::Duration::from_nanos(1_000_000_000 / 60);

            let now = Instant::now();
            if let Some(ref mut next_time) = self.next_gui_frame_time {
                let post_now = Instant::now();
                if post_now > *next_time + TARGET_FRAME_DURATION * 2
                    || *next_time > post_now + TARGET_FRAME_DURATION * 2
                {
                    *next_time = post_now + TARGET_FRAME_DURATION;
                } else {
                    *next_time += TARGET_FRAME_DURATION;
                }
            } else {
                self.next_gui_frame_time = Some(now + TARGET_FRAME_DURATION);
            }
        } else {
            if let Some(staged) = self.beam_lockstep.staged_video_frame.take() {
                self.apply_received_video_frame(staged);
            }
            if let Some(staged_dma) = self.beam_lockstep.staged_dmabuf_frame.take() {
                if let Some(ref mut hook) = self.gamescope_hook {
                    hook.current_dmabuf_frame = Some(staged_dma);
                }
            }
            self.beam_lockstep.deactivate();
            self.next_gui_frame_time = None;
        }
    }
}

fn sleep_until_precise(target: Instant) {
    const MAX_SLEEP: std::time::Duration = std::time::Duration::from_millis(30);
    const SPIN_MARGIN: std::time::Duration = std::time::Duration::from_micros(1200);
    let now = Instant::now();
    if target <= now {
        return;
    }
    let target = target.min(now + MAX_SLEEP);
    let remaining = target - now;
    if remaining > SPIN_MARGIN {
        std::thread::sleep(remaining - SPIN_MARGIN);
    }
    while Instant::now() < target {
        std::hint::spin_loop();
    }
}

impl eframe::App for AppState {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.ui.config_save_pending {
            config::flush_pending_config(self);
        }
        self.replay.disable();
        if let Some(gl) = _gl {
            self.replay.gpu.lock().unwrap().destroy(gl);
            if let Some(renderer) = self.crt_renderer.as_ref() {
                renderer.lock().unwrap().destroy(gl);
            }
            if let Some(fft) = self.fft_filter.as_ref() {
                fft.lock().unwrap().destroy(gl);
            }
        }
        self.stop_stream_resources();
        self.mic.active_stream = None;
        self.webcam.active_stream = None;
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui.in_egui_frame = true;
        self.ui.any_pointer_down = ctx.input(|i| i.pointer.any_down());
        if self.steam_deck.is_none() {
            ui::scaling::apply_ui_scale(ctx, &mut self.ui.scale);
        }
        self.pace_gui_frame();

        let raw_audio_peak = self
            .hardware
            .audio_peak_amplitude
            .swap(0, Ordering::Relaxed) as f32
            / 1000.0;
        self.hardware.live_audio_peak = raw_audio_peak.max(self.hardware.live_audio_peak * 0.88);

        let reactive_listen = self.audio_reactive.enabled
            || (self.ui.control_window_open
                && self.ui.active_settings_tab == crate::app::models::SettingsTab::SpecialFx);
        self.replay.audio.set_reactive_listening(reactive_listen);
        let raw_reactive = self.replay.audio.take_reactive_raw();
        self.audio_reactive_runtime.step(
            raw_reactive,
            self.hardware.live_audio_peak,
            &self.audio_reactive.current,
            reactive_listen,
        );

        let mic_db = f32::from_bits(self.mic.current_level_db.load(Ordering::Relaxed));
        let raw_mic =
            if self.mic.config.enabled && self.mic.active_stream.is_some() && mic_db > -80.0 {
                ((mic_db + 60.0) / 60.0).clamp(0.0, 1.0)
            } else {
                0.0
            };
        self.hardware.live_mic_peak = raw_mic.max(self.hardware.live_mic_peak * 0.88);

        self.replay.update();
        if let Some(message) = self.replay.notification() {
            self.info(message);
        }
        if self.replay.runtime.is_some() || self.replay.status().saving {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        self.ui.is_fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));

        // If Gamescope hook or PC-98/PC-88 mode is active, ensure any widget focus captured by
        // Shift+Tab tab-navigation is cleared so hotkeys and passthrough work cleanly,
        // unless a dialog, Twitch overlay input, or controls window is open for explicit user text input.
        if self.steam_deck.is_none()
            && (self.gamescope_hook.as_ref().map_or(false, |h| h.is_active)
                || self.pc98.is_active
                || self.pc88.is_active)
        {
            if !self.ui.control_window_open
                && !self.ui.show_stop_stream_dialog
                && !self.ui.show_quit_dialog
            {
                ctx.memory_mut(|m| {
                    if let Some(id) = m.focused() {
                        m.surrender_focus(id);
                    }
                });
            }
        }

        let gamescope_passthrough = self
            .gamescope_hook
            .as_ref()
            .map_or(false, |h| h.is_active && h.passthrough_enabled);
        let pc98_passthrough = (self.pc98.is_active && self.pc98.passthrough_enabled)
            || (self.pc88.is_active && self.pc88.passthrough_enabled);

        if self.steam_deck.is_none() && !gamescope_passthrough && !pc98_passthrough {
            self.replay.shortcuts(ctx);
        }
        let mut repaint_requested = false;

        // Wait for the startup UI scale to settle so the controls window is not created at
        // the wrong pixel size and immediately resized.
        if self.steam_deck.is_none() && self.ui.control_window_open && self.ui.scale.settled_once {
            let (controls_viewport, fitted) = ui::scaling::fixed_size_viewport(
                egui::ViewportBuilder::default().with_title("Michadame Controls"),
                ui::CONTROLS_WINDOW_SIZE,
                ui::scaling::monitor_size(ctx, &self.ui.scale),
            );
            ctx.show_viewport_immediate(
                ui::scaling::viewport_id("control_window", ctx, fitted.size),
                controls_viewport,
                |ctx, _class| {
                    self.ui.any_pointer_down |= ctx.input(|i| i.pointer.any_down());

                    repaint_requested |= ui::draw_main_ui(self, ctx);

                    if ctx.input(|i| i.viewport().close_requested()) {
                        self.ui.control_window_open = false;
                    }
                },
            );
        }

        if self.steam_deck.is_some() {
            repaint_requested |= ui::steam_deck::draw(self, ctx);
        } else {
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    ui::draw_video_player(self, ui, ctx);

                    if self.ui.show_stop_stream_dialog {
                        ui::dialogs::show_stop_stream_dialog(self, ctx, ui, ctx);
                    }

                    if self.ui.show_quit_dialog {
                        ui::dialogs::show_quit_dialog(self, ctx, ui);
                    }

                    if !self.ui.control_window_open
                        && self.config_load_error.is_some()
                        && !self.ui.dismissed_config_error
                    {
                        ui::dialogs::show_config_error_dialog(self, ctx, ui);
                    }
                });
        }

        if let Some(count) = self.fullscreen_toggle_frame_count {
            match count {
                0 => {
                    self.fullscreen_toggle_frame_count = Some(1);
                }
                1 => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                    self.fullscreen_toggle_frame_count = None;
                }
                _ => self.fullscreen_toggle_frame_count = None,
            }
            repaint_requested = true;
        }

        // Re-evaluate passthrough state as draw_video_player may have toggled it via Shift+Tab
        let gamescope_passthrough = self
            .gamescope_hook
            .as_ref()
            .map_or(false, |h| h.is_active && h.passthrough_enabled);
        let pc98_passthrough = (self.pc98.is_active && self.pc98.passthrough_enabled)
            || (self.pc88.is_active && self.pc88.passthrough_enabled);

        if self.steam_deck.is_none()
            && (self.gamescope_hook.as_ref().map_or(false, |h| h.is_active)
                || self.pc98.is_active
                || self.pc88.is_active)
        {
            if !self.ui.control_window_open
                && !self.ui.show_stop_stream_dialog
                && !self.ui.show_quit_dialog
            {
                ctx.memory_mut(|m| {
                    if let Some(id) = m.focused() {
                        m.surrender_focus(id);
                    }
                });
            }
        }

        // Video-window hotkeys must not fire while typing or while Gamescope/PC-98 passthrough is active.
        let typing = ctx.wants_keyboard_input();
        if self.steam_deck.is_none() && !gamescope_passthrough && !pc98_passthrough {
            if !typing && ctx.input(|i| i.key_pressed(egui::Key::F)) {
                let is_fullscreen = !ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(is_fullscreen));
            }
            if !ctx.wants_keyboard_input()
                && ctx.input(|i| i.modifiers.is_none() && i.key_pressed(egui::Key::C))
            {
                let current_filter = CrtFilter::from_u8(self.crt_filter.load(Ordering::Relaxed));
                let next_filter = if current_filter != CrtFilter::Off {
                    CrtFilter::Off
                } else if self.selected_crt_filter != CrtFilter::Off {
                    self.selected_crt_filter
                } else {
                    CrtFilter::Lottes
                };
                self.crt_filter.store(next_filter as u8, Ordering::Relaxed);
                self.beam_lockstep.deactivate();
                config::save_config(self);
                self.info(format!("CRT filter set to: {}", next_filter));
                ctx.request_repaint();
            }
            if !ctx.wants_keyboard_input()
                && ctx.input(|i| i.modifiers.is_none() && i.key_pressed(egui::Key::K))
            {
                let current_filter = CrtFilter::from_u8(self.crt_filter.load(Ordering::Relaxed));
                if current_filter != CrtFilter::Off {
                    self.toggle_lights_off_night_mode();
                    config::save_config(self);
                    ctx.request_repaint();
                }
            }
            if !typing && ctx.input(|i| i.key_pressed(egui::Key::G)) {
                self.video.pixelate_filter_enabled = !self.video.pixelate_filter_enabled;
                let status = if self.video.pixelate_filter_enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                self.info(format!("480p Pixelate filter {}.", status));
                config::save_config(self);
            }
            // While typing, Escape only unfocuses the text field.
            if !typing && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
            if !typing
                && ctx.input(|i| i.key_pressed(egui::Key::Q))
                && self.ui.video_window_open
                && !self.ui.show_stop_stream_dialog
            {
                self.ui.show_stop_stream_dialog = true;
            }
            if !typing && ctx.input(|i| i.key_pressed(egui::Key::M)) {
                self.ui.control_window_open = !self.ui.control_window_open;
            }
            if crate::ui::check_toggle_key(ctx, egui::Key::L) {
                self.ui.studio_window_open = !self.ui.studio_window_open;
                repaint_requested = true;
            }
            if crate::ui::check_toggle_key(ctx, egui::Key::H) {
                self.whiteboard.toggle();
                repaint_requested = true;
            }
            if crate::ui::check_toggle_key(ctx, egui::Key::J) {
                self.toggle_audio_reactive();
                repaint_requested = true;
            }
            if !ctx.wants_keyboard_input()
                && ctx.input(|i| i.modifiers.is_none() && i.key_pressed(egui::Key::B))
            {
                self.bank.window_open = !self.bank.window_open;
                if !self.bank.window_open {
                    self.bank.enlarged = None;
                }
            }
            if self.ui.video_window_open {
                if let Some(mode) = crate::screenshot::check_screenshot_hotkeys(ctx) {
                    self.request_screenshot(mode, ctx);
                    repaint_requested = true;
                }
            }
        }

        // Playback hotkeys when in video file mode
        if !typing && self.video_player.is_some() && self.ui.video_window_open {
            if ctx.input(|i| i.key_pressed(egui::Key::P)) {
                if let Some(ref p) = self.video_player {
                    let _ = p
                        .command_sender
                        .send(crate::video::player::PlayerCommand::TogglePause);
                    repaint_requested = true;
                }
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
                if let Some(ref p) = self.video_player {
                    let target = (p.current_position_secs - 5.0).max(0.0);
                    let _ = p
                        .command_sender
                        .send(crate::video::player::PlayerCommand::Seek(target));
                    repaint_requested = true;
                }
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
                if let Some(ref p) = self.video_player {
                    let target = (p.current_position_secs + 5.0).min(p.duration_secs);
                    let _ = p
                        .command_sender
                        .send(crate::video::player::PlayerCommand::Seek(target));
                    repaint_requested = true;
                }
            }
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            if self.steam_deck.is_none() && self.ui.video_window_open && !self.ui.show_quit_dialog {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.ui.show_quit_dialog = true;
            }
            repaint_requested = true;
        }

        if let Some(rx) = &self.video_file_dialog_receiver {
            if let Ok(path) = rx.try_recv() {
                self.start_video_file(&path, ctx);
                repaint_requested = true;
            }
        }

        if let Some(rx) = &self.gamescope_script_dialog_receiver {
            if let Ok(path) = rx.try_recv() {
                self.gamescope_launcher_script = path.to_string_lossy().to_string();
                crate::config::save_config(self);
                crate::config::save_global_hardware_config(self);
                repaint_requested = true;
            }
        }

        let dropped_file = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()));
        if let Some(path) = dropped_file.filter(|_| self.steam_deck.is_none()) {
            self.start_video_file(&path, ctx);
            repaint_requested = true;
        }

        if let Some(rx) = &self.device_scan_receiver {
            if let Ok(scan_result) = rx.try_recv() {
                repaint_requested |=
                    self.handle_device_scan_result_with_ctx(scan_result, Some(ctx));
            } else {
                repaint_requested = true;
            }
        }

        repaint_requested |= self.handle_video_thread_events(ctx);
        repaint_requested |= self.handle_audio_thread_events(ctx);
        repaint_requested |= self.handle_video_file_player_events(ctx);
        repaint_requested |= self.handle_pc98_events(ctx);
        repaint_requested |= self.handle_pc88_events(ctx);

        if let Some(ref mut p) = self.video_player {
            if !p.is_seeking {
                p.current_position_secs =
                    p.position_us.load(Ordering::Relaxed) as f64 / 1_000_000.0;
            }
            p.is_paused = p.is_paused_shared.load(Ordering::Relaxed);
            p.is_eof = p.is_eof_shared.load(Ordering::Relaxed);
        }

        let beam_lockstep_active = self.ui.video_window_open
            && self.steam_deck.is_none()
            && CrtFilter::from_u8(self.crt_filter.load(Ordering::Relaxed)) == CrtFilter::Beam;
        if !beam_lockstep_active {
            if let Some(rx) = &self.frame_receiver {
                let mut incoming = Vec::new();
                while let Ok(frame) = rx.try_recv() {
                    incoming.push(frame);
                }
                for frame in incoming {
                    self.apply_received_video_frame(frame);
                    repaint_requested = true;
                }
            }
        }

        // Draw FFT mask editor window and handle mask upload
        if self.video.fft_mask_window_open {
            let mask_changed = ui::fft_mask::draw_fft_mask_editor(self, ctx);
            if mask_changed {
                self.fft_mask_dirty = true;
            }
        }

        // Upload mask to GPU when dirty (from painting or loading from disk)
        if self.fft_mask_dirty {
            self.fft_mask_dirty = false;
            let fft_arc = self.fft_filter.clone();
            let mask_data = self.fft_mask_data.clone();
            let mask_res = self.fft_mask_resolution;
            if let Some(fft_arc) = fft_arc {
                let callback = egui::PaintCallback {
                    rect: egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0)),
                    callback: std::sync::Arc::new(eframe::egui_glow::CallbackFn::new(
                        move |_info, painter| {
                            let fft = fft_arc.lock().unwrap();
                            fft.upload_mask(painter.gl(), &mask_data, mask_res.0, mask_res.1);
                        },
                    )),
                };
                ctx.layer_painter(egui::LayerId::background()).add(callback);
            }
            repaint_requested = true;
        }

        while let Some(worker_result) = self
            .ocr
            .result_receiver
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            repaint_requested |= self.handle_ocr_worker_result(worker_result, ctx);
        }

        let screen_ai_events: Vec<crate::ocr::ScreenAiEvent> =
            if let Some(rx) = &self.ocr.screen_ai_event_rx {
                let mut events = Vec::new();
                while let Ok(ev) = rx.try_recv() {
                    events.push(ev);
                }
                events
            } else {
                Vec::new()
            };

        for event in screen_ai_events {
            match event {
                crate::ocr::ScreenAiEvent::UpdateCheckFinished {
                    update_available,
                    remote_metadata,
                } => {
                    self.ocr.screen_ai_update_available = update_available;
                    self.ocr.screen_ai_remote_metadata = remote_metadata;
                    repaint_requested = true;
                }
                crate::ocr::ScreenAiEvent::SyncProgress { message, progress } => {
                    if let Ok(mut lock) = self.ocr.screen_ai_sync_progress.lock() {
                        *lock = Some((message, progress));
                    }
                    repaint_requested = true;
                }
                crate::ocr::ScreenAiEvent::SyncFinished(result) => {
                    if let Ok(mut lock) = self.ocr.screen_ai_sync_progress.lock() {
                        *lock = None;
                    }
                    match result {
                        Ok(meta) => {
                            self.ocr.screen_ai_sync_error = None;
                            self.ocr.screen_ai_installed_metadata = Some(meta.clone());
                            self.ocr.screen_ai_update_available = false;
                            let dir = self.ocr.screen_ai_dir.clone();
                            let restart_needed =
                                crate::ocr::screen_ai::is_screen_ai_restart_required(&dir);
                            if !restart_needed {
                                std::thread::spawn(move || {
                                    let _ = crate::ocr::screen_ai::warmup_screen_ai(&dir);
                                });
                                self.info(format!(
                                    "Chrome Screen AI local OCR model updated to {}!",
                                    meta.version
                                ));
                            } else {
                                self.info(format!(
                                    "Chrome Screen AI {} staged on disk (restart Michadame to activate new runtime).",
                                    meta.version
                                ));
                            }
                        }
                        Err(e) => {
                            self.ocr.screen_ai_sync_error = Some(e.clone());
                            self.error(format!("Chrome Screen AI model sync failed: {e}"));
                        }
                    }
                    repaint_requested = true;
                }
            }
        }

        if !self.ocr.boxes.is_empty() && self.ocr.last_scan_time.is_none() {
            self.ocr.last_scan_time = Some(Instant::now());
        }

        repaint_requested |= self.handle_ocr_timeout(ctx);
        repaint_requested |= self.handle_constant_ocr_scanning(ctx);

        let dict_events: Vec<crate::dict::DictEvent> = if let Some(rx) = &self.dict.event_rx {
            let mut events = Vec::new();
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
            events
        } else {
            Vec::new()
        };

        for event in dict_events {
            match event {
                crate::dict::DictEvent::UpdateCheckFinished {
                    update_available,
                    remote_metadata,
                } => {
                    self.dict.update_available = update_available;
                    self.dict.remote_metadata = remote_metadata;
                    repaint_requested = true;
                }
                crate::dict::DictEvent::SyncProgress { message, progress } => {
                    if let Ok(mut lock) = self.dict.sync_progress.lock() {
                        *lock = Some((message, progress));
                    }
                    repaint_requested = true;
                }
                crate::dict::DictEvent::SyncFinished(result) => {
                    if let Ok(mut lock) = self.dict.sync_progress.lock() {
                        *lock = None;
                    }
                    match result {
                        Ok(meta) => match self.dict.reload_dict_db(Some(meta.clone())) {
                            Ok(meta) => {
                                self.info(format!(
                                    "Jitendex dictionary updated to {}!",
                                    meta.revision
                                ));
                            }
                            Err(e) => {
                                self.error(e.to_string());
                            }
                        },
                        Err(e) => {
                            self.error(format!("Dictionary sync failed: {e}"));
                        }
                    }
                    repaint_requested = true;
                }
                crate::dict::DictEvent::FreqUpdateCheckFinished {
                    update_available,
                    remote_metadata,
                } => {
                    self.dict.freq_update_available = update_available;
                    self.dict.remote_freq_metadata = remote_metadata;
                    repaint_requested = true;
                }
                crate::dict::DictEvent::FreqSyncProgress { message, progress } => {
                    if let Ok(mut lock) = self.dict.freq_sync_progress.lock() {
                        *lock = Some((message, progress));
                    }
                    repaint_requested = true;
                }
                crate::dict::DictEvent::FreqSyncFinished(result) => {
                    if let Ok(mut lock) = self.dict.freq_sync_progress.lock() {
                        *lock = None;
                    }
                    match result {
                        Ok(meta) => match self.dict.reload_freq_db(Some(meta.clone())) {
                            Ok(meta) => {
                                self.info(format!(
                                    "Jiten frequency dictionary updated to {}!",
                                    meta.revision
                                ));
                            }
                            Err(e) => {
                                self.error(e.to_string());
                            }
                        },
                        Err(e) => {
                            self.error(format!("Frequency dictionary sync failed: {e}"));
                        }
                    }
                    repaint_requested = true;
                }
                crate::dict::DictEvent::PitchImportProgress { message, progress } => {
                    if let Ok(mut lock) = self.dict.pitch_import_progress.lock() {
                        *lock = Some((message, progress));
                    }
                    repaint_requested = true;
                }
                crate::dict::DictEvent::PitchImportFinished(result) => {
                    if let Ok(mut lock) = self.dict.pitch_import_progress.lock() {
                        *lock = None;
                    }
                    match result {
                        Ok(meta) => match self.dict.reload_pitch_db(Some(meta.clone())) {
                            Ok(meta) => {
                                self.dict.pitch_import_error = None;
                                if let Ok(guard) = self.dict.pitch_db.lock() {
                                    if let Some(ref db) = *guard {
                                        let backfilled = self.bank.backfill_pitches(db);
                                        if backfilled > 0 {
                                            tracing::info!(
                                                "Backfilled pitch accents for {} mined words",
                                                backfilled
                                            );
                                        }
                                    }
                                }
                                self.info(format!(
                                    "Imported pitch dictionary '{}' ({} entries)!",
                                    meta.title, meta.total_entries
                                ));
                            }
                            Err(e) => {
                                self.dict.pitch_import_error = Some(format!("{e:#}"));
                                self.error(e.to_string());
                            }
                        },
                        Err(e) => {
                            self.dict.pitch_import_error = Some(e.clone());
                            self.error(format!("Pitch dictionary import failed: {e}"));
                        }
                    }
                    repaint_requested = true;
                }
            }
        }

        for message in self.bank.poll() {
            match message {
                Ok(info) => self.info(info),
                Err(err) => self.error(err),
            }
            repaint_requested = true;
        }
        for event in self.screenshot.poll_events() {
            match event {
                crate::screenshot::ScreenshotEvent::Copied(
                    crate::screenshot::ScreenshotMode::Normal,
                ) => {
                    self.info("Copied screenshot to clipboard");
                }
                crate::screenshot::ScreenshotEvent::Copied(
                    crate::screenshot::ScreenshotMode::PhonePhoto,
                ) => {
                    self.info("Copied smartphone photo to clipboard");
                }
                crate::screenshot::ScreenshotEvent::Failed(_, err) => {
                    self.error(format!("Screenshot failed: {err}"));
                }
            }
            repaint_requested = true;
        }
        for mined in self.bank.take_newly_mined() {
            self.maybe_share_mined_word_to_twitch(&mined);
        }
        if self.bank.is_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if self.steam_deck.is_none() {
            ui::bank::draw_bank_window(self, ctx);
            repaint_requested |= ui::studio::draw_studio_window(self, ctx);

            self.twitch.studio_active = self.ui.studio_window_open;
            repaint_requested |= self.twitch.poll(ctx);
            for (request_id, clip_url) in self.twitch.take_ready_mined_clips() {
                if let Err(e) = self.bank.attach_twitch_clip(request_id, &clip_url) {
                    tracing::warn!("Mining bank: failed to attach Twitch clip: {e}");
                    self.error(format!("Failed to attach Twitch clip: {e}"));
                }
            }
            for (is_error, message) in self.twitch.take_notifications() {
                if is_error {
                    self.error(message);
                } else {
                    self.info(message);
                }
            }
            ui::twitch_overlay::draw(self, ctx);
        }

        self.update_fps_counters(ctx);
        if self.steam_deck.is_none() {
            ui::debug::draw(self, ctx);
            self.toasts.show(ctx);
            if self.ui.video_window_open
                && CrtFilter::from_u8(self.crt_filter.load(Ordering::Relaxed)) == CrtFilter::Beam
                && self.beam_lockstep.sweep_start_time.is_some()
            {
                let just_after_sub0 = self.beam_lockstep.subframe_index == 0;
                self.stage_source_frames_for_next_sweep(just_after_sub0);
            }
        }

        self.ui.in_egui_frame = false;
        if self.ui.config_save_pending && !self.ui.any_pointer_down {
            config::flush_pending_config(self);
        }

        if repaint_requested {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_readings_survive_counter_reset_and_update_after_a_stall() {
        let mut state = AppState::default();
        let ctx = egui::Context::default();
        state.last_fps_check = Instant::now() - std::time::Duration::from_secs(2);
        state.last_video_fps_check = state.last_fps_check;
        state.frames_since_last_check = 239;
        state.video_frames_since_last_check = 120;
        state.update_fps_counters(&ctx);
        assert!((state.gui_fps - 120.0).abs() < 1.0);
        assert!((state.video_fps - 60.0).abs() < 1.0);
        let measured = (state.gui_fps, state.video_fps);
        state.update_fps_counters(&ctx);
        assert_eq!((state.gui_fps, state.video_fps), measured);
        state.last_video_fps_check = Instant::now() - std::time::Duration::from_secs(2);
        state.update_fps_counters(&ctx);
        assert_eq!(state.video_fps, 0.0);
    }

    #[test]
    fn test_gamescope_fps_calculation_and_title() {
        let mut state = AppState::default();
        let ctx = egui::Context::default();
        let mut hook = crate::gamescope::GamescopeHookState::default();
        hook.is_active = true;
        hook.initial_resolution = (3840, 2160);
        state.gamescope_hook = Some(hook);

        state.last_fps_check = Instant::now() - std::time::Duration::from_secs(1);
        state.last_video_fps_check = state.last_fps_check;
        state.frames_since_last_check = 60;
        state.video_frames_since_last_check = 60;

        state.update_fps_counters(&ctx);

        assert!((state.gui_fps - 60.0).abs() < 1.0);
        assert!((state.video_fps - 60.0).abs() < 1.0);
    }

    #[test]
    fn test_handle_device_scan_result_success() {
        let mut state = AppState::default();
        let result = Ok((
            vec!["/dev/video0".to_string()],
            vec![("default".to_string(), "Default Audio".to_string())],
            vec![("1234:5678".to_string(), "Test USB".to_string())],
        ));

        let success = state.handle_device_scan_result(result);
        assert!(success);
        assert_eq!(state.hardware.video_devices.len(), 1);
        assert_eq!(state.hardware.selected_video_device, "/dev/video0");
        assert_eq!(state.hardware.audio_sources.len(), 1);
        assert_eq!(state.hardware.usb_devices.len(), 1);
    }

    #[test]
    fn test_handle_device_scan_result_error() {
        let mut state = AppState::default();
        let error = anyhow::anyhow!("Test failure");

        let success = state.handle_device_scan_result(Err(error));
        assert!(!success);
        assert_eq!(state.hardware.video_devices.len(), 0);
    }

    #[test]
    fn test_handle_ocr_timeout_pauses_when_dict_popup_open() {
        let mut state = AppState::default();
        let ctx = egui::Context::default();

        state.ocr.timeout_seconds = 45;
        state.ocr.boxes.push(crate::ocr::models::OcrBox {
            text: "日本語".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.1,
            height: 0.1,
            lines: Vec::new(),
            writing_direction: crate::ocr::models::WritingDirection::LeftToRight,
        });
        // Expired scan (50 seconds ago)
        state.ocr.last_scan_time = Some(Instant::now() - std::time::Duration::from_secs(50));
        assert!(state.ocr.is_expired());

        // Simulate dictionary popup being open (e.g. mouse hovering on a word)
        state.dict.popup = Some(crate::dict::models::DictPopupState {
            matched_term: "日本".to_string(),
            source_text: "日本語".to_string(),
            char_range: (0, 2),
            word_rect: egui::Rect::from_min_max(egui::pos2(10.0, 10.0), egui::pos2(50.0, 30.0)),
            extra_word_rects: Vec::new(),
            box_rect: egui::Rect::from_min_max(egui::pos2(5.0, 5.0), egui::pos2(60.0, 35.0)),
            entries: Vec::new(),
            is_popup_hovered: false,
            popup_rect: None,
            last_hover_time: Instant::now(),
            last_word_hover_time: Instant::now(),
            child: None,
        });

        // While a popup is open, handle_ocr_timeout must NOT clear OCR scans regardless of pointer position
        let repainted = state.handle_ocr_timeout(&ctx);
        assert!(!repainted);
        assert_eq!(state.ocr.boxes.len(), 1);
        assert!(state.dict.popup.is_some());

        // When the popup is closed, handle_ocr_timeout clears expired OCR scans
        state.dict.popup = None;
        let repainted = state.handle_ocr_timeout(&ctx);
        assert!(repainted);
        assert!(state.ocr.boxes.is_empty());
    }

    #[test]
    fn test_handle_ocr_timeout_pauses_when_whiteboard_open() {
        let mut state = AppState::default();
        let ctx = egui::Context::default();

        state.ocr.timeout_seconds = 45;
        state.ocr.boxes.push(crate::ocr::models::OcrBox {
            text: "日本語".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.1,
            height: 0.1,
            lines: Vec::new(),
            writing_direction: crate::ocr::models::WritingDirection::LeftToRight,
        });
        state.ocr.last_scan_time = Some(Instant::now() - std::time::Duration::from_secs(50));
        state.whiteboard.open = true;

        // While the whiteboard is open, OCR timeout is stopped and boxes do not disappear
        let repainted = state.handle_ocr_timeout(&ctx);
        assert!(!repainted);
        assert_eq!(state.ocr.boxes.len(), 1);
    }

    #[test]
    fn test_handle_ocr_timeout_not_expired_requests_repaint() {
        let mut state = AppState::default();
        let ctx = egui::Context::default();

        state.ocr.timeout_seconds = 45;
        state.ocr.boxes.push(crate::ocr::models::OcrBox {
            text: "日本語".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.1,
            height: 0.1,
            lines: Vec::new(),
            writing_direction: crate::ocr::models::WritingDirection::LeftToRight,
        });
        // Scan was 10 seconds ago (35s remaining)
        state.ocr.last_scan_time = Some(Instant::now() - std::time::Duration::from_secs(10));
        assert!(!state.ocr.is_expired());

        // Without popup, handle_ocr_timeout does not clear boxes (requests repaint for remaining time)
        let repainted = state.handle_ocr_timeout(&ctx);
        assert!(!repainted);
        assert_eq!(state.ocr.boxes.len(), 1);

        // With popup open, handle_ocr_timeout also does not clear boxes
        state.dict.popup = Some(crate::dict::models::DictPopupState {
            matched_term: "日本".to_string(),
            source_text: "日本語".to_string(),
            char_range: (0, 2),
            word_rect: egui::Rect::from_min_max(egui::pos2(10.0, 10.0), egui::pos2(50.0, 30.0)),
            extra_word_rects: Vec::new(),
            box_rect: egui::Rect::from_min_max(egui::pos2(5.0, 5.0), egui::pos2(60.0, 35.0)),
            entries: Vec::new(),
            is_popup_hovered: false,
            popup_rect: None,
            last_hover_time: Instant::now(),
            last_word_hover_time: Instant::now(),
            child: None,
        });
        let repainted = state.handle_ocr_timeout(&ctx);
        assert!(!repainted);
        assert_eq!(state.ocr.boxes.len(), 1);
    }

    #[test]
    fn test_app_state_clear_color_is_opaque_black() {
        let state = AppState::default();
        let visuals = egui::Visuals::default();
        let color = eframe::App::clear_color(&state, &visuals);
        assert_eq!(color, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn test_studio_window_toggle() {
        let mut state = AppState::default();
        assert!(!state.ui.studio_window_open);
        state.ui.studio_window_open = !state.ui.studio_window_open;
        assert!(state.ui.studio_window_open);
        state.ui.studio_window_open = !state.ui.studio_window_open;
        assert!(!state.ui.studio_window_open);
    }

    #[test]
    fn test_maybe_share_mined_word_to_twitch_conditions() {
        let mut state = AppState::default();
        let entry = crate::bank::BankEntryMeta {
            id: 1,
            created_at: 100,
            term: "飽きる".into(),
            reading: "あきる".into(),
            definition_text: "to get tired of".into(),
            definition_json: serde_json::json!({
                "term": "飽きる",
                "reading": "あきる",
                "glossary": ["to get tired of / to tire of"]
            })
            .to_string(),
            sentence: "それは、いつまで見ていても飽きることがない。".into(),
            word_range: (14, 17),
            has_screenshot: false,
            tag: None,
            twitch_clip_url: None,
        };

        // 1. Disabled toggle -> does nothing
        state.twitch.config.share_mined_words_to_chat = false;
        state.twitch.stream_status = crate::twitch::StreamStatus::Live {
            viewer_count: Some(5),
            broadcaster_id: None,
        };
        state.maybe_share_mined_word_to_twitch(&entry);
        assert_eq!(state.twitch.messages.len(), 0);

        // 2. Enabled toggle but stream offline -> does nothing
        state.twitch.config.share_mined_words_to_chat = true;
        state.twitch.stream_status = crate::twitch::StreamStatus::Offline;
        state.maybe_share_mined_word_to_twitch(&entry);
        assert_eq!(state.twitch.messages.len(), 0);

        // 3. Enabled toggle, stream unknown -> does nothing
        state.twitch.stream_status = crate::twitch::StreamStatus::Unknown("checking".into());
        state.maybe_share_mined_word_to_twitch(&entry);
        assert_eq!(state.twitch.messages.len(), 0);

        // 4. Enabled toggle, stream is live, but not connected/authenticated -> does nothing
        state.twitch.stream_status = crate::twitch::StreamStatus::Live {
            viewer_count: Some(5),
            broadcaster_id: None,
        };
        assert!(!state.twitch.can_send());
        state.maybe_share_mined_word_to_twitch(&entry);
        assert_eq!(state.twitch.messages.len(), 0);
    }

    #[test]
    fn test_gui_frame_rate_limiter() {
        let mut state = AppState::default();
        state.ui.video_window_open = true;

        let start = Instant::now();
        state.pace_gui_frame();
        state.pace_gui_frame();
        let elapsed = start.elapsed();
        // Pacing must be non-blocking with zero artificial thread::sleep overhead
        assert!(elapsed < std::time::Duration::from_millis(5));
        assert!(state.next_gui_frame_time.is_some());

        // When video window is closed, pacing is disabled
        state.ui.video_window_open = false;
        state.pace_gui_frame();
        assert!(state.next_gui_frame_time.is_none());
    }

    #[test]
    fn test_shift_tab_focus_and_hotkeys() {
        let ctx = egui::Context::default();

        // Frame 1: Shift+Tab is pressed
        let mut raw_input = egui::RawInput::default();
        raw_input.focused = true;
        raw_input.modifiers.shift = true;
        raw_input.events.push(egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: Some(egui::Key::Tab),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        });

        let _ = ctx.run(raw_input, |ctx| {
            let consumed = ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab));
            assert!(consumed, "Shift+Tab must be consumed");
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            // Simulate CentralPanel with a response
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.allocate_response(egui::vec2(100.0, 100.0), egui::Sense::click());
            });
        });

        assert!(!ctx.wants_keyboard_input());
        assert_eq!(ctx.memory(|m| m.focused()), None);

        // Frame 2: User presses 'C' with no modifiers
        let mut raw_input2 = egui::RawInput::default();
        raw_input2.focused = true;
        raw_input2.events.push(egui::Event::Key {
            key: egui::Key::C,
            physical_key: Some(egui::Key::C),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });

        let mut c_pressed = false;
        let mut wants_kbd = false;
        let _ = ctx.run(raw_input2, |ctx| {
            wants_kbd = ctx.wants_keyboard_input();
            c_pressed = ctx.input(|i| i.modifiers.is_none() && i.key_pressed(egui::Key::C));
        });

        assert!(!wants_kbd);
        assert!(c_pressed);
    }

    #[test]
    fn test_handle_constant_ocr_scanning_and_pause_conditions() {
        let ctx = egui::Context::default();
        let mut state = AppState::default();
        state.ui.video_window_open = true;
        state.ocr.set_backend(crate::ocr::OcrBackend::LocalScreenAi);
        state.ocr.constant_scanning = true;
        state.ocr.scan_interval_secs = 0.5;
        state.ocr.screen_ai_installed_metadata = Some(crate::ocr::screen_ai::ScreenAiMetadata {
            version: "148.1".to_string(),
            instance_digest: "test".to_string(),
        });

        // 1. Triggers immediate capture when ready
        let triggered = state.handle_constant_ocr_scanning(&ctx);
        assert!(triggered);
        assert!(state.ocr.capture_requested.load(Ordering::Acquire));

        // 2. Does not re-trigger while capture_requested or is_processing is true
        assert!(!state.handle_constant_ocr_scanning(&ctx));
        state.ocr.capture_requested.store(false, Ordering::Release);
        state.ocr.is_processing.store(true, Ordering::Release);
        assert!(!state.handle_constant_ocr_scanning(&ctx));
        state.ocr.is_processing.store(false, Ordering::Release);

        // 3. Pauses when hovering over a visible OCR box, dictionary popup is open, or whiteboard is open
        state.ocr.hide_overlay = false;
        state.ocr.is_box_hovered = true;
        assert!(!state.handle_constant_ocr_scanning(&ctx));
        assert!(state.ocr.was_constant_scan_paused);
        assert!(!state.ocr.capture_requested.load(Ordering::Acquire));

        // 4. Unhovering schedules next scan after base interval rather than firing on the exact unhover frame
        state.ocr.is_box_hovered = false;
        assert!(!state.handle_constant_ocr_scanning(&ctx));
        assert!(!state.ocr.was_constant_scan_paused);
        assert!(state.ocr.next_constant_scan_time.is_some());

        // 5. When hide_overlay is enabled, hovering an invisible OCR box does NOT pause scanning
        state.ocr.next_constant_scan_time = None;
        state.ocr.hide_overlay = true;
        state.ocr.is_box_hovered = true;
        assert!(state.handle_constant_ocr_scanning(&ctx));
        assert!(!state.ocr.was_constant_scan_paused);
        assert!(state.ocr.capture_requested.load(Ordering::Acquire));
        state.ocr.capture_requested.store(false, Ordering::Release);

        // 6. Remote model never triggers constant scanning
        state.ocr.set_backend(crate::ocr::OcrBackend::GoogleLens);
        assert!(!state.handle_constant_ocr_scanning(&ctx));
        assert!(state.ocr.next_constant_scan_time.is_none());
    }

    #[test]
    fn changing_crt_geometry_rejects_old_ocr_and_reschedules_pending_capture() {
        use crate::video::gpu::geometry::OcrProjection;
        let mut state = AppState::default();
        state.update_ocr_projection(OcrProjection::default());
        let generation = state.ocr.current_generation();
        state.ocr.active_request_id.store(7, Ordering::Release);
        state.ocr.is_processing.store(true, Ordering::Release);
        state.update_ocr_projection(OcrProjection::lottes(0.031, 0.041));
        assert!(state.ocr.current_generation() > generation);
        assert!(state.ocr.capture_requested.load(Ordering::Acquire));
        let stale = crate::ocr::OcrWorkerResult {
            request_id: 7,
            generation,
            backend: crate::ocr::OcrBackend::GoogleLens,
            constant_scan: false,
            result: Ok(vec![]),
        };
        assert!(!state.ocr.accept_worker_result(&stale));
        let generation = state.ocr.current_generation();
        state.update_ocr_projection(OcrProjection::lottes(0.031, 0.041));
        assert_eq!(state.ocr.current_generation(), generation);
        state.update_ocr_projection(OcrProjection::halo(true, 95.0));
        assert!(state.ocr.current_generation() > generation);
    }

    #[test]
    fn test_f23_stale_ocr_results_do_not_repopulate_after_clear_or_stop_stream() {
        let ctx = egui::Context::default();
        let mut state = AppState::default();
        let gen1 = state.ocr.current_generation();
        state.ocr.active_request_id.store(10, Ordering::Release);
        state.ocr.is_processing.store(true, Ordering::Release);

        // User clears OCR (or stops stream) while request 10 is in flight
        state.clear_ocr();
        let gen2 = state.ocr.current_generation();
        assert!(gen2 > gen1);
        assert!(!state.ocr.is_processing.load(Ordering::Acquire));

        // Late success from generation 1 arrives: must be ignored
        let stale_ok = crate::ocr::OcrWorkerResult {
            request_id: 10,
            generation: gen1,
            backend: crate::ocr::OcrBackend::GoogleLens,
            constant_scan: false,
            result: Ok(vec![crate::ocr::ParsedLine {
                text: "古いテキスト".to_string(),
                center_x: 0.5,
                center_y: 0.5,
                width: 0.2,
                height: 0.1,
                paragraph_idx: 0,
                writing_direction: crate::ocr::WritingDirection::LeftToRight,
                char_spans: Vec::new(),
            }]),
        };
        assert!(!state.handle_ocr_worker_result(stale_ok, &ctx));
        assert!(state.ocr.boxes.is_empty());
        assert!(state.ocr.raw_lines.is_empty());

        // Late error from generation 1 arrives: must also be ignored
        let stale_err = crate::ocr::OcrWorkerResult {
            request_id: 10,
            generation: gen1,
            backend: crate::ocr::OcrBackend::GoogleLens,
            constant_scan: false,
            result: Err("stale network error".to_string()),
        };
        assert!(!state.handle_ocr_worker_result(stale_err, &ctx));
        assert!(state.ocr.last_error.is_none());

        // Current generation result is applied normally
        state.ocr.active_request_id.store(11, Ordering::Release);
        state.ocr.is_processing.store(true, Ordering::Release);
        let fresh_ok = crate::ocr::OcrWorkerResult {
            request_id: 11,
            generation: gen2,
            backend: crate::ocr::OcrBackend::GoogleLens,
            constant_scan: false,
            result: Ok(vec![crate::ocr::ParsedLine {
                text: "新しいテキスト".to_string(),
                center_x: 0.5,
                center_y: 0.5,
                width: 0.2,
                height: 0.1,
                paragraph_idx: 0,
                writing_direction: crate::ocr::WritingDirection::LeftToRight,
                char_spans: Vec::new(),
            }]),
        };
        assert!(state.handle_ocr_worker_result(fresh_ok, &ctx));
        assert_eq!(state.ocr.boxes.len(), 1);
        assert_eq!(state.ocr.boxes[0].text, "新しいテキスト");
        assert!(!state.ocr.is_processing.load(Ordering::Acquire));
    }

    #[test]
    fn test_beam_lockstep_59_94fps_and_29_97fps_synchronization() {
        let mut state = AppState::default();
        state.ui.video_window_open = true;
        state
            .crt_filter
            .store(CrtFilter::Beam as u8, Ordering::Relaxed);
        state.beam.frames_per_hz = 2.0;
        state.beam.lcd_anti_retention = false;

        let (tx, rx) = crossbeam_channel::unbounded();
        state.frame_receiver = Some(rx);

        let make_frame = |pts: i64| -> Arc<video::types::RawFrame> {
            Arc::new(video::types::RawFrame {
                captured_at: pts,
                rate: crate::replay::config::Rate {
                    num: 60000,
                    den: 1001,
                },
                width: 64,
                height: 64,
                data: vec![0u8; 64 * 64 * 4],
                format: ffmpeg_next::format::Pixel::RGBA,
                color_range: crate::video::types::ColorRange::Full,
            })
        };

        // Send Frame 1 before Sweep 0 starts
        tx.send(make_frame(1)).unwrap();
        state.pace_gui_frame();
        assert_eq!(state.beam_lockstep.subframe_index, 0);
        assert_eq!(state.beam_lockstep.current_i_frame, 0);
        assert_eq!(state.latest_frame.as_ref().map(|f| f.captured_at), Some(1));

        // Send Frame 2 while Sweep 0 is in progress: Sub-frame 1 MUST NOT pull Frame 2 mid-sweep!
        tx.send(make_frame(2)).unwrap();
        // Fast-forward sweep_start_time so pace_gui_frame doesn't sleep in unit test
        state.beam_lockstep.sweep_start_time =
            Some(Instant::now() - std::time::Duration::from_millis(9));
        state.pace_gui_frame();
        assert_eq!(state.beam_lockstep.subframe_index, 1);
        assert_eq!(state.beam_lockstep.current_i_frame, 1);
        assert_eq!(
            state.latest_frame.as_ref().map(|f| f.captured_at),
            Some(1),
            "Sub-frame 1 must not switch source textures mid-sweep!"
        );
        assert_eq!(
            state
                .beam_lockstep
                .staged_video_frame
                .as_ref()
                .map(|f| f.captured_at),
            Some(2),
            "Sub-frame 1 must drain bounded(1) channel into staged_video_frame to prevent queue drops!"
        );

        // Next call wraps to Sweep 1, Sub-frame 0 (`iFrame = 2`) and latches staged Frame 2!
        state.beam_lockstep.sweep_start_time =
            Some(Instant::now() - std::time::Duration::from_millis(17));
        state.pace_gui_frame();
        assert_eq!(state.beam_lockstep.subframe_index, 0);
        assert_eq!(state.beam_lockstep.current_i_frame, 2);
        assert_eq!(
            state.latest_frame.as_ref().map(|f| f.captured_at),
            Some(2),
            "Sub-frame 0 of Sweep 1 must latch Frame 2!"
        );

        // Verify EMA convergence for 59.94 FPS (16,683,333 ns) and 29.97 FPS (33,366,667 ns across 2 sweeps)
        let mut lockstep = crate::app::models::BeamLockstepState::default();
        let t0 = Instant::now();
        let ntsc_60 = std::time::Duration::from_nanos(16_683_333);
        lockstep.record_source_frame(0, t0, false);
        for i in 1..=10 {
            lockstep.sweeps_since_live_arrival = 1;
            lockstep.record_source_frame(0, t0 + ntsc_60 * i, false);
        }
        assert!(lockstep.has_measured_interval);
        assert!(lockstep.phase_locked);
        let diff_ns = (lockstep.smoothed_sweep_duration.as_nanos() as i128
            - ntsc_60.as_nanos() as i128)
            .abs();
        assert!(
            diff_ns < 5_000,
            "Expected ~16.683ms, got {:?}",
            lockstep.smoothed_sweep_duration
        );

        // 29.97 FPS (1 live frame every 2 sweeps)
        lockstep.sweeps_since_live_arrival = 2;
        lockstep.record_source_frame(0, t0 + ntsc_60 * 12, false);
        let diff_30_ns = (lockstep.smoothed_sweep_duration.as_nanos() as i128
            - ntsc_60.as_nanos() as i128)
            .abs();
        assert!(diff_30_ns < 5_000);
    }

    #[test]
    fn test_beam_lockstep_framerate_drops_and_vrr_lfc_multiples() {
        let mut lockstep = crate::app::models::BeamLockstepState::default();
        lockstep.last_effective_fph = 2.0;
        lockstep.last_target_hz = 144;

        let t0 = Instant::now();
        let mut ts_us = 1_000_000_i64;
        let mut cur_inst = t0;

        // 1. Start at 60 FPS (16,667 us) with variable CPU sws_scale arrival jitter (+1.5ms / -1.5ms)
        lockstep.record_source_frame(ts_us, cur_inst, false);
        for i in 0..4 {
            ts_us += 16_667;
            cur_inst += std::time::Duration::from_micros(16_667);
            let jitter_us = if i % 2 == 0 { 1_500_i64 } else { -1_500_i64 };
            let pop_inst = if jitter_us >= 0 {
                cur_inst + std::time::Duration::from_micros(jitter_us as u64)
            } else {
                cur_inst - std::time::Duration::from_micros((-jitter_us) as u64)
            };
            lockstep.record_source_frame(ts_us, pop_inst, false);
            let committed = lockstep.commit_next_sweep_duration(pop_inst, true, true);
            lockstep.sweep_counter += 1;
            assert!(
                (committed.as_micros() as i64 - 16_667).abs() < 30,
                "Expected exact 16.667ms (60.0 FPS / 120.0 FPS UI) despite sws_scale arrival jitter, got {:?}",
                committed
            );
        }
        assert_eq!(lockstep.sweeps_per_frame, 1);
        assert!(
            (lockstep.smoothed_sweep_duration.as_micros() as i64 - 16_667).abs() < 50,
            "Expected ~16.67ms at 60 FPS, got {:?}",
            lockstep.smoothed_sweep_duration
        );

        // 2. Drop framerate to 48 FPS (20,833 us)
        for _ in 0..5 {
            ts_us += 20_833;
            cur_inst += std::time::Duration::from_micros(20_833);
            lockstep.record_source_frame(ts_us, cur_inst, false);
            let committed = lockstep.commit_next_sweep_duration(
                cur_inst + std::time::Duration::from_micros(900),
                true,
                true,
            );
            lockstep.sweep_counter += 1;
            // Every sub-frame at 2x must stay above the 144Hz ceiling (>= 6.944ms)
            let subframe_dur = committed / 2;
            assert!(
                subframe_dur >= std::time::Duration::from_nanos(6_944_444),
                "Sub-frame duration {:?} violated 144Hz VRR ceiling!",
                subframe_dur
            );
        }
        assert_eq!(lockstep.sweeps_per_frame, 1);
        assert!(
            (lockstep.smoothed_sweep_duration.as_micros() as i64 - 20_833).abs() < 200,
            "Expected ~20.83ms at 48 FPS (96Hz VRR 2x lockstep), got {:?}",
            lockstep.smoothed_sweep_duration
        );

        // 3. Drop framerate to 40 FPS (25,000 us) -> 80Hz VRR 2x lockstep (12.5ms per sub-frame)
        for _ in 0..5 {
            ts_us += 25_000;
            cur_inst += std::time::Duration::from_micros(25_000);
            lockstep.record_source_frame(ts_us, cur_inst, false);
        }
        assert_eq!(lockstep.sweeps_per_frame, 1);
        assert!(
            (lockstep.smoothed_sweep_duration.as_micros() as i64 - 25_000).abs() < 250,
            "Expected ~25.0ms at 40 FPS (80Hz VRR 2x lockstep), got {:?}",
            lockstep.smoothed_sweep_duration
        );

        // 4. Drop framerate to 30 FPS (33,333 us) -> LFC 2 sweeps per frame (60Hz CRT sweep, 120Hz VRR 4x lockstep)
        for _ in 0..5 {
            ts_us += 33_333;
            cur_inst += std::time::Duration::from_micros(33_333);
            lockstep.record_source_frame(ts_us, cur_inst, false);
        }
        assert_eq!(
            lockstep.sweeps_per_frame, 2,
            "30 FPS should engage 2x LFC sweeps per source frame (120Hz VRR)"
        );
        assert!(
            (lockstep.smoothed_sweep_duration.as_micros() as i64 - 16_667).abs() < 250,
            "Expected ~16.67ms per LFC sweep at 30 FPS, got {:?}",
            lockstep.smoothed_sweep_duration
        );

        // 5. Simulate disabling Beam (e.g. pressing 'C') and re-enabling it later:
        // `deactivate()` must advance `sweep_counter` so `BeamRenderer` never sees a duplicate
        // `crt_hz_counter` on re-enable, and clear all stale timestamps.
        let prev_sweep_counter = lockstep.sweep_counter;
        lockstep.sweep_start_time = Some(cur_inst);
        lockstep.last_present_time = Some(cur_inst);
        lockstep.deactivate();
        assert_eq!(lockstep.sweep_start_time, None);
        assert_eq!(lockstep.last_present_time, None);
        assert_eq!(
            lockstep.sweep_counter,
            (prev_sweep_counter + 1) % 4_000_000,
            "deactivate() must advance sweep_counter so BeamRenderer never sees a duplicate crt_hz_counter"
        );

        // 6. Simulate unstable / jittery frame times (16.7ms, 22.0ms, 18.5ms, 24.0ms, 17.0ms)
        // Verify sweep-to-sweep slew rate is bounded and jitter cushion expands smoothly.
        let unstable_dts_us = [16_700_i64, 22_000, 18_500, 24_000, 17_000];
        let mut prev_committed = lockstep.current_sweep_duration;
        for dt_us in unstable_dts_us {
            ts_us += dt_us;
            cur_inst += std::time::Duration::from_micros(dt_us as u64);
            lockstep.record_source_frame(ts_us, cur_inst, false);
            let committed = lockstep.commit_next_sweep_duration(
                cur_inst + std::time::Duration::from_micros(1200),
                true,
                true,
            );
            lockstep.sweep_counter += 1;
            let step_diff_us =
                (committed.as_micros() as i64 - prev_committed.as_micros() as i64).abs();
            assert!(
                step_diff_us <= 1_650,
                "Sweep-to-sweep step {}us exceeded max slew limit!",
                step_diff_us
            );
            assert!(
                committed / 2 >= std::time::Duration::from_nanos(6_944_444),
                "Sub-frame duration {:?} violated 144Hz VRR ceiling!",
                committed / 2
            );
            prev_committed = committed;
        }
        assert!(
            lockstep.frame_jitter_ema > std::time::Duration::from_micros(200),
            "Jitter EMA should detect unstable frame intervals: {:?}",
            lockstep.frame_jitter_ema
        );
    }

    #[test]
    fn test_beam_disable_and_reenable_never_locks_up() {
        for fph in [2.0_f32, 2.4_f32] {
            let mut state = AppState::default();
            state.ui.video_window_open = true;
            state.beam.frames_per_hz = fph;
            state.beam.target_hz = 100;
            state.beam.lcd_anti_retention = true;
            state.beam.lcd_inversion_compensation_slew = 0.001;

            // 1. Enable Beam
            state
                .crt_filter
                .store(CrtFilter::Beam as u8, Ordering::Relaxed);
            state.pace_gui_frame();
            if (fph - 2.0).abs() < 1e-4 {
                assert!(
                    state.beam_lockstep.sweep_start_time.is_some(),
                    "2.0 frames_per_hz with lcd_anti_retention=true must still engage integer lockstep"
                );
            }

            // 2. Disable Beam (e.g. pressing 'C') and run 600 UI frames while CRT is Off
            state
                .crt_filter
                .store(CrtFilter::Off as u8, Ordering::Relaxed);
            for _ in 0..600 {
                state.pace_gui_frame();
            }

            // 3. Re-enable Beam and verify pace_gui_frame completes instantaneously without sleeping on stale timestamps
            state
                .crt_filter
                .store(CrtFilter::Beam as u8, Ordering::Relaxed);
            let t0 = Instant::now();
            state.pace_gui_frame();
            let elapsed = t0.elapsed();
            assert!(
                elapsed < std::time::Duration::from_millis(5),
                "Re-enabling Beam (fph={}) locked up pace_gui_frame for {:?}!",
                fph,
                elapsed
            );
        }
    }
}
