use crate::config;
use crate::devices::{self};
use crate::video::{types::RawFrame, VideoFormat};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicU8},
    Arc, Mutex,
};
use std::thread::JoinHandle;
use std::time::Instant;

pub struct HardwareState {
    pub video_queue_drops: Arc<AtomicU64>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    Shaders,
    Geometry,
    Effects,
    Devices,
    AudioReplay,
    OcrDict,
    Hotkeys,
}

pub struct UiState {
    pub debug_open: bool,
    pub is_fullscreen: bool,
    pub reset_usb_on_startup: bool,
    pub show_first_run_dialog: bool,
    pub show_quit_dialog: bool,
    pub show_stop_stream_dialog: bool,
    pub video_window_open: bool,
    pub control_window_open: bool,
    pub dismissed_config_error: bool,
    pub active_settings_tab: SettingsTab,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HaloSettings {
    pub brightboost: f32,
    pub brightboost1: f32,
    pub beam_min: f32,
    pub beam_max: f32,
    pub beam_size: f32,
    pub h_sharp: f32,
    pub glow: f32,
    pub bloom: f32,
    pub halation: f32,
    pub shadow_mask: f32,
    pub masksize: f32,
    pub maskstr: f32,
    pub mcut: f32,
    pub slotmask: f32,
    pub slotmask1: f32,
    pub double_slot: f32,
    pub smoothmask: f32,
    pub halo_zoom: f32,
    pub halo_intensity: f32,
    pub corner_size: f32,
    pub curvature: bool,
}

impl Default for HaloSettings {
    fn default() -> Self {
        Self {
            brightboost: 1.30,
            brightboost1: 1.65,
            beam_min: 1.80,
            beam_max: 1.65,
            beam_size: 1.25,
            h_sharp: 4.20,
            glow: 0.53,
            bloom: 0.57,
            halation: 0.23,
            shadow_mask: 6.0,
            masksize: 2.0,
            maskstr: 0.50,
            mcut: 0.80,
            slotmask: 0.45,
            slotmask1: 0.30,
            double_slot: 3.0,
            smoothmask: 0.80,
            halo_zoom: 100.0,
            halo_intensity: 0.95,
            corner_size: 0.0,
            curvature: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CathodeInterferenceSettings {
    pub enabled: bool,
    pub intensity: f32,
    pub frequency: f32,
    pub randomization: f32,
    pub electricity_glow: f32,
    pub flicker_depth: f32,
    pub interference: f32,
    pub lightbulb_effect: f32,
}

impl Default for CathodeInterferenceSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            intensity: 0.5,
            frequency: 1.0,
            randomization: 0.5,
            electricity_glow: 0.5,
            flicker_depth: 0.4,
            interference: 0.3,
            lightbulb_effect: 0.35,
        }
    }
}

pub struct VideoSettings {
    pub pixelate_filter_enabled: bool,
    pub use_magenta_background: bool,
    pub retro_pc_frame: bool,
    pub retro_pc_frame_dark_mode: bool,
    pub retro_pc_ambient_glow: f32,
    pub lights_off_night_mode: bool,
    pub night_mode_glow_intensity: f32,
    pub crt_glass_enabled: bool,
    pub crt_glass_intensity: f32,
    pub crt_glass_glossiness: f32,
    pub crt_glass_ceiling_light_enabled: bool,
    pub crt_glass_photographer_enabled: bool,
    pub crt_glass_photographer_intensity: f32,
    pub crt_glass_flash_enabled: bool,
    pub crt_glass_flash_intensity: f32,
    pub horizontal_stretch: f32,
    pub median_filter_enabled: bool,
    pub median_mix: f32,
    pub deinterlace_filter_enabled: bool,
    pub deinterlace_mode: u8,
    pub deinterlace_blend: f32,
    pub deinterlace_motion_threshold: f32,
    pub deinterlace_line_spacing: f32,
    pub deinterlace_spatial_mix: f32,
    pub vibrance: f32,
    pub overscan_x: f32,
    pub overscan_y: f32,
    pub underscan_x: f32,
    pub underscan_y: f32,
    pub border_crop_left: f32,
    pub border_crop_right: f32,
    pub border_crop_top: f32,
    pub border_crop_bottom: f32,
    pub fft_filter_enabled: bool,
    pub fft_mask_window_open: bool,
}

pub struct PendingAudioStream {
    pub source_name: String,
    pub buffer_size: u32,
    pub sample_rate: u32,
    pub sample_format: String,
}

pub struct AppState {
    pub replay: crate::replay::Replay,
    pub hardware: HardwareState,
    pub ui: UiState,
    pub crt: CrtSettings,
    pub halo: HaloSettings,
    pub halo_defaults: HaloSettings,
    pub cathode_interference: CathodeInterferenceSettings,
    pub cathode_interference_defaults: CathodeInterferenceSettings,
    pub selected_crt_filter: crate::devices::filter_type::CrtFilter,
    pub video: VideoSettings,
    pub toasts: egui_toast::Toasts,

    pub video_thread: Option<JoinHandle<()>>,
    pub video_stop_requested: Option<Arc<AtomicBool>>,
    pub video_texture: Option<egui::TextureHandle>,
    pub latest_frame: Option<Arc<RawFrame>>,
    pub frame_receiver: Option<crossbeam_channel::Receiver<Arc<RawFrame>>>,
    pub video_status_receiver:
        Option<crossbeam_channel::Receiver<crate::video::decoder::VideoThreadEvent>>,
    pub pending_audio_stream: Option<PendingAudioStream>,
    pub device_scan_receiver: Option<crossbeam_channel::Receiver<devices::DeviceScanResult>>,
    pub logo_texture: Option<egui::TextureHandle>,
    pub gui_fps: f32,
    pub video_fps: f32,
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

    pub fft_filter: Option<Arc<Mutex<crate::video::gpu::FftFilter>>>,
    pub fft_mask_data: Vec<u8>,
    pub fft_mask_dirty: bool,
    pub fft_mask_resolution: (u32, u32),
    pub fft_brush_radius: f32,
    pub fft_mask_threshold: f32,
    pub fft_black_threshold: f32,
    pub fft_mask_save_name: String,
    pub fft_available_masks: Vec<String>,
    pub ocr: crate::ocr::OcrState,
    pub dict: crate::dict::DictState,
    pub bank: crate::bank::BankState,
    pub config_load_error: Option<String>,
    pub config_quarantine_path: Option<std::path::PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn test_app_state_default() {
        let state = AppState::default();
        assert_eq!(state.active_profile, "Default");
        assert!(state.profiles.contains_key("Default"));
        assert!(state.hardware.video_devices.is_empty());
        assert!(!state.ui.is_fullscreen);
        assert!(state.ui.control_window_open);
        assert!(!state.ui.video_window_open);
        assert!(state.video_status_receiver.is_none());
        assert!(state.pending_audio_stream.is_none());
        assert_eq!(state.crt.hard_scan, -8.0);
        assert_eq!(state.halo.brightboost, 1.30);
        assert_eq!(state.halo_defaults, state.halo);
        assert_eq!(state.cathode_interference_defaults, state.cathode_interference);
        assert!(!state.cathode_interference.enabled);
        assert!(state.config_load_error.is_none());
        assert!(state.config_quarantine_path.is_none());
        assert_eq!(state.selected_crt_filter, crate::devices::filter_type::CrtFilter::Lottes);
        assert_eq!(state.video.horizontal_stretch, 1.0);
    }

    #[test]
    fn test_hardware_state_defaults() {
        let state = AppState::default();
        assert_eq!(state.hardware.audio_buffer_size, 1024);
        assert_eq!(state.hardware.audio_sample_rate, 48000);
        assert_eq!(state.hardware.audio_sample_format, "S16LE");
        assert!(state.hardware.video_devices.is_empty());
    }

    #[test]
    fn test_atomic_defaults() {
        let state = AppState::default();
        assert_eq!(
            state.crt_filter.load(Ordering::Relaxed),
            crate::devices::filter_type::CrtFilter::Off as u8
        );
        assert_eq!(
            state.scaler_filter.load(Ordering::Relaxed),
            crate::video::types::ScalerFilter::Bicubic as u8
        );
        assert_eq!(
            state.color_range.load(Ordering::Relaxed),
            crate::video::types::ColorRange::Full as u8
        );
    }
}
