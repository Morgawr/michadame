pub mod fft_masks;
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
                median_mix: legacy.median_mix,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn test_legacy_config_conversion() {
        let legacy = LegacyConfig {
            video_device: Some("/dev/video0".to_string()),
            usb_device: None,
            video_resolution: Some((640, 480)),
            video_framerate: Some(60),
            reset_usb_on_startup: Some(true),
            has_shown_first_run_warning: Some(true),
            active_profile: "Default".to_string(),
            profiles: BTreeMap::new(),
            audio_source: Some("mic".to_string()),
            video_format_fourcc: Some("MJPG".to_string()),
            crt_filter: Some(1),
            scaler_filter: Some(2),
            color_range: Some(0),
            pixelate_filter_enabled: Some(true),
            audio_buffer_size: Some(1024),
            audio_sample_rate: Some(48000),
            audio_sample_format: Some("S16LE".to_string()),
            crt_hard_scan: Some(-8.0),
            crt_warp_x: Some(0.031),
            crt_warp_y: Some(0.041),
            crt_shadow_mask: Some(3.0),
            crt_brightboost: Some(1.0),
            crt_hard_bloom_pix: Some(-1.5),
            crt_hard_bloom_scan: Some(-2.0),
            crt_bloom_amount: Some(0.15),
            crt_shape: Some(2.0),
            crt_hard_pix: Some(-3.0),
            use_magenta_background: Some(false),
            horizontal_stretch: Some(1.0),
            median_filter_enabled: Some(false),
            median_mix: Some(1.0),
            vibrance: Some(1.0),
            overscan_x: Some(0.0),
            overscan_y: Some(0.0),
        };

        let config: MichadameConfig = MichadameConfig::from(legacy);
        assert_eq!(config.video_device, Some("/dev/video0".to_string()));
        assert_eq!(config.profiles.len(), 1);
        let profile = config.profiles.get("Default").unwrap();
        assert_eq!(profile.video_format_fourcc, Some("MJPG".to_string()));
        assert_eq!(profile.crt_filter, Some(1));
        assert_eq!(profile.pixelate_filter_enabled, Some(true));
        assert_eq!(profile.crt_hard_scan, Some(-8.0));
        assert_eq!(profile.crt_warp_x, Some(0.031));
    }

    #[test]
    fn test_legacy_config_with_existing_profiles() {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            "Custom".to_string(),
            Profile {
                crt_hard_scan: Some(-10.0),
                ..Default::default()
            },
        );

        let legacy = LegacyConfig {
            active_profile: "Custom".to_string(),
            profiles,
            video_device: None,
            usb_device: None,
            video_resolution: None,
            video_framerate: None,
            reset_usb_on_startup: None,
            has_shown_first_run_warning: None,
            audio_source: None,
            video_format_fourcc: None,
            crt_filter: None,
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
            horizontal_stretch: None,
            median_filter_enabled: None,
            median_mix: None,
            vibrance: None,
            overscan_x: None,
            overscan_y: None,
        };

        let config: MichadameConfig = MichadameConfig::from(legacy);
        assert_eq!(config.active_profile, "Custom");
        assert_eq!(config.profiles.len(), 1);
        assert_eq!(
            config.profiles.get("Custom").unwrap().crt_hard_scan,
            Some(-10.0)
        );
    }
}
