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
        Self {
            replay: crate::replay::Replay::default(),
            hardware: HardwareState {
                video_queue_drops: Arc::new(AtomicU64::new(0)),
                audio_peak_amplitude: Arc::new(AtomicU64::new(0)),
                audio_latency_ms: Arc::new(AtomicU64::new(0)),
                audio_buffer_size: 1024,
                audio_sample_rate: 48000,
                audio_sample_format: "S16LE".to_string(),
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
                dismissed_config_error: false,
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
                retro_pc_ambient_glow: 0.55,
                crt_glass_enabled: false,
                crt_glass_intensity: 0.25,
                crt_glass_glossiness: 0.50,
                crt_glass_photographer_enabled: false,
                crt_glass_photographer_intensity: 0.50,
                crt_glass_flash_enabled: false,
                crt_glass_flash_intensity: 0.70,
                horizontal_stretch: 1.0,
                median_filter_enabled: false,
                median_mix: 1.0,
                vibrance: 1.0,
                overscan_x: 0.0,
                overscan_y: 0.0,
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
            pending_audio_stream: None,
            device_scan_receiver: None,
            logo_texture: None,
            gui_fps: 0.0,
            video_fps: 0.0,
            last_fps_check: Instant::now(),
            frames_since_last_check: 0,
            last_video_fps_check: Instant::now(),
            video_frames_since_last_check: 0,
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

        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "Michadame Viewer | UI: {:.0} FPS | Video: {:.0} FPS | Audio Latency: {} ms",
            self.gui_fps, self.video_fps, audio_latency
        )));
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
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.replay.update();
        if let Some(message) = self.replay.notification() {
            self.info(message);
        }
        if self.replay.runtime.is_some() || self.replay.status().saving {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        self.ui.is_fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        self.replay.shortcuts(ctx);
        let mut repaint_requested = false;

        if self.ui.control_window_open {
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("control_window"),
                egui::ViewportBuilder::default()
                    .with_title("Michadame Controls")
                    .with_inner_size([900.0, 900.0]),
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

        if ctx.input(|i| i.key_pressed(egui::Key::F)) {
            let is_fullscreen = !ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(is_fullscreen));
        }
        if !ctx.wants_keyboard_input()
            && ctx.input(|i| i.focused && i.modifiers.is_none() && i.key_pressed(egui::Key::C))
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
        if ctx.input(|i| i.key_pressed(egui::Key::G)) {
            self.video.pixelate_filter_enabled = !self.video.pixelate_filter_enabled;
            let status = if self.video.pixelate_filter_enabled {
                "enabled"
            } else {
                "disabled"
            };
            self.info(format!("480p Pixelate filter {}.", status));
            config::save_config(self);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Q))
            && self.ui.video_window_open
            && !self.ui.show_stop_stream_dialog
        {
            self.ui.show_stop_stream_dialog = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::M)) {
            self.ui.control_window_open = !self.ui.control_window_open;
        }
        if !ctx.wants_keyboard_input()
            && ctx.input(|i| i.modifiers.is_none() && i.key_pressed(egui::Key::B))
        {
            self.bank.window_open = !self.bank.window_open;
            if !self.bank.window_open {
                self.bank.enlarged = None;
            }
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            if self.ui.video_window_open && !self.ui.show_quit_dialog {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.ui.show_quit_dialog = true;
            }
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

        if let Some(rx) = &self.frame_receiver {
            if let Ok(frame) = rx.try_recv() {
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
        if self.bank.is_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        ui::bank::draw_bank_window(self, ctx);

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
}
