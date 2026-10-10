use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Global Twitch integration settings (not tied to a profile).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct TwitchConfig {
    /// Normalized channel login (lowercase, no `#`).
    pub channel: String,
    /// Show the live chat overlay on the video window.
    pub chat_overlay_enabled: bool,
    /// Overlay width as a fraction of the video window width.
    pub overlay_width_pct: f32,
    /// Overlay background opacity (0..1).
    pub overlay_opacity: f32,
    pub font_size: f32,
    /// Seconds before a chat message disappears. 0 = never.
    pub message_lifetime_secs: u32,
    pub max_messages: u32,
    /// Optional Twitch application Client ID override. Empty = built-in.
    pub client_id: String,
    /// Fire each chat message across the video, niconico style.
    pub niconico_enabled: bool,
    /// Comment text height as a fraction of the video height.
    pub niconico_size_pct: f32,
    /// Average seconds a comment takes to cross the screen.
    pub niconico_duration_secs: f32,
    /// Most comments on screen at once (oldest dropped first). 0 = unlimited.
    pub niconico_max_comments: u32,
}

impl Default for TwitchConfig {
    fn default() -> Self {
        Self {
            channel: String::new(),
            chat_overlay_enabled: false,
            overlay_width_pct: 0.22,
            overlay_opacity: 0.65,
            font_size: 14.0,
            message_lifetime_secs: 120,
            max_messages: 150,
            client_id: String::new(),
            niconico_enabled: false,
            niconico_size_pct: 0.06,
            niconico_duration_secs: 7.0,
            niconico_max_comments: 0,
        }
    }
}

#[derive(Default, Serialize, Deserialize, Clone)]
#[serde(default)]
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
    pub retro_pc_frame: Option<bool>,
    pub retro_pc_frame_dark_mode: Option<bool>,
    pub retro_pc_ambient_glow: Option<f32>,
    pub retro_software_mouse: Option<bool>,
    pub lights_off_night_mode: Option<bool>,
    pub night_mode_glow_intensity: Option<f32>,
    pub crt_glass_enabled: Option<bool>,
    pub crt_glass_intensity: Option<f32>,
    pub crt_glass_glossiness: Option<f32>,
    pub crt_glass_ceiling_light_enabled: Option<bool>,
    pub crt_glass_photographer_enabled: Option<bool>,
    pub crt_glass_photographer_intensity: Option<f32>,
    pub crt_glass_flash_enabled: Option<bool>,
    pub crt_glass_flash_intensity: Option<f32>,
    pub horizontal_stretch: Option<f32>,
    pub median_filter_enabled: Option<bool>,
    pub median_mix: Option<f32>,
    pub deinterlace_filter_enabled: Option<bool>,
    pub deinterlace_mode: Option<u8>,
    pub deinterlace_blend: Option<f32>,
    pub deinterlace_motion_threshold: Option<f32>,
    pub deinterlace_line_spacing: Option<f32>,
    pub deinterlace_spatial_mix: Option<f32>,
    pub vibrance: Option<f32>,
    pub overscan_x: Option<f32>,
    pub overscan_y: Option<f32>,
    pub underscan_x: Option<f32>,
    pub underscan_y: Option<f32>,
    pub border_crop_left: Option<f32>,
    pub border_crop_right: Option<f32>,
    pub border_crop_top: Option<f32>,
    pub border_crop_bottom: Option<f32>,

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

    // Cathode interference params
    pub cathode_interference_enabled: Option<bool>,
    pub cathode_intensity: Option<f32>,
    pub cathode_frequency: Option<f32>,
    pub cathode_randomization: Option<f32>,
    pub cathode_electricity_glow: Option<f32>,
    pub cathode_flicker_depth: Option<f32>,
    pub cathode_interference: Option<f32>,
    pub cathode_lightbulb_effect: Option<f32>,

    // Video resolution & framerate
    pub video_resolution: Option<(u32, u32)>,
    pub video_framerate: Option<u32>,

    // FFT mask filter
    pub fft_filter_enabled: Option<bool>,
    pub fft_mask_save_name: Option<String>,

    // Audio device settings
    pub audio_source: Option<String>,
    pub audio_buffer_size: Option<u32>,
    pub audio_sample_rate: Option<u32>,
    pub audio_sample_format: Option<String>,

    pub popup_under_crt: Option<bool>,

    // Audio filter
    pub audio_filter: Option<crate::replay::config::AudioFilterConfig>,
}

#[derive(Deserialize, Clone)]
#[serde(default)]
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
    pub retro_pc_frame: Option<bool>,
    pub retro_pc_frame_dark_mode: Option<bool>,
    pub retro_pc_ambient_glow: Option<f32>,
    pub retro_software_mouse: Option<bool>,
    pub lights_off_night_mode: Option<bool>,
    pub night_mode_glow_intensity: Option<f32>,
    pub crt_glass_enabled: Option<bool>,
    pub crt_glass_intensity: Option<f32>,
    pub crt_glass_glossiness: Option<f32>,
    pub crt_glass_ceiling_light_enabled: Option<bool>,
    pub crt_glass_photographer_enabled: Option<bool>,
    pub crt_glass_photographer_intensity: Option<f32>,
    pub crt_glass_flash_enabled: Option<bool>,
    pub crt_glass_flash_intensity: Option<f32>,
    pub horizontal_stretch: Option<f32>,
    pub median_filter_enabled: Option<bool>,
    pub median_mix: Option<f32>,
    pub deinterlace_filter_enabled: Option<bool>,
    pub deinterlace_mode: Option<u8>,
    pub deinterlace_blend: Option<f32>,
    pub deinterlace_motion_threshold: Option<f32>,
    pub deinterlace_line_spacing: Option<f32>,
    pub deinterlace_spatial_mix: Option<f32>,
    pub vibrance: Option<f32>,
    pub overscan_x: Option<f32>,
    pub overscan_y: Option<f32>,
    pub underscan_x: Option<f32>,
    pub underscan_y: Option<f32>,

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
    pub ocr_timeout_seconds: Option<u32>,
    pub default_halo: Option<crate::app::models::HaloSettings>,
    pub cathode_interference_enabled: Option<bool>,
    pub cathode_intensity: Option<f32>,
    pub cathode_frequency: Option<f32>,
    pub cathode_randomization: Option<f32>,
    pub cathode_electricity_glow: Option<f32>,
    pub cathode_flicker_depth: Option<f32>,
    pub cathode_interference: Option<f32>,
    pub cathode_lightbulb_effect: Option<f32>,
    pub default_cathode_interference: Option<crate::app::models::CathodeInterferenceSettings>,
    pub bank_current_tag: Option<String>,
    pub bank_compact_mode: Option<bool>,
    pub popup_under_crt: Option<bool>,
    #[serde(default)]
    pub twitch: TwitchConfig,
}

impl Default for LegacyConfig {
    fn default() -> Self {
        Self {
            replay: Default::default(),
            video_device: None,
            usb_device: None,
            video_resolution: None,
            video_framerate: None,
            reset_usb_on_startup: None,
            has_shown_first_run_warning: None,
            active_profile: default_active_profile(),
            profiles: BTreeMap::new(),
            audio_source: None,
            video_format_fourcc: None,
            crt_filter: None,
            selected_crt_filter: None,
            scaler_filter: None,
            color_range: None,
            pixelate_filter_enabled: None,
            audio_buffer_size: None,
            audio_sample_rate: None,
            audio_sample_format: None,
            crt_hard_scan: None,
            crt_warp_x: None,
            crt_warp_y: None,
            crt_shadow_mask: None,
            crt_brightboost: None,
            crt_hard_bloom_pix: None,
            crt_hard_bloom_scan: None,
            crt_bloom_amount: None,
            crt_shape: None,
            crt_hard_pix: None,
            use_magenta_background: None,
            retro_pc_frame: None,
            retro_pc_frame_dark_mode: None,
            retro_pc_ambient_glow: None,
            retro_software_mouse: None,
            lights_off_night_mode: None,
            night_mode_glow_intensity: None,
            crt_glass_enabled: None,
            crt_glass_intensity: None,
            crt_glass_glossiness: None,
            crt_glass_ceiling_light_enabled: None,
            crt_glass_photographer_enabled: None,
            crt_glass_photographer_intensity: None,
            crt_glass_flash_enabled: None,
            crt_glass_flash_intensity: None,
            horizontal_stretch: None,
            median_filter_enabled: None,
            median_mix: None,
            deinterlace_filter_enabled: None,
            deinterlace_mode: None,
            deinterlace_blend: None,
            deinterlace_motion_threshold: None,
            deinterlace_line_spacing: None,
            deinterlace_spatial_mix: None,
            vibrance: None,
            overscan_x: None,
            overscan_y: None,
            underscan_x: None,
            underscan_y: None,
            halo_brightboost: None,
            halo_brightboost1: None,
            halo_beam_min: None,
            halo_beam_max: None,
            halo_beam_size: None,
            halo_h_sharp: None,
            halo_glow: None,
            halo_bloom: None,
            halo_halation: None,
            halo_shadow_mask: None,
            halo_masksize: None,
            halo_maskstr: None,
            halo_mcut: None,
            halo_slotmask: None,
            halo_slotmask1: None,
            halo_double_slot: None,
            halo_smoothmask: None,
            halo_zoom: None,
            halo_intensity: None,
            halo_corner_size: None,
            halo_curvature: None,
            ocr_sticky_distance: None,
            ocr_hide_overlay: None,
            ocr_timeout_seconds: None,
            default_halo: None,
            cathode_interference_enabled: None,
            cathode_intensity: None,
            cathode_frequency: None,
            cathode_randomization: None,
            cathode_electricity_glow: None,
            cathode_flicker_depth: None,
            cathode_interference: None,
            cathode_lightbulb_effect: None,
            default_cathode_interference: None,
            bank_current_tag: None,
            bank_compact_mode: None,
            popup_under_crt: None,
            twitch: TwitchConfig::default(),
        }
    }
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
    pub ocr_timeout_seconds: Option<u32>,
    /// Tag applied to newly mined words in the mining bank.
    pub bank_current_tag: Option<String>,
    /// Whether the mining bank window starts in compact mode.
    pub bank_compact_mode: Option<bool>,
    pub popup_under_crt: Option<bool>,
    pub default_halo: Option<crate::app::models::HaloSettings>,
    pub default_cathode_interference: Option<crate::app::models::CathodeInterferenceSettings>,
    pub twitch: TwitchConfig,
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
            ocr_timeout_seconds: Some(45),
            default_halo: None,
            default_cathode_interference: None,
            twitch: TwitchConfig::default(),
            bank_current_tag: None,
            bank_compact_mode: None,
            popup_under_crt: Some(false),
            profiles,
        }
    }
}
