use crate::video::{VideoFormat, types::RawFrame};
use crate::devices::{self};
use crate::config;
use eframe::egui;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, atomic::{AtomicU64, AtomicU8}};
use std::thread::JoinHandle;
use std::time::Instant;

pub struct HardwareState {
    pub audio_peak_amplitude: Arc<AtomicU64>,
    pub audio_latency_ms: Arc<AtomicU64>,
    pub audio_buffer_size: u32,
    pub audio_sample_rate: u32,
    pub audio_sample_format: String,
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
    pub latest_frame: Option<Arc<RawFrame>>,
    pub frame_receiver: Option<crossbeam_channel::Receiver<Arc<RawFrame>>>,
    pub device_scan_receiver: Option<crossbeam_channel::Receiver<devices::DeviceScanResult>>,
    pub logo_texture: Option<egui::TextureHandle>,
    pub last_fps_check: Instant,
    pub frames_since_last_check: u32,
    pub last_video_fps_check: Instant,
    pub video_frames_since_last_check: u32,

    pub crt_filter: Arc<AtomicU8>,
    pub scaler_filter: Arc<AtomicU8>,
    pub color_range: Arc<AtomicU8>,
    pub crt_renderer: Option<Arc<Mutex<crate::video::gpu::CrtFilterRenderer>>>,
    pub fullscreen_toggle_frame_count: Option<u8>,

    pub profiles: BTreeMap<String, config::Profile>,
    pub active_profile: String,
    pub new_profile_name: String,
}
