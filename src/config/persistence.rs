use crate::app::models::AppState;
use super::models::{MichadameConfig, Profile};
use std::sync::atomic::Ordering;

pub fn build_profile_from_state(state: &AppState) -> Profile {
    Profile {
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
        median_mix: Some(state.video.median_mix),
        vibrance: Some(state.video.vibrance),
        overscan_x: Some(state.video.overscan_x),
        overscan_y: Some(state.video.overscan_y),
    }
}

pub fn save_config(state: &AppState) {
    let mut cfg = match confy::load::<MichadameConfig>("michadame", None) {
        Ok(c) => c,
        Err(_) => MichadameConfig::default(),
    };

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
    cfg.audio_source = state.hardware.selected_audio_source_name.clone();
    cfg.audio_buffer_size = Some(state.hardware.audio_buffer_size);
    cfg.audio_sample_rate = Some(state.hardware.audio_sample_rate);
    cfg.audio_sample_format = Some(state.hardware.audio_sample_format.clone());

    cfg.active_profile = state.active_profile.clone();
    cfg.profiles = state.profiles.clone();

    let current_profile_data = build_profile_from_state(state);
    cfg.profiles.insert(state.active_profile.clone(), current_profile_data);

    if let Err(e) = confy::store("michadame", None, cfg) {
        tracing::error!("Failed to save configuration: {}", e);
    }
}

pub fn save_global_hardware_config(state: &AppState) {
    let mut cfg = confy::load::<MichadameConfig>("michadame", None).unwrap_or_default();

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
    cfg.audio_source = state.hardware.selected_audio_source_name.clone();
    cfg.audio_buffer_size = Some(state.hardware.audio_buffer_size);
    cfg.audio_sample_rate = Some(state.hardware.audio_sample_rate);
    cfg.audio_sample_format = Some(state.hardware.audio_sample_format.clone());

    cfg.active_profile = state.active_profile.clone();
    cfg.profiles = state.profiles.clone();

    if let Err(e) = confy::store("michadame", None, cfg) {
        tracing::error!("Failed to save global hardware configuration: {}", e);
    }
}

pub fn apply_profile_to_state(state: &mut AppState, profile: &Profile) {
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
    if let Some(val) = profile.median_mix {
        state.video.median_mix = val;
    }
    if let Some(val) = profile.vibrance {
        state.video.vibrance = val;
    }
    if let Some(val) = profile.overscan_x {
        state.video.overscan_x = val;
    }
    if let Some(val) = profile.overscan_y {
        state.video.overscan_y = val;
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
    if let Some(saved_source) = &cfg.audio_source {
        if state.hardware.audio_sources.iter().any(|(_, name)| name == saved_source) {
            state.hardware.selected_audio_source_name = Some(saved_source.clone());
        }
    }
    state.hardware.audio_buffer_size = cfg.audio_buffer_size.unwrap_or(1024);
    state.hardware.audio_sample_rate = cfg.audio_sample_rate.unwrap_or(48000);
    state.hardware.audio_sample_format = cfg.audio_sample_format.clone().unwrap_or_else(|| "S16LE".to_string());

    if !state.hardware.selected_video_device.is_empty() {
        crate::video::types::apply_saved_format_config(state, cfg);
    }
    state.ui.reset_usb_on_startup = cfg.reset_usb_on_startup.unwrap_or(false);
    if state.ui.reset_usb_on_startup {
        if let Some(device_to_reset) = &state.hardware.selected_usb_device {
            let msg = match crate::devices::usb::reset_usb_device(device_to_reset) {
                Ok(_) => "Auto-reset USB device successfully.".to_string(),
                Err(e) => format!("Failed to auto-reset USB: {}", e),
            };
            state.info(msg);
            tracing::info!("USB device reset on startup as requested.");
        }
    }
    if !cfg.has_shown_first_run_warning.unwrap_or(false) {
        state.ui.show_first_run_dialog = true;
    }

    state.active_profile = cfg.active_profile.clone();

    let profile_to_apply = state.profiles.get(&state.active_profile).cloned();
    if let Some(profile) = profile_to_apply {
        apply_profile_to_state(state, &profile);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::models::AppState;
    use crate::config::models::{MichadameConfig, Profile};

    #[test]
    fn test_build_profile_from_state() {
        let mut state = AppState::default();
        state.crt.hard_scan = -12.0;
        state.video.pixelate_filter_enabled = true;
        state.crt_filter.store(crate::devices::filter_type::CrtFilter::Lottes as u8, Ordering::Relaxed);

        let profile = build_profile_from_state(&state);
        assert_eq!(profile.crt_hard_scan, Some(-12.0));
        assert_eq!(profile.pixelate_filter_enabled, Some(true));
        assert_eq!(profile.crt_filter, Some(crate::devices::filter_type::CrtFilter::Lottes as u8));
    }

    #[test]
    fn test_apply_profile_to_state() {
        let mut state = AppState::default();
        let mut profile = Profile::default();
        profile.crt_hard_scan = Some(-15.0);
        profile.pixelate_filter_enabled = Some(true);
        profile.crt_filter = Some(crate::devices::filter_type::CrtFilter::Lottes as u8);

        apply_profile_to_state(&mut state, &profile);
        assert_eq!(state.crt.hard_scan, -15.0);
        assert_eq!(state.video.pixelate_filter_enabled, true);
        assert_eq!(state.crt_filter.load(Ordering::Relaxed), crate::devices::filter_type::CrtFilter::Lottes as u8);
    }

    #[test]
    fn test_apply_config_hardware_settings() {
        let mut state = AppState::default();
        state.hardware.video_devices = vec!["/dev/video0".to_string()];
        state.hardware.audio_sources = vec![("id".to_string(), "Mic".to_string())];

        let mut cfg = MichadameConfig::default();
        cfg.video_device = Some("/dev/video0".to_string());
        cfg.audio_source = Some("Mic".to_string());
        cfg.audio_buffer_size = Some(2048);

        apply_config(&mut state, &cfg);
        assert_eq!(state.hardware.selected_video_device, "/dev/video0");
        assert_eq!(state.hardware.selected_audio_source_name, Some("Mic".to_string()));
        assert_eq!(state.hardware.audio_buffer_size, 2048);
    }

    #[test]
    fn test_apply_config_with_missing_hardware() {
        let mut state = AppState::default();
        state.hardware.video_devices = vec![]; // No devices found during scan

        let mut cfg = MichadameConfig::default();
        cfg.video_device = Some("/dev/video0".to_string()); // Saved device not present

        apply_config(&mut state, &cfg);
        // Should NOT update if not in list
        assert_eq!(state.hardware.selected_video_device, "");
    }
}
