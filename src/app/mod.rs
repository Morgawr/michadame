pub mod models;
pub mod stream;
pub mod init;

pub use models::*;
pub use init::init_app_state;
use crate::{config, devices, video, ui};
use crate::devices::filter_type::CrtFilter;
use eframe::egui;
use std::collections::BTreeMap;
use std::sync::{Arc, atomic::{AtomicU64, AtomicU8, Ordering}};
use std::time::Instant;

impl Default for AppState {
    fn default() -> Self {
        Self {
            hardware: HardwareState {
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
                is_fullscreen: false,
                reset_usb_on_startup: false,
                show_first_run_dialog: false,
                show_quit_dialog: false,
                show_stop_stream_dialog: false,
                video_window_open: false,
                control_window_open: true,
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
            video: VideoSettings {
                pixelate_filter_enabled: false,
                use_magenta_background: false,
                horizontal_stretch: 1.0,
                median_filter_enabled: false,
                vibrance: 1.0,
                overscan_x: 0.0,
                overscan_y: 0.0,
                fft_filter_enabled: false,
                fft_mask_window_open: false,
            },
            toasts: egui_toast::Toasts::new().anchor(egui::Align2::LEFT_TOP, (10.0, 10.0)).direction(egui::Direction::TopDown),
            video_thread: None,
            video_texture: None,
            frame_receiver: None,
            device_scan_receiver: None,
            logo_texture: None,
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
            fft_mask_resolution: (0, 0),
            fft_brush_radius: 8.0,
            fft_mask_threshold: 0.0,
            fft_black_threshold: 0.0,
            fft_mask_save_name: String::new(),
            fft_available_masks: Vec::new(),
        }
    }
}

impl AppState {
    pub fn handle_device_scan_result(&mut self, result: devices::DeviceScanResult) -> bool {
        let scan_successful = match result {
            Ok((video_devices, audio_sources, usb_devices)) => {
                self.hardware.video_devices = video_devices;
                self.hardware.selected_video_device =
                    self.hardware.video_devices.first().cloned().unwrap_or_default();
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

    pub fn update_fps_counters(&mut self, ctx: &egui::Context) {
        self.frames_since_last_check += 1;
        let now = Instant::now();
        let elapsed_secs = (now - self.last_fps_check).as_secs_f32();

        if elapsed_secs >= 1.0 {
            self.last_fps_check = now;
            self.frames_since_last_check = 0;
        }

        let video_elapsed_secs = (now - self.last_video_fps_check).as_secs_f32();
        if video_elapsed_secs >= 1.0 {
            self.last_video_fps_check = now;
            self.video_frames_since_last_check = 0;
        }

        let gui_fps = if elapsed_secs > 0.0 {
            self.frames_since_last_check as f32 / elapsed_secs
        } else {
            0.0
        };
        let video_fps = if video_elapsed_secs > 0.0 {
            self.video_frames_since_last_check as f32 / video_elapsed_secs
        } else {
            0.0
        };

        let audio_latency = self.hardware.audio_latency_ms.load(Ordering::Relaxed);

        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "Michadame Viewer | UI: {:.0} FPS | Video: {:.0} FPS | Audio Latency: {} ms",
            gui_fps, video_fps, audio_latency
        )));
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.toasts.add(egui_toast::Toast {
            kind: egui_toast::ToastKind::Info,
            options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(3)),
            text: text.into().into(),
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.toasts.add(egui_toast::Toast {
            kind: egui_toast::ToastKind::Error,
            options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(3)),
            text: text.into().into(),
        });
    }
}

impl eframe::App for AppState {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(gl) = _gl {
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
        if ctx.input(|i| i.key_pressed(egui::Key::C)) {
            let current_filter = CrtFilter::from_u8(self.crt_filter.load(Ordering::Relaxed));
            let next_filter = current_filter.next();
            self.crt_filter.store(next_filter as u8, Ordering::Relaxed);
            config::save_config(self);
            self.info(format!("CRT filter set to: {}", next_filter.to_string()));
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
        if ctx.input(|i| i.key_pressed(egui::Key::Q)) {
            if self.ui.video_window_open && !self.ui.show_stop_stream_dialog {
                self.ui.show_stop_stream_dialog = true;
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::M)) {
            self.ui.control_window_open = !self.ui.control_window_open;
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

        if let Some(rx) = &self.frame_receiver {
            if let Ok(frame) = rx.try_recv() {
                // Initialize or resize FFT mask when frame dimensions change
                if self.video.fft_filter_enabled {
                    let (fft_w, fft_h) = crate::video::gpu::FftFilter::fft_dimensions(frame.width, frame.height);
                    if self.fft_mask_resolution != (fft_w, fft_h) {
                        self.fft_mask_resolution = (fft_w, fft_h);
                        self.fft_mask_data = vec![255u8; (fft_w * fft_h) as usize];
                    }
                }
                self.latest_frame = Some(frame);
                self.video_frames_since_last_check += 1;
            }
            repaint_requested = true;
        }

        // Draw FFT mask editor window and handle mask upload
        if self.video.fft_mask_window_open {
            let mask_changed = ui::fft_mask::draw_fft_mask_editor(self, ctx);
            if mask_changed {
                // Upload updated mask to GPU via a paint callback
                let fft_arc = self.fft_filter.clone();
                let mask_data = self.fft_mask_data.clone();
                let mask_res = self.fft_mask_resolution;
                if let Some(fft_arc) = fft_arc {
                    // We need GL context for upload - use a paint callback
                    let callback = egui::PaintCallback {
                        rect: egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0)),
                        callback: std::sync::Arc::new(eframe::egui_glow::CallbackFn::new(
                            move |_info, painter| {
                                let fft = fft_arc.lock().unwrap();
                                fft.upload_mask(painter.gl(), &mask_data, mask_res.0, mask_res.1);
                            },
                        )),
                    };
                    // Register the callback on the main viewport's painter
                    ctx.layer_painter(egui::LayerId::background()).add(callback);
                }
                repaint_requested = true;
            }
        }

        self.update_fps_counters(ctx);
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
}
