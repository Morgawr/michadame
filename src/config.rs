use crate::{app::AppState, devices, video::types as video_types};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;

#[derive(Default, Serialize, Deserialize, Clone)]
pub struct MichadameConfig {
    pub video_device: Option<String>,
    pub usb_device: Option<String>,
    pub pulse_source: Option<String>,
    pub pulse_sink: Option<String>,
    pub video_format_fourcc: Option<String>,
    pub video_resolution: Option<(u32, u32)>,
    pub video_framerate: Option<u32>,
    pub reset_usb_on_startup: Option<bool>,
    pub has_shown_first_run_warning: Option<bool>, // Add this line
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
}

pub fn save_config(state: &AppState) {
    let cfg = MichadameConfig {
        video_device: Some(state.selected_video_device.clone()),
        usb_device: state.selected_usb_device.clone(),
        pulse_source: state.selected_pulse_source_name.clone(),
        pulse_sink: state.selected_pulse_sink_name.clone(),
        video_format_fourcc: state
            .supported_formats
            .get(state.selected_format_index)
            .map(|f| f.fourcc.clone()),
        video_resolution: if state.selected_resolution.0 > 0 {
            Some(state.selected_resolution)
        } else {
            None
        },
        video_framerate: if state.selected_framerate > 0 { Some(state.selected_framerate) } else { None },
        reset_usb_on_startup: Some(state.reset_usb_on_startup),
        has_shown_first_run_warning: Some(!state.show_first_run_dialog),
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
    };

    if let Err(e) = confy::store("michadame", None, cfg) {
        tracing::error!("Failed to save configuration: {}", e);
    }
}

pub fn apply_config(state: &mut AppState, cfg: &MichadameConfig) {
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
    if let Some(saved_source) = &cfg.pulse_source {
        if state.pulse_sources.iter().any(|(_, name)| name == saved_source) {
            state.selected_pulse_source_name = Some(saved_source.clone());
        }
    }
    if let Some(saved_sink) = &cfg.pulse_sink {
        if state.pulse_sinks.iter().any(|(_, name)| name == saved_sink) {
            state.selected_pulse_sink_name = Some(saved_sink.clone());
        }
    }
    if !state.selected_video_device.is_empty() {
        video_types::apply_saved_format_config(state, cfg);
    }
    state.reset_usb_on_startup = cfg.reset_usb_on_startup.unwrap_or(false);
    if state.reset_usb_on_startup {
        if let Some(device_to_reset) = &state.selected_usb_device {
            state.status_message = match devices::usb::reset_usb_device(device_to_reset) {
                Ok(_) => "Auto-reset USB device successfully.".to_string(),
                Err(e) => format!("Failed to auto-reset USB: {}", e),
            };
            tracing::info!("USB device reset on startup as requested.");
        }
    }
    if !cfg.has_shown_first_run_warning.unwrap_or(false) {
        state.show_first_run_dialog = true;
    }
    if let Some(filter) = cfg.crt_filter {
        state.crt_filter.store(filter, Ordering::Relaxed);
    }
    if let Some(val) = cfg.pixelate_filter_enabled {
        state.pixelate_filter_enabled = val;
    }
    if let Some(val) = cfg.crt_hard_scan {
        state.crt_hard_scan = val;
    }
    if let Some(val) = cfg.crt_hard_pix {
        state.crt_hard_pix = val;
    }
    if let Some(val) = cfg.crt_brightboost {
        state.crt_brightboost = val;
    }
    if let Some(val) = cfg.crt_warp_x {
        state.crt_warp_x = val;
    }
    if let Some(val) = cfg.crt_warp_y {
        state.crt_warp_y = val;
    }
    if let Some(val) = cfg.crt_shadow_mask {
        state.crt_shadow_mask = val;
    }
    if let Some(val) = cfg.crt_hard_bloom_pix {
        state.crt_hard_bloom_pix = val;
    }
    if let Some(val) = cfg.crt_hard_bloom_scan {
        state.crt_hard_bloom_scan = val;
    }
    if let Some(val) = cfg.crt_bloom_amount {
        state.crt_bloom_amount = val;
    }
    if let Some(val) = cfg.crt_shape {
        state.crt_shape = val;
    }
}