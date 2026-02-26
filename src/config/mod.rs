pub mod models;
pub mod persistence;

pub use models::*;
pub use persistence::*;

impl From<LegacyConfig> for MichadameConfig {
    fn from(legacy: LegacyConfig) -> Self {
        let mut profiles = legacy.profiles;
        let mut active_profile = legacy.active_profile;

        if profiles.is_empty() {
            let legacy_profile = Profile {
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
                overscan_x: legacy.overscan_x,
                overscan_y: legacy.overscan_y,
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
            audio_source: legacy.audio_source,
            audio_buffer_size: legacy.audio_buffer_size,
            audio_sample_rate: legacy.audio_sample_rate,
            audio_sample_format: legacy.audio_sample_format,
            active_profile,
            profiles,
        }
    }
}
