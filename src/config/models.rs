use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Default, Serialize, Deserialize, Clone)]
pub struct Profile {
    pub video_format_fourcc: Option<String>,
    pub crt_filter: Option<u8>,
    pub scaler_filter: Option<u8>,
    pub color_range: Option<u8>,
    pub pixelate_filter_enabled: Option<bool>,

    // Lottes params
    pub crt_hard_scan: Option<f32>,
    pub crt_warp_x: Option<f32>,
    pub crt_warp_y: Option<f32>,
    pub crt_shadow_mask: Option<f32>,
    pub crt_brightboost: Option<f32>,
    pub crt_hard_bloom_pix: Option<f32>,
    pub crt_hard_bloom_scan: Option<f32>,
    pub crt_bloom_amount: Option<f32>,
    pub crt_shape: Option<f32>,
    pub crt_hard_pix: Option<f32>,
    pub use_magenta_background: Option<bool>,
    pub horizontal_stretch: Option<f32>,
    pub median_filter_enabled: Option<bool>,
    pub median_mix: Option<f32>,
    pub vibrance: Option<f32>,
    pub overscan_x: Option<f32>,
    pub overscan_y: Option<f32>,

    pub selected_crt_filter: Option<u8>,

    // Halo params
    pub halo_brightboost: Option<f32>,
    pub halo_brightboost1: Option<f32>,
    pub halo_beam_min: Option<f32>,
    pub halo_beam_max: Option<f32>,
    pub halo_beam_size: Option<f32>,
    pub halo_h_sharp: Option<f32>,
    pub halo_glow: Option<f32>,
    pub halo_bloom: Option<f32>,
    pub halo_halation: Option<f32>,
    pub halo_shadow_mask: Option<f32>,
    pub halo_masksize: Option<f32>,
    pub halo_maskstr: Option<f32>,
    pub halo_mcut: Option<f32>,
    pub halo_slotmask: Option<f32>,
    pub halo_slotmask1: Option<f32>,
    pub halo_double_slot: Option<f32>,
    pub halo_smoothmask: Option<f32>,
    pub halo_zoom: Option<f32>,
    pub halo_intensity: Option<f32>,
    pub halo_corner_size: Option<f32>,
    pub halo_curvature: Option<bool>,
}

#[derive(Deserialize, Clone)]
pub struct LegacyConfig {
    #[serde(default)]
    pub replay: crate::replay::config::ReplayConfig,
    pub video_device: Option<String>,
    pub usb_device: Option<String>,
    pub video_resolution: Option<(u32, u32)>,
    pub video_framerate: Option<u32>,
    pub reset_usb_on_startup: Option<bool>,
    pub has_shown_first_run_warning: Option<bool>,

    #[serde(default = "default_active_profile")]
    pub active_profile: String,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,

    pub audio_source: Option<String>,
    pub video_format_fourcc: Option<String>,
    pub crt_filter: Option<u8>,
    pub selected_crt_filter: Option<u8>,
    pub scaler_filter: Option<u8>,
    pub color_range: Option<u8>,
    pub pixelate_filter_enabled: Option<bool>,
    pub audio_buffer_size: Option<u32>,
    pub audio_sample_rate: Option<u32>,
    pub audio_sample_format: Option<String>,
    pub crt_hard_scan: Option<f32>,
    pub crt_warp_x: Option<f32>,
    pub crt_warp_y: Option<f32>,
    pub crt_shadow_mask: Option<f32>,
    pub crt_brightboost: Option<f32>,
    pub crt_hard_bloom_pix: Option<f32>,
    pub crt_hard_bloom_scan: Option<f32>,
    pub crt_bloom_amount: Option<f32>,
    pub crt_shape: Option<f32>,
    pub crt_hard_pix: Option<f32>,
    pub use_magenta_background: Option<bool>,
    pub horizontal_stretch: Option<f32>,
    pub median_filter_enabled: Option<bool>,
    pub median_mix: Option<f32>,
    pub vibrance: Option<f32>,
    pub overscan_x: Option<f32>,
    pub overscan_y: Option<f32>,

    // Halo params
    pub halo_brightboost: Option<f32>,
    pub halo_brightboost1: Option<f32>,
    pub halo_beam_min: Option<f32>,
    pub halo_beam_max: Option<f32>,
    pub halo_beam_size: Option<f32>,
    pub halo_h_sharp: Option<f32>,
    pub halo_glow: Option<f32>,
    pub halo_bloom: Option<f32>,
    pub halo_halation: Option<f32>,
    pub halo_shadow_mask: Option<f32>,
    pub halo_masksize: Option<f32>,
    pub halo_maskstr: Option<f32>,
    pub halo_mcut: Option<f32>,
    pub halo_slotmask: Option<f32>,
    pub halo_slotmask1: Option<f32>,
    pub halo_double_slot: Option<f32>,
    pub halo_smoothmask: Option<f32>,
    pub halo_zoom: Option<f32>,
    pub halo_intensity: Option<f32>,
    pub halo_corner_size: Option<f32>,
    pub halo_curvature: Option<bool>,
    pub ocr_sticky_distance: Option<f32>,
    pub ocr_hide_overlay: Option<bool>,
    pub default_halo: Option<crate::app::models::HaloSettings>,
}

pub fn default_active_profile() -> String {
    "Default".to_string()
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(from = "LegacyConfig")]
pub struct MichadameConfig {
    pub video_device: Option<String>,
    pub usb_device: Option<String>,
    pub video_resolution: Option<(u32, u32)>,
    pub video_framerate: Option<u32>,
    pub reset_usb_on_startup: Option<bool>,
    pub has_shown_first_run_warning: Option<bool>,
    pub audio_source: Option<String>,
    pub audio_buffer_size: Option<u32>,
    pub audio_sample_rate: Option<u32>,
    pub audio_sample_format: Option<String>,
    pub active_profile: String,
    pub ocr_sticky_distance: Option<f32>,
    pub ocr_hide_overlay: Option<bool>,
    pub default_halo: Option<crate::app::models::HaloSettings>,
    // confy's TOML serializer requires scalar fields before nested tables.
    pub replay: crate::replay::config::ReplayConfig,
    pub profiles: BTreeMap<String, Profile>,
}

impl Default for MichadameConfig {
    fn default() -> Self {
        let mut profiles = BTreeMap::new();
        profiles.insert("Default".to_string(), Profile::default());
        Self {
            replay: Default::default(),
            video_device: None,
            usb_device: None,
            video_resolution: None,
            video_framerate: None,
            reset_usb_on_startup: None,
            has_shown_first_run_warning: None,
            audio_source: None,
            audio_buffer_size: None,
            audio_sample_rate: None,
            audio_sample_format: None,
            active_profile: "Default".to_string(),
            ocr_sticky_distance: Some(0.6),
            ocr_hide_overlay: Some(false),
            default_halo: None,
            profiles,
        }
    }
}
