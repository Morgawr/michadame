use crate::video::VideoFormat;
use crate::{config, devices, ui, video, devices::filter_type::CrtFilter};
use anyhow::Context;
use eframe::egui;
use std::sync::{Mutex, 
    atomic::{AtomicBool, AtomicU8, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::Instant;

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
    device_scan_receiver: Option<crossbeam_channel::Receiver<devices::DeviceScanResult>>,
    pub logo_texture: Option<egui::TextureHandle>,
    last_fps_check: Instant,
    frames_since_last_check: u32,
    last_video_fps_check: Instant,
    video_frames_since_last_check: u32,
    pub is_fullscreen: bool,
    pub reset_usb_on_startup: bool,
    pub show_first_run_dialog: bool,
    pub show_quit_dialog: bool,
    pub show_stop_stream_dialog: bool,
    pub video_window_open: bool,
    pub pixelate_filter_enabled: bool,
    pub crt_filter: Arc<AtomicU8>,
    pub crt_renderer: Option<Arc<Mutex<video::gpu_filter::CrtFilterRenderer>>>,

    // Lottes Filter Params
    pub crt_hard_scan: f32,
    pub crt_warp_x: f32,
    pub crt_warp_y: f32,
    pub crt_shadow_mask: f32,
    pub crt_brightboost: f32,
    pub crt_hard_bloom_pix: f32,
    pub crt_hard_bloom_scan: f32,
    pub crt_bloom_amount: f32,
    pub crt_shape: f32,
    pub crt_hard_pix: f32,
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
            show_stop_stream_dialog: false,
            video_window_open: false,
            pixelate_filter_enabled: false,
            crt_filter: Arc::new(AtomicU8::new(CrtFilter::Scanlines as u8)),
            crt_renderer: None,

            // Lottes Filter Params
            crt_hard_scan: -8.0,
            crt_warp_x: 0.031,
            crt_warp_y: 0.041,
            crt_shadow_mask: 3.0,
            crt_brightboost: 1.0,
            crt_hard_bloom_pix: -1.5,
            crt_hard_bloom_scan: -2.0,
            crt_bloom_amount: 0.15,
            crt_shape: 2.0,
            crt_hard_pix: -3.0,
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

        if let Some(gl) = cc.gl.as_ref() {
            app_state.crt_renderer = Some(Arc::new(Mutex::new(video::gpu_filter::CrtFilterRenderer::new(gl))));
        }

        app_state.logo_texture = Some(logo_texture);

        // Asynchronous Device Scanning
        let (tx, rx) = crossbeam_channel::unbounded();
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
        let crt_filter = self.crt_filter.clone();
        self.frame_receiver = Some(rx);

        let handle = thread::spawn(move || {
            if let Err(e) =
                video::decoder::video_thread_main(tx, stop_flag, device, format, resolution, framerate, crt_filter)
            {
                tracing::error!("Video thread error: {}", e);
            }
        });
        self.video_thread = Some(handle);
        self.status_message = "Stream started.".to_string();
        self.video_window_open = true;
    }

    pub fn stop_stream(&mut self, ctx: &egui::Context) {
        if self.is_fullscreen {
            self.is_fullscreen = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        self.stop_stream_resources();
        self.video_window_open = false;
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
        self.video_window_open = false;
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
        
        // --- Video Window ---
        if self.video_window_open {
            let video_ctx = ctx.clone();
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("video_window"),
                egui::ViewportBuilder::default()
                    .with_title("Michadame Video") // Set a default title
                    .with_inner_size([self.selected_resolution.0 as f32, self.selected_resolution.1 as f32])
                    .with_inner_size([self.selected_resolution.0 as f32, self.selected_resolution.1 as f32]),
                |ctx, class| {
                    assert!(
                        class == egui::ViewportClass::Immediate,
                        "This egui backend doesn't support multiple viewports"
                    );

                    egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
                        ui::draw_video_player(self, ui, ctx);

                        if self.show_stop_stream_dialog {
                            ui::dialogs::show_stop_stream_dialog(self, ctx, ui, &video_ctx);
                        }
                    });

                    // Handle keyboard shortcuts only for this window
                    if ctx.input(|i| i.key_pressed(egui::Key::F)) {
                        let is_fullscreen = !ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(is_fullscreen));
                    }
                    if ctx.input(|i| i.key_pressed(egui::Key::C)) {
                        let current_filter = CrtFilter::from_u8(self.crt_filter.load(Ordering::Relaxed));
                        let next_filter = current_filter.next();
                        self.crt_filter.store(next_filter as u8, Ordering::Relaxed);
                        self.status_message = format!("CRT filter set to: {}", next_filter.to_string());
                    }
                    if ctx.input(|i| i.key_pressed(egui::Key::G)) {
                        self.pixelate_filter_enabled = !self.pixelate_filter_enabled;
                        let status = if self.pixelate_filter_enabled { "enabled" } else { "disabled" };
                        self.status_message = format!("480p Pixelate filter {}.", status);
                    }
                    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                        // Allow Esc to exit fullscreen on the video window
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    }
                    if ctx.input(|i| i.key_pressed(egui::Key::Q)) {
                        if self.video_window_open && !self.show_stop_stream_dialog {
                            self.show_stop_stream_dialog = true;
                        }
                    }

                    if ctx.input(|i| i.viewport().close_requested()) {
                        // This is how we close the window.
                        self.stop_stream(&video_ctx);
                    }
                },
            );
        }

        // Handle window close request (e.g., from the 'X' button)
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.video_window_open && !self.show_quit_dialog {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.show_quit_dialog = true;
            } else {
            }
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