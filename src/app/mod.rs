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
            cathode_interference: crate::app::models::CathodeInterferenceSettings::default(),
            cathode_interference_defaults: crate::app::models::CathodeInterferenceSettings::default(),
            selected_crt_filter: CrtFilter::Lottes,
            video: VideoSettings {
                pixelate_filter_enabled: false,
                use_magenta_background: false,
                retro_pc_frame: false,
                retro_pc_frame_dark_mode: false,
                retro_pc_ambient_glow: 0.55,
                retro_software_mouse: false,
                lights_off_night_mode: false,
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
            previous_normal_profile: None,
            previous_gamescope_normal_profile: None,
            previous_capture_resolution: (0, 0),
            previous_capture_framerate: 0,
            gamescope_hook: None,
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
            config_load_error: None,
            config_quarantine_path: None,
        }
    }
}

impl AppState {
    pub fn handle_device_scan_result(&mut self, result: devices::DeviceScanResult) -> bool {
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

                if let Ok(cfg) = confy::load::<config::MichadameConfig>("michadame", None) {
                    config::apply_config(self, &cfg);
                }
                self.sync_mic_stream();
                self.sync_webcam_stream(None);
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

        let term_entry = crate::bank::entry_from_json(&entry.term, &entry.reading, &entry.definition_json);
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

    /// Checks OCR timeout and clears expired scans when no dictionary popup is open.
    ///
    /// If a dictionary popup is open (mouse hovering on a word or inside the popup),
    /// the OCR expiration timer is paused so scans aren't cleared out from under the user.
    /// Once the popup is dismissed, the expiration timer resumes/applies.
    pub fn handle_ocr_timeout(&mut self, ctx: &egui::Context) -> bool {
        let mut repaint_requested = false;
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

    pub fn update_fps_counters(&mut self, ctx: &egui::Context) {
        self.frames_since_last_check += 1;
        let now = Instant::now();
        let elapsed_secs = (now - self.last_fps_check).as_secs_f32();

        if elapsed_secs >= 1.0 {
            self.gui_fps = self.frames_since_last_check as f32 / elapsed_secs;
            self.last_fps_check = now;
            self.frames_since_last_check = 0;
        }

        let video_elapsed_secs = (now - self.last_video_fps_check).as_secs_f32();
        if video_elapsed_secs >= 1.0 {
            self.video_fps = self.video_frames_since_last_check as f32 / video_elapsed_secs;
            self.last_video_fps_check = now;
            self.video_frames_since_last_check = 0;
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
        } else {
            format!(
                "Michadame Viewer | UI: {:.1} FPS | Video: {:.1} FPS | Audio Latency: {} ms",
                self.gui_fps, self.video_fps, audio_latency
            )
        };

        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.toasts.add(egui_toast::Toast {
            kind: egui_toast::ToastKind::Info,
            options: egui_toast::ToastOptions::default()
                .duration(std::time::Duration::from_secs(3)),
            text: text.into().into(),
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.toasts.add(egui_toast::Toast {
            kind: egui_toast::ToastKind::Error,
            options: egui_toast::ToastOptions::default()
                .duration(std::time::Duration::from_secs(3)),
            text: text.into().into(),
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

    /// Updates the target time for the next 60 FPS presentation interval without blocking.
    ///
    /// Presentation timing is driven directly by frame arrivals (e.g. PipeWire DMA-BUF delivery)
    /// rather than artificial thread sleep loops, eliminating scheduler jitter and microstutters.
    pub fn pace_gui_frame(&mut self) {
        if self.ui.video_window_open {
            const TARGET_FRAME_DURATION: std::time::Duration =
                std::time::Duration::from_nanos(1_000_000_000 / 60);

            let now = Instant::now();
            if let Some(ref mut next_time) = self.next_gui_frame_time {
                let post_now = Instant::now();
                if post_now > *next_time + TARGET_FRAME_DURATION * 2 {
                    *next_time = post_now + TARGET_FRAME_DURATION;
                } else {
                    *next_time += TARGET_FRAME_DURATION;
                }
            } else {
                self.next_gui_frame_time = Some(now + TARGET_FRAME_DURATION);
            }
        } else {
            self.next_gui_frame_time = None;
        }
    }
}

impl eframe::App for AppState {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
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
        self.pace_gui_frame();

        let raw_audio_peak = self
            .hardware
            .audio_peak_amplitude
            .swap(0, Ordering::Relaxed) as f32
            / 1000.0;
        self.hardware.live_audio_peak = raw_audio_peak.max(self.hardware.live_audio_peak * 0.88);

        let mic_db = f32::from_bits(self.mic.current_level_db.load(Ordering::Relaxed));
        let raw_mic = if self.mic.config.enabled && self.mic.active_stream.is_some() && mic_db > -80.0 {
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

        // If Gamescope hook is active and passthrough is paused (michadame control mode),
        // ensure any widget focus captured by Shift+Tab tab-navigation is cleared so hotkeys work,
        // unless a dialog or controls window is open for explicit user text input.
        if self
            .gamescope_hook
            .as_ref()
            .map_or(false, |h| h.is_active && !h.passthrough_enabled)
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

        if !gamescope_passthrough {
            self.replay.shortcuts(ctx);
        }
        let mut repaint_requested = false;

        if self.ui.control_window_open {
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("control_window"),
                egui::ViewportBuilder::default()
                    .with_title("Michadame Controls")
                    .with_inner_size([960.0, 770.0])
                    .with_min_inner_size([960.0, 770.0])
                    .with_max_inner_size([960.0, 770.0])
                    .with_resizable(false),
                |ctx, class| {
                    assert!(
                        class == egui::ViewportClass::Immediate,
                        "This egui backend doesn't support multiple viewports"
                    );

                    repaint_requested |= ui::draw_main_ui(self, ctx);

                    if ctx.input(|i| i.viewport().close_requested()) {
                        self.ui.control_window_open = false;
                    }
                },
            );
        }

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

        if let Some(count) = self.fullscreen_toggle_frame_count {
            match count {
                0 => {
                    self.fullscreen_toggle_frame_count = Some(1);
                }
                1 => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                    self.fullscreen_toggle_frame_count = Some(2);
                }
                2 => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
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

        if self
            .gamescope_hook
            .as_ref()
            .map_or(false, |h| h.is_active && !h.passthrough_enabled)
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

        // Video-window hotkeys must not fire while typing or while Gamescope passthrough is active.
        let typing = ctx.wants_keyboard_input();
        if !gamescope_passthrough {
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
                config::save_config(self);
                self.info(format!("CRT filter set to: {}", next_filter));
                ctx.request_repaint();
            }
            if !ctx.wants_keyboard_input()
                && ctx.input(|i| i.modifiers.is_none() && i.key_pressed(egui::Key::K))
            {
                let current_filter = CrtFilter::from_u8(self.crt_filter.load(Ordering::Relaxed));
                if current_filter != CrtFilter::Off {
                    self.video.lights_off_night_mode = !self.video.lights_off_night_mode;
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
            if !ctx.wants_keyboard_input()
                && ctx.input(|i| i.modifiers.is_none() && i.key_pressed(egui::Key::B))
            {
                self.bank.window_open = !self.bank.window_open;
                if !self.bank.window_open {
                    self.bank.enlarged = None;
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
            if self.ui.video_window_open && !self.ui.show_quit_dialog {
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

        let dropped_file = ctx.input(|i| {
            i.raw
                .dropped_files
                .first()
                .and_then(|f| f.path.clone())
        });
        if let Some(path) = dropped_file {
            self.start_video_file(&path, ctx);
            repaint_requested = true;
        }

        if let Some(rx) = &self.device_scan_receiver {
            if let Ok(scan_result) = rx.try_recv() {
                repaint_requested |= self.handle_device_scan_result(scan_result);
            } else {
                repaint_requested = true;
            }
        }

        repaint_requested |= self.handle_video_thread_events(ctx);
        repaint_requested |= self.handle_audio_thread_events(ctx);
        repaint_requested |= self.handle_video_file_player_events(ctx);

        if let Some(ref mut p) = self.video_player {
            if !p.is_seeking {
                p.current_position_secs =
                    p.position_us.load(Ordering::Relaxed) as f64 / 1_000_000.0;
            }
            p.is_paused = p.is_paused_shared.load(Ordering::Relaxed);
            p.is_eof = p.is_eof_shared.load(Ordering::Relaxed);
        }

        if let Some(rx) = &self.frame_receiver {
            while let Ok(frame) = rx.try_recv() {
                // Initialize or resize FFT mask when frame dimensions change
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
                repaint_requested = true;
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

        if let Some(rx) = &self.ocr.result_receiver {
            if let Ok(result) = rx.try_recv() {
                self.ocr.is_processing.store(false, Ordering::Release);
                match result {
                    Ok(lines) => {
                        self.ocr.raw_lines = lines;
                        self.ocr.recompute_boxes();
                        let count = self.ocr.boxes.len();
                        self.ocr.last_copied_index = None;
                        self.ocr.last_error = None;
                        if count == 0 {
                            self.ocr.last_scan_time = None;
                            self.info("Google Lens OCR: No Japanese text detected.");
                        } else {
                            self.ocr.last_scan_time = Some(Instant::now());
                            self.info(format!(
                                "Google Lens OCR: Found {} text box{}.",
                                count,
                                if count == 1 { "" } else { "es" }
                            ));
                        }
                    }
                    Err(e) => {
                        self.ocr.last_error = Some(e.clone());
                        self.error(format!("OCR error: {}", e));
                    }
                }
                repaint_requested = true;
            }
        }

        if !self.ocr.boxes.is_empty() && self.ocr.last_scan_time.is_none() {
            self.ocr.last_scan_time = Some(Instant::now());
        }

        repaint_requested |= self.handle_ocr_timeout(ctx);

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
                    match result {
                        Ok(meta) => {
                            let db_path = self.dict.dict_dir.join("jitendex.db");
                            match crate::dict::DictDatabase::open(&db_path) {
                                Ok(db) => {
                                    if let Ok(mut db_lock) = self.dict.db.lock() {
                                        *db_lock = Some(db);
                                    }
                                }
                                Err(e) => {
                                    self.error(format!("Failed to open synced dictionary: {e}"));
                                }
                            }
                            self.dict.installed_metadata = Some(meta.clone());
                            self.dict.update_available = false;
                            if let Ok(mut lock) = self.dict.sync_progress.lock() {
                                *lock = None;
                            }
                            self.info(format!("Jitendex dictionary updated to {}!", meta.revision));
                        }
                        Err(e) => {
                            if let Ok(mut lock) = self.dict.sync_progress.lock() {
                                *lock = None;
                            }
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
                    match result {
                        Ok(meta) => {
                            let db_path = self.dict.dict_dir.join("jiten_freq.db");
                            match crate::dict::FreqDatabase::open(&db_path) {
                                Ok(db) => {
                                    if let Ok(mut db_lock) = self.dict.freq_db.lock() {
                                        *db_lock = Some(db);
                                    }
                                }
                                Err(e) => {
                                    self.error(format!("Failed to open synced frequency dictionary: {e}"));
                                }
                            }
                            self.dict.installed_freq_metadata = Some(meta.clone());
                            self.dict.freq_update_available = false;
                            if let Ok(mut lock) = self.dict.freq_sync_progress.lock() {
                                *lock = None;
                            }
                            self.info(format!("Jiten frequency dictionary updated to {}!", meta.revision));
                        }
                        Err(e) => {
                            if let Ok(mut lock) = self.dict.freq_sync_progress.lock() {
                                *lock = None;
                            }
                            self.error(format!("Frequency dictionary sync failed: {e}"));
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
        for mined in self.bank.take_newly_mined() {
            self.maybe_share_mined_word_to_twitch(&mined);
        }
        if self.bank.is_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        ui::bank::draw_bank_window(self, ctx);
        repaint_requested |= ui::studio::draw_studio_window(self, ctx);

        self.twitch.studio_active = self.ui.studio_window_open;
        repaint_requested |= self.twitch.poll(ctx);
        for (created_at, clip_url) in self.twitch.take_ready_mined_clips() {
            if let Err(e) = self.bank.attach_twitch_clip(created_at, &clip_url) {
                tracing::warn!("Mining bank: failed to attach Twitch clip: {e}");
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

        self.update_fps_counters(ctx);
        ui::debug::draw(self, ctx);
        self.toasts.show(ctx);

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
            }).to_string(),
            sentence: "それは、いつまで見ていても飽きることがない。".into(),
            word_range: (14, 17),
            has_screenshot: false,
            tag: None,
            twitch_clip_url: None,
        };

        // 1. Disabled toggle -> does nothing
        state.twitch.config.share_mined_words_to_chat = false;
        state.twitch.stream_status = crate::twitch::StreamStatus::Live { viewer_count: Some(5), broadcaster_id: None };
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
        state.twitch.stream_status = crate::twitch::StreamStatus::Live { viewer_count: Some(5), broadcaster_id: None };
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
}
