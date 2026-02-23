use crate::app::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::Ordering;

#[derive(Default, Serialize, Deserialize, Clone)]
pub struct Profile {
    pub pulse_source: Option<String>,
    pub pulse_sink: Option<String>,
    pub video_format_fourcc: Option<String>,
    pub crt_filter: Option<u8>,
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
    pub vibrance: Option<f32>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct MichadameConfig {
    // Global Settings
    pub video_device: Option<String>,
    pub usb_device: Option<String>,
    pub video_resolution: Option<(u32, u32)>,
    pub video_framerate: Option<u32>,
    pub reset_usb_on_startup: Option<bool>,
    pub has_shown_first_run_warning: Option<bool>,

    // Profiles
    pub active_profile: String,
    pub profiles: HashMap<String, Profile>,

    // Legacy Settings (kept for backwards compatibility on deserialize)
    pub pulse_source: Option<String>,
    pub pulse_sink: Option<String>,
    pub video_format_fourcc: Option<String>,
    pub crt_filter: Option<u8>,
    pub pixelate_filter_enabled: Option<bool>,
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
    pub vibrance: Option<f32>,
}

impl Default for MichadameConfig {
    fn default() -> Self {
        let mut profiles = HashMap::new();
        profiles.insert("Default".to_string(), Profile::default());
        Self {
            video_device: None,
            usb_device: None,
            video_resolution: None,
            video_framerate: None,
            reset_usb_on_startup: None,
            has_shown_first_run_warning: None,
            active_profile: "Default".to_string(),
            profiles,
            pulse_source: None,
            pulse_sink: None,
            video_format_fourcc: None,
            crt_filter: None,
            pixelate_filter_enabled: None,
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
            horizontal_stretch: None,
            median_filter_enabled: None,
            vibrance: None,
        }
    }
}

pub fn build_profile_from_state(state: &AppState) -> Profile {
    Profile {
        pulse_source: state.selected_pulse_source_name.clone(),
        pulse_sink: state.selected_pulse_sink_name.clone(),
        video_format_fourcc: state
            .supported_formats
            .get(state.selected_format_index)
            .map(|f| f.fourcc.clone()),
        crt_filter: Some(state.crt_filter.load(Ordering::Relaxed)),
        pixelate_filter_enabled: Some(state.pixelate_filter_enabled),

        crt_hard_scan: Some(state.crt_hard_scan),
        crt_warp_x: Some(state.crt_warp_x),
        crt_warp_y: Some(state.crt_warp_y),
        crt_shadow_mask: Some(state.crt_shadow_mask),
        crt_brightboost: Some(state.crt_brightboost),
        crt_hard_bloom_pix: Some(state.crt_hard_bloom_pix),
        crt_hard_bloom_scan: Some(state.crt_hard_bloom_scan),
        crt_bloom_amount: Some(state.crt_bloom_amount),
        crt_shape: Some(state.crt_shape),
        crt_hard_pix: Some(state.crt_hard_pix),
        use_magenta_background: Some(state.use_magenta_background),
        horizontal_stretch: Some(state.horizontal_stretch),
        median_filter_enabled: Some(state.median_filter_enabled),
        vibrance: Some(state.vibrance),
    }
}

pub fn save_config(state: &AppState) {
    let mut cfg = match confy::load::<MichadameConfig>("michadame", None) {
        Ok(c) => c,
        Err(_) => MichadameConfig::default(),
    };

    // Update global settings
    cfg.video_device = Some(state.selected_video_device.clone());
    cfg.usb_device = state.selected_usb_device.clone();
    cfg.video_resolution = if state.selected_resolution.0 > 0 {
        Some(state.selected_resolution)
    } else {
        None
    };
    cfg.video_framerate = if state.selected_framerate > 0 {
        Some(state.selected_framerate)
    } else {
        None
    };
    cfg.reset_usb_on_startup = Some(state.reset_usb_on_startup);
    cfg.has_shown_first_run_warning = Some(!state.show_first_run_dialog);

    cfg.active_profile = state.active_profile.clone();

    // Sync all profiles from state to drop any deleted ones
    cfg.profiles = state.profiles.clone();

    // Update active profile specifically (to capture latest unsaved UI state before saving)
    let current_profile_data = build_profile_from_state(state);
    cfg.profiles
        .insert(state.active_profile.clone(), current_profile_data);

    if let Err(e) = confy::store("michadame", None, cfg) {
        tracing::error!("Failed to save configuration: {}", e);
    }
}

pub fn apply_profile_to_state(state: &mut AppState, profile: &Profile) {
    if let Some(saved_source) = &profile.pulse_source {
        if state
            .pulse_sources
            .iter()
            .any(|(_, name)| name == saved_source)
        {
            state.selected_pulse_source_name = Some(saved_source.clone());
        }
    }
    if let Some(saved_sink) = &profile.pulse_sink {
        if state.pulse_sinks.iter().any(|(_, name)| name == saved_sink) {
            state.selected_pulse_sink_name = Some(saved_sink.clone());
        }
    }
    if let Some(filter) = profile.crt_filter {
        state.crt_filter.store(filter, Ordering::Relaxed);
    }
    if let Some(val) = profile.pixelate_filter_enabled {
        state.pixelate_filter_enabled = val;
    }
    if let Some(val) = profile.crt_hard_scan {
        state.crt_hard_scan = val;
    }
    if let Some(val) = profile.crt_hard_pix {
        state.crt_hard_pix = val;
    }
    if let Some(val) = profile.crt_brightboost {
        state.crt_brightboost = val;
    }
    if let Some(val) = profile.crt_warp_x {
        state.crt_warp_x = val;
    }
    if let Some(val) = profile.crt_warp_y {
        state.crt_warp_y = val;
    }
    if let Some(val) = profile.crt_shadow_mask {
        state.crt_shadow_mask = val;
    }
    if let Some(val) = profile.crt_hard_bloom_pix {
        state.crt_hard_bloom_pix = val;
    }
    if let Some(val) = profile.crt_hard_bloom_scan {
        state.crt_hard_bloom_scan = val;
    }
    if let Some(val) = profile.crt_bloom_amount {
        state.crt_bloom_amount = val;
    }
    if let Some(val) = profile.crt_shape {
        state.crt_shape = val;
    }
    if let Some(val) = profile.use_magenta_background {
        state.use_magenta_background = val;
    }
    if let Some(val) = profile.horizontal_stretch {
        state.horizontal_stretch = val;
    }
    if let Some(val) = profile.median_filter_enabled {
        state.median_filter_enabled = val;
    }
    if let Some(val) = profile.vibrance {
        state.vibrance = val;
    }
}

pub fn apply_config(state: &mut AppState, cfg: &MichadameConfig) {
    if state.profiles.is_empty() {
        // Migration: Populate default profile from legacy flat fields
        let mut default_profile = Profile::default();
        default_profile.pulse_source = cfg.pulse_source.clone();
        default_profile.pulse_sink = cfg.pulse_sink.clone();
        default_profile.video_format_fourcc = cfg.video_format_fourcc.clone();
        default_profile.crt_filter = cfg.crt_filter;
        default_profile.pixelate_filter_enabled = cfg.pixelate_filter_enabled;
        default_profile.crt_hard_scan = cfg.crt_hard_scan;
        default_profile.crt_warp_x = cfg.crt_warp_x;
        default_profile.crt_warp_y = cfg.crt_warp_y;
        default_profile.crt_shadow_mask = cfg.crt_shadow_mask;
        default_profile.crt_brightboost = cfg.crt_brightboost;
        default_profile.crt_hard_bloom_pix = cfg.crt_hard_bloom_pix;
        default_profile.crt_hard_bloom_scan = cfg.crt_hard_bloom_scan;
        default_profile.crt_bloom_amount = cfg.crt_bloom_amount;
        default_profile.crt_shape = cfg.crt_shape;
        default_profile.crt_hard_pix = cfg.crt_hard_pix;
        default_profile.use_magenta_background = cfg.use_magenta_background;
        default_profile.horizontal_stretch = cfg.horizontal_stretch;
        default_profile.median_filter_enabled = cfg.median_filter_enabled;
        default_profile.vibrance = cfg.vibrance;
        state
            .profiles
            .insert("Default".to_string(), default_profile);
        state.active_profile = "Default".to_string();
    } else {
        state.profiles = cfg.profiles.clone();
        state.active_profile = cfg.active_profile.clone();
    }

    if let Some(saved_device) = &cfg.video_device {
        if state.video_devices.contains(saved_device) {
            state.selected_video_device = saved_device.clone();
        }
    }
    if let Some(saved_usb) = &cfg.usb_device {
        if state.usb_devices.iter().any(|(id, _)| id == saved_usb) {
            state.selected_usb_device = Some(saved_usb.clone());
        }
    }

    if !state.selected_video_device.is_empty() {
        crate::video::types::apply_saved_format_config(state, cfg);
    }
    state.reset_usb_on_startup = cfg.reset_usb_on_startup.unwrap_or(false);
    if state.reset_usb_on_startup {
        if let Some(device_to_reset) = &state.selected_usb_device {
            state.status_message = match crate::devices::usb::reset_usb_device(device_to_reset) {
                Ok(_) => "Auto-reset USB device successfully.".to_string(),
                Err(e) => format!("Failed to auto-reset USB: {}", e),
            };
            tracing::info!("USB device reset on startup as requested.");
        }
    }
    if !cfg.has_shown_first_run_warning.unwrap_or(false) {
        state.show_first_run_dialog = true;
    }

    // Apply active profile
    let profile_to_apply = state.profiles.get(&state.active_profile).cloned();
    if let Some(profile) = profile_to_apply {
        apply_profile_to_state(state, &profile);
    }
}
