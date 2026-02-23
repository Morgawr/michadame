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
    pub vibrance: Option<f32>,
}

#[derive(Deserialize, Clone)]
struct LegacyConfig {
    video_device: Option<String>,
    usb_device: Option<String>,
    video_resolution: Option<(u32, u32)>,
    video_framerate: Option<u32>,
    reset_usb_on_startup: Option<bool>,
    has_shown_first_run_warning: Option<bool>,

    #[serde(default = "default_active_profile")]
    active_profile: String,
    #[serde(default)]
    profiles: HashMap<String, Profile>,

    pulse_source: Option<String>,
    pulse_sink: Option<String>,
    video_format_fourcc: Option<String>,
    crt_filter: Option<u8>,
    scaler_filter: Option<u8>,
    color_range: Option<u8>,
    pixelate_filter_enabled: Option<bool>,
    crt_hard_scan: Option<f32>,
    crt_warp_x: Option<f32>,
    crt_warp_y: Option<f32>,
    crt_shadow_mask: Option<f32>,
    crt_brightboost: Option<f32>,
    crt_hard_bloom_pix: Option<f32>,
    crt_hard_bloom_scan: Option<f32>,
    crt_bloom_amount: Option<f32>,
    crt_shape: Option<f32>,
    crt_hard_pix: Option<f32>,
    use_magenta_background: Option<bool>,
    horizontal_stretch: Option<f32>,
    median_filter_enabled: Option<bool>,
    vibrance: Option<f32>,
}

fn default_active_profile() -> String {
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
    pub active_profile: String,
    pub profiles: HashMap<String, Profile>,
}

impl From<LegacyConfig> for MichadameConfig {
    fn from(legacy: LegacyConfig) -> Self {
        let mut profiles = legacy.profiles;
        let mut active_profile = legacy.active_profile;

        if profiles.is_empty() {
            let legacy_profile = Profile {
                pulse_source: legacy.pulse_source,
                pulse_sink: legacy.pulse_sink,
                video_format_fourcc: legacy.video_format_fourcc,
                crt_filter: legacy.crt_filter,
                scaler_filter: legacy.scaler_filter,
                color_range: legacy.color_range,
                pixelate_filter_enabled: legacy.pixelate_filter_enabled,
                crt_hard_scan: legacy.crt_hard_scan,
                crt_warp_x: legacy.crt_warp_x,
                crt_warp_y: legacy.crt_warp_y,
                crt_shadow_mask: legacy.crt_shadow_mask,
                crt_brightboost: legacy.crt_brightboost,
                crt_hard_bloom_pix: legacy.crt_hard_bloom_pix,
                crt_hard_bloom_scan: legacy.crt_hard_bloom_scan,
                crt_bloom_amount: legacy.crt_bloom_amount,
                crt_shape: legacy.crt_shape,
                crt_hard_pix: legacy.crt_hard_pix,
                use_magenta_background: legacy.use_magenta_background,
                horizontal_stretch: legacy.horizontal_stretch,
                median_filter_enabled: legacy.median_filter_enabled,
                vibrance: legacy.vibrance,
            };
            profiles.insert("Default".to_string(), legacy_profile);
            active_profile = "Default".to_string();
        }

        MichadameConfig {
            video_device: legacy.video_device,
            usb_device: legacy.usb_device,
            video_resolution: legacy.video_resolution,
            video_framerate: legacy.video_framerate,
            reset_usb_on_startup: legacy.reset_usb_on_startup,
            has_shown_first_run_warning: legacy.has_shown_first_run_warning,
            active_profile,
            profiles,
        }
    }
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
        }
    }
}

pub fn build_profile_from_state(state: &AppState) -> Profile {
    Profile {
        pulse_source: state.hardware.selected_pulse_source_name.clone(),
        pulse_sink: state.hardware.selected_pulse_sink_name.clone(),
        video_format_fourcc: state.hardware
            .supported_formats
            .get(state.hardware.selected_format_index)
            .map(|f| f.fourcc.clone()),
        crt_filter: Some(state.crt_filter.load(Ordering::Relaxed)),
        scaler_filter: Some(state.scaler_filter.load(Ordering::Relaxed)),
        color_range: Some(state.color_range.load(Ordering::Relaxed)),
        pixelate_filter_enabled: Some(state.video.pixelate_filter_enabled),

        crt_hard_scan: Some(state.crt.hard_scan),
        crt_warp_x: Some(state.crt.warp_x),
        crt_warp_y: Some(state.crt.warp_y),
        crt_shadow_mask: Some(state.crt.shadow_mask),
        crt_brightboost: Some(state.crt.brightboost),
        crt_hard_bloom_pix: Some(state.crt.hard_bloom_pix),
        crt_hard_bloom_scan: Some(state.crt.hard_bloom_scan),
        crt_bloom_amount: Some(state.crt.bloom_amount),
        crt_shape: Some(state.crt.shape),
        crt_hard_pix: Some(state.crt.hard_pix),
        use_magenta_background: Some(state.video.use_magenta_background),
        horizontal_stretch: Some(state.video.horizontal_stretch),
        median_filter_enabled: Some(state.video.median_filter_enabled),
        vibrance: Some(state.video.vibrance),
    }
}

pub fn save_config(state: &AppState) {
    let mut cfg = match confy::load::<MichadameConfig>("michadame", None) {
        Ok(c) => c,
        Err(_) => MichadameConfig::default(),
    };

    // Update global settings
    cfg.video_device = Some(state.hardware.selected_video_device.clone());
    cfg.usb_device = state.hardware.selected_usb_device.clone();
    cfg.video_resolution = if state.hardware.selected_resolution.0 > 0 {
        Some(state.hardware.selected_resolution)
    } else {
        None
    };
    cfg.video_framerate = if state.hardware.selected_framerate > 0 {
        Some(state.hardware.selected_framerate)
    } else {
        None
    };
    cfg.reset_usb_on_startup = Some(state.ui.reset_usb_on_startup);
    cfg.has_shown_first_run_warning = Some(!state.ui.show_first_run_dialog);

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

pub fn save_global_hardware_config(state: &AppState) {
    let mut cfg = match confy::load::<MichadameConfig>("michadame", None) {
        Ok(c) => c,
        Err(_) => MichadameConfig::default(),
    };

    // Update global settings
    cfg.video_device = Some(state.hardware.selected_video_device.clone());
    cfg.usb_device = state.hardware.selected_usb_device.clone();
    cfg.video_resolution = if state.hardware.selected_resolution.0 > 0 {
        Some(state.hardware.selected_resolution)
    } else {
        None
    };
    cfg.video_framerate = if state.hardware.selected_framerate > 0 {
        Some(state.hardware.selected_framerate)
    } else {
        None
    };
    cfg.reset_usb_on_startup = Some(state.ui.reset_usb_on_startup);
    cfg.has_shown_first_run_warning = Some(!state.ui.show_first_run_dialog);

    cfg.active_profile = state.active_profile.clone();
    
    // We intentionally DO NOT update the `active_profile` data inline here. 
    // This allows Michadame to save hardware settings independently without touching UI filters.
    cfg.profiles = state.profiles.clone();

    if let Err(e) = confy::store("michadame", None, cfg) {
        tracing::error!("Failed to save global hardware configuration: {}", e);
    }
}

pub fn apply_profile_to_state(state: &mut AppState, profile: &Profile) {
    if let Some(saved_source) = &profile.pulse_source {
        if state
            .hardware.pulse_sources
            .iter()
            .any(|(_, name)| name == saved_source)
        {
            state.hardware.selected_pulse_source_name = Some(saved_source.clone());
        }
    }
    if let Some(saved_sink) = &profile.pulse_sink {
        if state.hardware.pulse_sinks.iter().any(|(_, name)| name == saved_sink) {
            state.hardware.selected_pulse_sink_name = Some(saved_sink.clone());
        }
    }
    if let Some(filter) = profile.crt_filter {
        state.crt_filter.store(filter, Ordering::Relaxed);
    }
    if let Some(s) = profile.scaler_filter {
        state.scaler_filter.store(s, Ordering::Relaxed);
    }
    if let Some(c) = profile.color_range {
        state.color_range.store(c, Ordering::Relaxed);
    }
    if let Some(val) = profile.pixelate_filter_enabled {
        state.video.pixelate_filter_enabled = val;
    }
    if let Some(val) = profile.crt_hard_scan {
        state.crt.hard_scan = val;
    }
    if let Some(val) = profile.crt_hard_pix {
        state.crt.hard_pix = val;
    }
    if let Some(val) = profile.crt_brightboost {
        state.crt.brightboost = val;
    }
    if let Some(val) = profile.crt_warp_x {
        state.crt.warp_x = val;
    }
    if let Some(val) = profile.crt_warp_y {
        state.crt.warp_y = val;
    }
    if let Some(val) = profile.crt_shadow_mask {
        state.crt.shadow_mask = val;
    }
    if let Some(val) = profile.crt_hard_bloom_pix {
        state.crt.hard_bloom_pix = val;
    }
    if let Some(val) = profile.crt_hard_bloom_scan {
        state.crt.hard_bloom_scan = val;
    }
    if let Some(val) = profile.crt_bloom_amount {
        state.crt.bloom_amount = val;
    }
    if let Some(val) = profile.crt_shape {
        state.crt.shape = val;
    }
    if let Some(val) = profile.use_magenta_background {
        state.video.use_magenta_background = val;
    }
    if let Some(val) = profile.horizontal_stretch {
        state.video.horizontal_stretch = val;
    }
    if let Some(val) = profile.median_filter_enabled {
        state.video.median_filter_enabled = val;
    }
    if let Some(val) = profile.vibrance {
        state.video.vibrance = val;
    }
}

pub fn apply_config(state: &mut AppState, cfg: &MichadameConfig) {
    state.profiles = cfg.profiles.clone();
    state.active_profile = cfg.active_profile.clone();

    if let Some(saved_device) = &cfg.video_device {
        if state.hardware.video_devices.contains(saved_device) {
            state.hardware.selected_video_device = saved_device.clone();
        }
    }
    if let Some(saved_usb) = &cfg.usb_device {
        if state.hardware.usb_devices.iter().any(|(id, _)| id == saved_usb) {
            state.hardware.selected_usb_device = Some(saved_usb.clone());
        }
    }

    if !state.hardware.selected_video_device.is_empty() {
        crate::video::types::apply_saved_format_config(state, cfg);
    }
    state.ui.reset_usb_on_startup = cfg.reset_usb_on_startup.unwrap_or(false);
    if state.ui.reset_usb_on_startup {
        if let Some(device_to_reset) = &state.hardware.selected_usb_device {
            state.toasts.add(egui_toast::Toast { kind: egui_toast::ToastKind::Info, options: egui_toast::ToastOptions::default().duration(std::time::Duration::from_secs(2)), text: (match crate::devices::usb::reset_usb_device(device_to_reset) {
                Ok(_) => "Auto-reset USB device successfully.".to_string(),
                Err(e) => format!("Failed to auto-reset USB: {}", e),
            }).into() });
            tracing::info!("USB device reset on startup as requested.");
        }
    }
    if !cfg.has_shown_first_run_warning.unwrap_or(false) {
        state.ui.show_first_run_dialog = true;
    }

    state.active_profile = cfg.active_profile.clone();

    // Apply active profile
    let profile_to_apply = state.profiles.get(&state.active_profile).cloned();
    if let Some(profile) = profile_to_apply {
        apply_profile_to_state(state, &profile);
    }
}
