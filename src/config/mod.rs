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
                retro_pc_frame: legacy.retro_pc_frame,
                crt_glass_enabled: legacy.crt_glass_enabled,
                crt_glass_intensity: legacy.crt_glass_intensity,
                crt_glass_glossiness: legacy.crt_glass_glossiness,
                horizontal_stretch: legacy.horizontal_stretch,
                median_filter_enabled: legacy.median_filter_enabled,
                median_mix: legacy.median_mix,
                vibrance: legacy.vibrance,
                overscan_x: legacy.overscan_x,
                overscan_y: legacy.overscan_y,
                border_crop_left: None,
                border_crop_right: None,
                border_crop_top: None,
                border_crop_bottom: None,
                selected_crt_filter: legacy.selected_crt_filter,
                halo_brightboost: legacy.halo_brightboost,
                halo_brightboost1: legacy.halo_brightboost1,
                halo_beam_min: legacy.halo_beam_min,
                halo_beam_max: legacy.halo_beam_max,
                halo_beam_size: legacy.halo_beam_size,
                halo_h_sharp: legacy.halo_h_sharp,
                halo_glow: legacy.halo_glow,
                halo_bloom: legacy.halo_bloom,
                halo_halation: legacy.halo_halation,
                halo_shadow_mask: legacy.halo_shadow_mask,
                halo_masksize: legacy.halo_masksize,
                halo_maskstr: legacy.halo_maskstr,
                halo_mcut: legacy.halo_mcut,
                halo_slotmask: legacy.halo_slotmask,
                halo_slotmask1: legacy.halo_slotmask1,
                halo_double_slot: legacy.halo_double_slot,
                halo_smoothmask: legacy.halo_smoothmask,
                halo_zoom: legacy.halo_zoom,
                halo_intensity: legacy.halo_intensity,
                halo_corner_size: legacy.halo_corner_size,
                halo_curvature: legacy.halo_curvature,
                cathode_interference_enabled: legacy.cathode_interference_enabled,
                cathode_intensity: legacy.cathode_intensity,
                cathode_frequency: legacy.cathode_frequency,
                cathode_randomization: legacy.cathode_randomization,
                cathode_electricity_glow: legacy.cathode_electricity_glow,
                cathode_flicker_depth: legacy.cathode_flicker_depth,
                cathode_interference: legacy.cathode_interference,
                cathode_lightbulb_effect: legacy.cathode_lightbulb_effect,
            };
            profiles.insert("Default".to_string(), legacy_profile);
            active_profile = "Default".to_string();
        }

        MichadameConfig {
            replay: legacy.replay,
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
            ocr_sticky_distance: legacy.ocr_sticky_distance,
            ocr_hide_overlay: legacy.ocr_hide_overlay,
            ocr_timeout_seconds: legacy.ocr_timeout_seconds,
            default_halo: legacy.default_halo,
            default_cathode_interference: legacy.default_cathode_interference,
            bank_current_tag: legacy.bank_current_tag,
            bank_compact_mode: legacy.bank_compact_mode,
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
            replay: Default::default(),
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
            retro_pc_frame: Some(false),
            crt_glass_enabled: Some(false),
            crt_glass_intensity: Some(0.25),
            crt_glass_glossiness: Some(0.50),
            horizontal_stretch: Some(1.0),
            median_filter_enabled: Some(false),
            median_mix: Some(1.0),
            vibrance: Some(1.0),
            overscan_x: Some(0.0),
            overscan_y: Some(0.0),
            selected_crt_filter: Some(2),
            halo_brightboost: Some(1.9),
            halo_brightboost1: Some(1.8),
            halo_beam_min: Some(1.6),
            halo_beam_max: Some(0.85),
            halo_beam_size: Some(0.85),
            halo_h_sharp: Some(3.5),
            halo_glow: Some(0.1),
            halo_bloom: Some(0.15),
            halo_halation: Some(0.03),
            halo_shadow_mask: Some(6.0),
            halo_masksize: Some(3.0),
            halo_maskstr: Some(0.35),
            halo_mcut: Some(0.55),
            halo_slotmask: Some(0.15),
            halo_slotmask1: Some(0.3),
            halo_double_slot: Some(2.0),
            halo_smoothmask: Some(1.0),
            halo_zoom: Some(85.0),
            halo_intensity: Some(0.75),
            halo_corner_size: Some(0.02),
            halo_curvature: Some(true),
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
        };

        let config: MichadameConfig = MichadameConfig::from(legacy);
        assert_eq!(config.video_device, Some("/dev/video0".to_string()));
        assert_eq!(config.default_halo, None);
        assert_eq!(config.profiles.len(), 1);
        let profile = config.profiles.get("Default").unwrap();
        assert_eq!(profile.video_format_fourcc, Some("MJPG".to_string()));
        assert_eq!(profile.crt_filter, Some(1));
        assert_eq!(profile.selected_crt_filter, Some(2));
        assert_eq!(profile.halo_brightboost, Some(1.9));
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
            replay: Default::default(),
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
            crt_glass_enabled: None,
            crt_glass_intensity: None,
            crt_glass_glossiness: None,
            horizontal_stretch: None,
            median_filter_enabled: None,
            median_mix: None,
            vibrance: None,
            overscan_x: None,
            overscan_y: None,
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
