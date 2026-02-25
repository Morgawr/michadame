use crate::video::VideoFormat;
use crate::{config, devices, devices::filter_type::CrtFilter, ui, video};
use eframe::egui;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, AtomicU8, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::Instant;


pub struct HardwareState {
    pub audio_peak_amplitude: Arc<AtomicU64>,
    pub video_devices: Vec<String>,
    pub usb_devices: Vec<(String, String)>,
    pub selected_usb_device: Option<String>,
    pub selected_video_device: String,
    pub audio_sources: Vec<(String, String)>,
    pub selected_audio_source_name: Option<String>,
    pub active_audio_stream: Option<devices::audio::AudioStreamHandle>,
    pub supported_formats: Vec<VideoFormat>,
    pub selected_format_index: usize,
    pub selected_resolution: (u32, u32),
    pub selected_framerate: u32,
}

pub struct UiState {
    pub is_fullscreen: bool,
    pub reset_usb_on_startup: bool,
    pub show_first_run_dialog: bool,
    pub show_quit_dialog: bool,
    pub show_stop_stream_dialog: bool,
    pub video_window_open: bool,
    pub control_window_open: bool,
}

pub struct CrtSettings {
    pub hard_scan: f32,
    pub warp_x: f32,
    pub warp_y: f32,
    pub shadow_mask: f32,
    pub brightboost: f32,
    pub hard_bloom_pix: f32,
    pub hard_bloom_scan: f32,
    pub bloom_amount: f32,
    pub shape: f32,
    pub hard_pix: f32,
}

pub struct VideoSettings {
    pub pixelate_filter_enabled: bool,
    pub use_magenta_background: bool,
    pub horizontal_stretch: f32,
    pub median_filter_enabled: bool,
    pub vibrance: f32,
    pub overscan_x: f32,
    pub overscan_y: f32,
}

pub struct AppState {
    pub hardware: HardwareState,
    pub ui: UiState,
    pub crt: CrtSettings,
    pub video: VideoSettings,
    pub toasts: egui_toast::Toasts,

    pub video_thread: Option<JoinHandle<()>>,
    pub video_texture: Option<egui::TextureHandle>,
    pub latest_frame: Option<Arc<video::types::RawFrame>>,
    pub frame_receiver: Option<crossbeam_channel::Receiver<Arc<video::types::RawFrame>>>,
    pub device_scan_receiver: Option<crossbeam_channel::Receiver<devices::DeviceScanResult>>,
    pub logo_texture: Option<egui::TextureHandle>,
    pub last_fps_check: Instant,
    pub frames_since_last_check: u32,
    pub last_video_fps_check: Instant,
    pub video_frames_since_last_check: u32,

    pub crt_filter: Arc<AtomicU8>,
    pub scaler_filter: Arc<AtomicU8>,
    pub color_range: Arc<AtomicU8>,
    pub crt_renderer: Option<Arc<Mutex<video::gpu_filter::CrtFilterRenderer>>>,
    pub fullscreen_toggle_frame_count: Option<u8>,

    pub profiles: HashMap<String, config::Profile>,
    pub active_profile: String,
    pub new_profile_name: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            hardware: HardwareState {
                audio_peak_amplitude: Arc::new(AtomicU64::new(0)),
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
            },
            toasts: egui_toast::Toasts::new().anchor(egui::Align2::RIGHT_BOTTOM, (-10.0, -10.0)).direction(egui::Direction::BottomUp),
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
                let mut p = HashMap::new();
                p.insert("Default".to_string(), config::Profile::default());
                p
            },
            active_profile: "Default".to_string(),
            new_profile_name: String::new(),
        }
    }
}

impl AppState {
    fn handle_device_scan_result(&mut self, result: devices::DeviceScanResult) -> bool {
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
                self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(2)), text: "Devices loaded successfully.".to_string().into() });
                true
            }
            Err(e) => {
                self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(2)), text: format!("Error: {}", e).into() });
                false
            }
        };
        self.device_scan_receiver = None;
        scan_successful
    }

    fn update_fps_counters(&mut self, ctx: &egui::Context) {
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
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "Michadame Viewer | UI: {:.0} FPS | Video: {:.0} FPS",
            gui_fps, video_fps
        )));
    }

    pub fn start_stream(&mut self, ctx: &egui::Context) {
        if self.hardware.active_audio_stream.is_some() {
            tracing::warn!("Stream already active, ignoring start request.");
            return;
        }

        if let Some(mic) = &self.hardware.selected_audio_source_name {
            match devices::audio::start_audio_stream(mic, Arc::clone(&self.hardware.audio_peak_amplitude)) {
                Ok(handle) => {
                    self.hardware.active_audio_stream = Some(handle);
                    self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(2)), text: "Audio stream started.".to_string().into() });
                }
                Err(e) => {
                    self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(2)), text: format!("Failed to start audio stream: {}", e).into() });
                    return;
                }
            }
        } else {
            self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(2)), text: "Cannot start: Missing audio input device.".to_string().into() });
            return;
        }

        let format = if let Some(f) = self.hardware.supported_formats.get(self.hardware.selected_format_index) {
            f
        } else {
            self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(2)), text: "Cannot start: No video format selected.".to_string().into() });
            return;
        };

        let resolution = self.hardware.selected_resolution;

        // Resize the main window to match the video stream resolution
        // The command needs to be sent to the main viewport.
        let new_size = egui::vec2(resolution.0 as f32, resolution.1 as f32);
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::InnerSize(new_size),
        );
        ctx.request_repaint(); // Force a repaint to ensure the new texture is drawn

        let device = self.hardware.selected_video_device.clone();
        let format = format.clone();
        let framerate = self.hardware.selected_framerate;
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.frame_receiver = Some(rx);

        let scaler_filter = self.scaler_filter.clone();
        let color_range = self.color_range.clone();

        let handle = thread::spawn(move || {
            if let Err(e) = video::decoder::video_thread_main(
                tx,
                device,
                format,
                resolution,
                framerate,
                scaler_filter,
                color_range,
            ) {
                tracing::error!("Video thread error: {}", e);
            }
        });
        self.video_thread = Some(handle);
        self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(2)), text: "Stream started.".to_string().into() });
        self.ui.video_window_open = true;
        self.ui.control_window_open = false;

        // Start the fullscreen toggle sequence to fix resizing issues.
        self.fullscreen_toggle_frame_count = Some(0);
    }

    pub fn stop_stream(&mut self, ctx: &egui::Context) {
        if self.ui.is_fullscreen {
            self.ui.is_fullscreen = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        self.stop_stream_resources();
        // Reset the texture to a black screen instead of removing it
        if let Some(texture) = &mut self.video_texture {
            texture.set(
                egui::ImageData::Color(egui::ColorImage::new([1, 1], egui::Color32::BLACK).into()),
                egui::TextureOptions::LINEAR,
            );
        }
        self.ui.video_window_open = false; // This now means "stream is not active"
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn stop_stream_resources(&mut self) {
        self.frame_receiver = None; // Drop receiver to signal threads to exit

        if let Some(handle) = self.video_thread.take() {
            let _ = handle.join();
        }

        if self.hardware.active_audio_stream.take().is_some() {
            self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(2)), text: "Stream stopped and audio stream dropped.".to_string().into() });
        } else {
            self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(2)), text: "Stream stopped.".to_string().into() });
        }

        self.frame_receiver = None;
        self.ui.video_window_open = false;
    }
}

impl eframe::App for AppState {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(gl) = _gl {
            if let Some(renderer) = self.crt_renderer.as_ref() {
                renderer.lock().unwrap().destroy(gl);
            }
        }
        self.stop_stream_resources();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut repaint_requested = false;

        // --- Control Window (Secondary) ---
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

        // --- Video Window (Primary) ---
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

        // Handle the fullscreen toggle sequence to fix window sizing on stream start.
        if let Some(count) = self.fullscreen_toggle_frame_count {
            match count {
                0 => {
                    // Frame 1 (after start): Wait one frame for stream to initialize.
                    self.fullscreen_toggle_frame_count = Some(1);
                }
                1 => {
                    // Frame 2: Go fullscreen.
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                    self.fullscreen_toggle_frame_count = Some(2);
                }
                2 => {
                    // Frame 3: Go back to windowed and end the sequence.
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    self.fullscreen_toggle_frame_count = None;
                }
                _ => self.fullscreen_toggle_frame_count = None, // Should not happen.
            }
            repaint_requested = true;
        }

        // Handle keyboard shortcuts for the main video window
        if ctx.input(|i| i.key_pressed(egui::Key::F)) {
            let is_fullscreen = !ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(is_fullscreen));
        }
        if ctx.input(|i| i.key_pressed(egui::Key::C)) {
            let current_filter = CrtFilter::from_u8(self.crt_filter.load(Ordering::Relaxed));
            let next_filter = current_filter.next();
            self.crt_filter.store(next_filter as u8, Ordering::Relaxed);
            config::save_config(self);
            self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(2)), text: format!("CRT filter set to: {}", next_filter.to_string()).into() });
        }
        if ctx.input(|i| i.key_pressed(egui::Key::G)) {
            self.video.pixelate_filter_enabled = !self.video.pixelate_filter_enabled;
            let status = if self.video.pixelate_filter_enabled {
                "enabled"
            } else {
                "disabled"
            };
            self.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(2)), text: format!("480p Pixelate filter {}.", status).into() });
            config::save_config(self);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            // Allow Esc to exit fullscreen on the video window
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

        // Handle window close request (e.g., from the 'X' button)
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.ui.video_window_open && !self.ui.show_quit_dialog {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.ui.show_quit_dialog = true;
            } // If no stream, or dialog is already open, allow the default close behavior.
            repaint_requested = true;
        }

        if let Some(rx) = &self.device_scan_receiver {
            if let Ok(scan_result) = rx.try_recv() {
                repaint_requested |= self.handle_device_scan_result(scan_result);
            } else {
                // Still loading
                repaint_requested = true;
            }
        }

        if let Some(rx) = &self.frame_receiver {
            if let Ok(frame) = rx.try_recv() {
                self.latest_frame = Some(frame);
                self.video_frames_since_last_check += 1;
            }
            // Always repaint when video is playing to show new frames
            repaint_requested = true;
        }

        self.update_fps_counters(ctx);
        self.toasts.show(ctx);

        if repaint_requested {
            ctx.request_repaint();
        }
    }
}
