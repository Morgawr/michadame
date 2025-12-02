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
    pub crt_gamma: Option<f32>,
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
        crt_filter: Some(state.crt_filter.load(Ordering::Relaxed)),
        crt_gamma: Some(state.crt_gamma),
        has_shown_first_run_warning: Some(!state.show_first_run_dialog),
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
    if let Some(gamma) = cfg.crt_gamma {
        state.crt_gamma = gamma;
    }
}