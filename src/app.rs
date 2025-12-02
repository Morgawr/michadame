use crate::video::VideoFormat;
use crate::{config, devices, ui, video};
use anyhow::Context;
use eframe::egui;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::Instant;

#[derive(PartialEq, Eq, Copy, Clone)]
enum FullscreenAction {
    Idle,
    Enter,
    Exit,
}

pub struct AppState {
    pub video_devices: Vec<String>,
    pub usb_devices: Vec<(String, String)>,
    pub selected_usb_device: Option<String>,
    pub selected_video_device: String,
    pub pulse_sources: Vec<(String, String)>,
    pub pulse_sinks: Vec<(String, String)>,
    pub selected_pulse_source_name: Option<String>,
    pub selected_pulse_sink_name: Option<String>,
    pub pulse_loopback_module_index: Option<u32>,
    pub status_message: String,
    pub supported_formats: Vec<VideoFormat>,
    pub selected_format_index: usize,
    pub selected_resolution: (u32, u32),
    pub selected_framerate: u32,
    pub video_thread: Option<JoinHandle<()>>,
    pub stop_video_thread: Option<Arc<AtomicBool>>,
    pub video_texture: Option<egui::TextureHandle>,
    pub frame_receiver: Option<crossbeam_channel::Receiver<Arc<egui::ColorImage>>>,
    device_scan_receiver: Option<mpsc::Receiver<devices::DeviceScanResult>>,
    pub logo_texture: Option<egui::TextureHandle>,
    last_fps_check: Instant,
    frames_since_last_check: u32,
    last_video_fps_check: Instant,
    video_frames_since_last_check: u32,
    pub is_fullscreen: bool,
    pub reset_usb_on_startup: bool,
    pub show_first_run_dialog: bool,
    pub show_quit_dialog: bool,
    fullscreen_action: FullscreenAction,
    pub crt_filter_enabled: Arc<AtomicBool>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            video_devices: Vec::new(),
            usb_devices: Vec::new(),
            selected_usb_device: None,
            selected_video_device: String::new(),
            pulse_sources: Vec::new(),
            pulse_sinks: Vec::new(),
            selected_pulse_source_name: None,
            selected_pulse_sink_name: None,
            pulse_loopback_module_index: None,
            status_message: "Loading devices...".to_string(),
            supported_formats: Vec::new(),
            selected_format_index: 0,
            selected_resolution: (0, 0),
            selected_framerate: 0,
            video_thread: None,
            stop_video_thread: None,
            video_texture: None,
            frame_receiver: None,
            device_scan_receiver: None,
            logo_texture: None,
            last_fps_check: Instant::now(),
            frames_since_last_check: 0,
            last_video_fps_check: Instant::now(),
            video_frames_since_last_check: 0,
            is_fullscreen: false,
            reset_usb_on_startup: false,
            show_first_run_dialog: false,
            show_quit_dialog: false,
            fullscreen_action: FullscreenAction::Idle,
            crt_filter_enabled: Arc::new(AtomicBool::new(true)),
        }
    }
}

impl AppState {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        let mut app_state = AppState::default();

        // Load UI Logo Texture
        let logo_image =
            image::load_from_memory(include_bytes!("../assets/logo.png")).expect("Failed to load logo");
        let logo_size = [logo_image.width() as _, logo_image.height() as _];
        let logo_rgba = logo_image.to_rgba8();
        let logo_pixels = logo_rgba.as_flat_samples();
        let logo_color_image =
            egui::ColorImage::from_rgba_unmultiplied(logo_size, logo_pixels.as_slice());
        let logo_texture = cc
            .egui_ctx
            .load_texture("logo", logo_color_image, Default::default());
        app_state.logo_texture = Some(logo_texture);

        // Asynchronous Device Scanning
        let (tx, rx) = mpsc::channel();
        app_state.device_scan_receiver = Some(rx);

        let egui_ctx = cc.egui_ctx.clone();
        std::thread::spawn(move || {
            let video_result = devices::video::find_video_devices();
            let pulse_result = devices::audio::find_pulse_devices();
            let usb_result = devices::usb::find_usb_devices();

            let result: devices::DeviceScanResult = (|| {
                let video_devices = video_result.context("Failed to find video devices")?;
                let (pulse_sources, pulse_sinks) =
                    pulse_result.context("Failed to find PulseAudio devices")?;
                let usb_devices = usb_result.context("Failed to find USB devices")?;
                Ok((video_devices, pulse_sources, pulse_sinks, usb_devices))
            })();

            if let Err(e) = &result {
                tracing::error!("Device scan failed: {:?}", e);
            };
            let _ = tx.send(result);
            egui_ctx.request_repaint();
        });
        app_state
    }

    fn handle_device_scan_result(&mut self, result: devices::DeviceScanResult) -> bool {
        let scan_successful = match result {
            Ok((video_devices, pulse_sources, pulse_sinks, usb_devices)) => {
                self.video_devices = video_devices;
                self.selected_video_device = self.video_devices.first().cloned().unwrap_or_default();
                self.pulse_sources = pulse_sources;
                self.pulse_sinks = pulse_sinks;
                self.usb_devices = usb_devices;

                if let Ok(cfg) = confy::load::<config::MichadameConfig>("michadame", None) {
                    config::apply_config(self, &cfg);
                }
                self.status_message = "Devices loaded successfully.".to_string();
                true
            }
            Err(e) => {
                self.status_message = format!("Error: {}", e);
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

        let gui_fps = if elapsed_secs > 0.0 { self.frames_since_last_check as f32 / elapsed_secs } else { 0.0 };
        let video_fps = if video_elapsed_secs > 0.0 { self.video_frames_since_last_check as f32 / video_elapsed_secs } else { 0.0 };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "Michadame Viewer | UI: {:.0} FPS | Video: {:.0} FPS",
            gui_fps, video_fps
        )));
    }

    pub fn start_stream(&mut self, ctx: &egui::Context) {
        match (&self.selected_pulse_source_name, &self.selected_pulse_sink_name) {
            (Some(mic), Some(sink)) => {
                match devices::audio::load_pulse_loopback(mic, sink) {
                    Ok(index) => {
                        self.pulse_loopback_module_index = Some(index);
                        self.status_message = "PulseAudio loopback loaded.".to_string();
                    }
                    Err(e) => {
                        self.status_message = format!("Failed to load loopback: {}", e);
                        return;
                    }
                }
            }
            _ => {
                self.status_message = "Cannot start: Missing PulseAudio devices.".to_string();
                return;
            }
        }

        let format = if let Some(f) = self.supported_formats.get(self.selected_format_index) {
            f
        } else {
            self.status_message = "Cannot start: No video format selected.".to_string();
            return;
        };

        if self.video_texture.is_none() {
            let tex_manager = ctx.tex_manager();
            let tex_id = tex_manager.write().alloc(
                "ffmpeg_video".to_string(),
                egui::ImageData::Color(egui::ColorImage::new([1, 1], egui::Color32::BLACK).into()),
                egui::TextureOptions::LINEAR,
            );
            self.video_texture = Some(egui::TextureHandle::new(tex_manager, tex_id));
        }

        let stop_flag = Arc::new(AtomicBool::new(false));
        self.stop_video_thread = Some(stop_flag.clone());

        let device = self.selected_video_device.clone();
        let format = format.clone();
        let resolution = self.selected_resolution;
        let framerate = self.selected_framerate;
        let (tx, rx) = crossbeam_channel::bounded(1);
        let crt_filter_enabled = self.crt_filter_enabled.clone();
        self.frame_receiver = Some(rx);

        let handle = thread::spawn(move || {
            if let Err(e) = video::decoder::video_thread_main(
                tx, stop_flag, device, format, resolution, framerate, crt_filter_enabled,
            ) {
                tracing::error!("Video thread error: {}", e);
            }
        });
        self.video_thread = Some(handle);
        self.status_message = "Stream started.".to_string();

        if !self.is_fullscreen {
            let top_ui_height = 400.0; // Approximate height of the top UI panel.
            let required_size = egui::vec2(resolution.0 as f32, resolution.1 as f32 + top_ui_height);
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(required_size));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(required_size));
            self.fullscreen_action = FullscreenAction::Enter;
        }
    }

    pub fn stop_stream(&mut self, ctx: &egui::Context) {
        if self.is_fullscreen {
            self.is_fullscreen = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        self.stop_stream_resources();
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize([500.0, 200.0].into()));
    }

    fn stop_stream_resources(&mut self) {
        if let Some(stop_flag) = self.stop_video_thread.take() {
            stop_flag.store(true, Ordering::Relaxed);
        }
        if let Some(handle) = self.video_thread.take() {
            let _ = handle.join();
        }

        if let Some(index) = self.pulse_loopback_module_index.take() {
            if let Err(e) = devices::audio::unload_pulse_loopback(index) {
                self.status_message = format!("Stream stopped, but failed to unload PulseAudio module: {}", e);
            } else {
                self.status_message = "Stream stopped and PulseAudio module unloaded.".to_string();
            }
        } else {
            self.status_message = "Stream stopped.".to_string();
        }

        self.video_texture = None;
        self.frame_receiver = None;
    }
}

impl eframe::App for AppState {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_stream_resources();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut repaint_requested = false;

        match self.fullscreen_action {
            FullscreenAction::Enter => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                self.fullscreen_action = FullscreenAction::Exit;
                repaint_requested = true;
            }
            FullscreenAction::Exit => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                self.fullscreen_action = FullscreenAction::Idle;
                repaint_requested = true;
            }
            FullscreenAction::Idle => {}
        }

        // Handle window close request (e.g., from the 'X' button)
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.video_thread.is_some() && !self.show_quit_dialog {
                // If a stream is running, show the confirmation dialog
                // and cancel the default close behavior.
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.show_quit_dialog = true;
            }
            // If no stream is running, we don't cancel the close,
            // so the application will exit. The `on_exit` method will be called for cleanup.
            repaint_requested = true;
        }

        if ctx.input(|i| i.key_pressed(egui::Key::Q) || i.key_pressed(egui::Key::Escape)) {
            if self.video_thread.is_some() {
                self.show_quit_dialog = true;
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            repaint_requested = true;
        }

        if self.video_thread.is_some() {
            if ctx.input(|i| i.key_pressed(egui::Key::F)) {
                self.is_fullscreen = !self.is_fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
                repaint_requested = true;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::C)) {
                let is_enabled = !self.crt_filter_enabled.load(Ordering::Relaxed);
                self.crt_filter_enabled.store(is_enabled, Ordering::Relaxed);
                self.status_message = format!(
                    "CRT filter {}",
                    if is_enabled { "enabled" } else { "disabled" }
                );
                repaint_requested = true;
            }
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
            if let Ok(image) = rx.try_recv() {
                self.video_texture.as_mut().unwrap().set(image, egui::TextureOptions::LINEAR);
                self.video_frames_since_last_check += 1;
            }
            // Always repaint when video is playing to show new frames
            repaint_requested = true;
        }

        repaint_requested |= ui::draw_main_ui(self, ctx);
        self.update_fps_counters(ctx);

        if repaint_requested {
            ctx.request_repaint();
        }
    }
}