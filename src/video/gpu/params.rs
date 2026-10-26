use crate::app::AppState;
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;

#[derive(Clone, Serialize, Deserialize)]
pub struct ShaderParams {
    pub hard_scan: f32,
    pub warp_x: f32,
    pub warp_y: f32,
    pub shadow_mask: f32,
    pub brightboost: f32,
    pub hard_bloom_pix: f32,
    pub hard_bloom_scan: f32,
    pub bloom_amount: f32,
    pub shape: f32,
    pub hard_pix: f32,
    pub background_color: [f32; 3],
    pub horizontal_stretch: f32,
    pub median_filter_enabled: bool,
    pub median_mix: f32,
    pub deinterlace_filter_enabled: bool,
    pub deinterlace_mode: u8,
    pub deinterlace_blend: f32,
    pub deinterlace_motion_threshold: f32,
    pub deinterlace_line_spacing: f32,
    pub deinterlace_spatial_mix: f32,
    pub vibrance: f32,
    pub scaler_filter: u8,
    pub overscan_x: f32,
    pub overscan_y: f32,
    pub underscan_x: f32,
    pub underscan_y: f32,
    pub border_crop: [f32; 4],
}

#[derive(Clone, Serialize, Deserialize)]
pub struct HaloShaderParams {
    pub brightboost: f32,
    pub brightboost1: f32,
    pub beam_min: f32,
    pub beam_max: f32,
    pub beam_size: f32,
    pub h_sharp: f32,
    pub glow: f32,
    pub bloom: f32,
    pub halation: f32,
    pub shadow_mask: f32,
    pub masksize: f32,
    pub maskstr: f32,
    pub mcut: f32,
    pub slotmask: f32,
    pub slotmask1: f32,
    pub double_slot: f32,
    pub smoothmask: f32,
    pub halo_zoom: f32,
    pub halo_intensity: f32,
    pub corner_size: f32,
    pub curvature: bool,
    pub background_color: [f32; 3],
    pub horizontal_stretch: f32,
    pub vibrance: f32,
    pub border_crop: [f32; 4],
}

impl HaloShaderParams {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            brightboost: state.halo.brightboost,
            brightboost1: state.halo.brightboost1,
            beam_min: state.halo.beam_min,
            beam_max: state.halo.beam_max,
            beam_size: state.halo.beam_size,
            h_sharp: state.halo.h_sharp,
            glow: state.halo.glow,
            bloom: state.halo.bloom,
            halation: state.halo.halation,
            shadow_mask: state.halo.shadow_mask,
            masksize: state.halo.masksize,
            maskstr: state.halo.maskstr,
            mcut: state.halo.mcut,
            slotmask: state.halo.slotmask,
            slotmask1: state.halo.slotmask1,
            double_slot: state.halo.double_slot,
            smoothmask: state.halo.smoothmask,
            halo_zoom: state.halo.halo_zoom,
            halo_intensity: state.halo.halo_intensity,
            corner_size: state.halo.corner_size,
            curvature: state.halo.curvature,
            background_color: if state.video.use_magenta_background {
                [1.0, 0.0, 1.0]
            } else {
                [0.0, 0.0, 0.0]
            },
            horizontal_stretch: state.video.horizontal_stretch,
            vibrance: state.video.vibrance,
            border_crop: [
                state.video.border_crop_left,
                state.video.border_crop_right,
                state.video.border_crop_top,
                state.video.border_crop_bottom,
            ],
        }
    }
}

impl Default for HaloShaderParams {
    fn default() -> Self {
        Self {
            brightboost: 1.30,
            brightboost1: 1.65,
            beam_min: 1.80,
            beam_max: 1.65,
            beam_size: 1.25,
            h_sharp: 4.20,
            glow: 0.53,
            bloom: 0.57,
            halation: 0.23,
            shadow_mask: 6.0,
            masksize: 2.0,
            maskstr: 0.50,
            mcut: 0.80,
            slotmask: 0.45,
            slotmask1: 0.30,
            double_slot: 3.0,
            smoothmask: 0.80,
            halo_zoom: 100.0,
            halo_intensity: 0.95,
            corner_size: 0.0,
            curvature: true,
            background_color: [0.0, 0.0, 0.0],
            horizontal_stretch: 1.0,
            vibrance: 1.0,
            border_crop: [0.0, 0.0, 0.0, 0.0],
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CathodeInterferenceShaderParams {
    pub enabled: bool,
    pub intensity: f32,
    pub frequency: f32,
    pub randomization: f32,
    pub electricity_glow: f32,
    pub flicker_depth: f32,
    pub interference: f32,
    pub lightbulb_effect: f32,
    pub border_crop: [f32; 4],
}

impl CathodeInterferenceShaderParams {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            enabled: state.cathode_interference.enabled,
            intensity: state.cathode_interference.intensity,
            frequency: state.cathode_interference.frequency,
            randomization: state.cathode_interference.randomization,
            electricity_glow: state.cathode_interference.electricity_glow,
            flicker_depth: state.cathode_interference.flicker_depth,
            interference: state.cathode_interference.interference,
            lightbulb_effect: state.cathode_interference.lightbulb_effect,
            border_crop: [
                state.video.border_crop_left,
                state.video.border_crop_right,
                state.video.border_crop_top,
                state.video.border_crop_bottom,
            ],
        }
    }
}

impl Default for CathodeInterferenceShaderParams {
    fn default() -> Self {
        Self {
            enabled: false,
            intensity: 0.5,
            frequency: 1.0,
            randomization: 0.5,
            electricity_glow: 0.5,
            flicker_depth: 0.4,
            interference: 0.3,
            lightbulb_effect: 0.35,
            border_crop: [0.0, 0.0, 0.0, 0.0],
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AudioReactiveShaderParams {
    pub enabled: bool,
    pub master_intensity: f32,
    pub low_env: f32,
    pub mid_env: f32,
    pub high_env: f32,
    pub transient_env: f32,
    pub stereo_balance: f32,
    pub spring_pos: [f32; 2],
    pub phase_accum: f32,
    pub deflection_shake: f32,
    pub stereo_tug: f32,
    pub flyback_pump: f32,
    pub rgb_deconvergence: f32,
    pub scanline_wave: f32,
    pub phosphor_surge: f32,
    pub sync_tear: f32,
    pub warp: [f32; 2],
    pub corner_size: f32,
    pub filter_type: i32,
    pub border_crop: [f32; 4],
    pub horizontal_stretch: f32,
}

impl AudioReactiveShaderParams {
    pub fn from_state(
        state: &AppState,
        warp: [f32; 2],
        corner_size: f32,
        filter_type: i32,
    ) -> Self {
        let cur = &state.audio_reactive.current;
        let rt = &state.audio_reactive_runtime;
        Self {
            enabled: state.audio_reactive.enabled,
            master_intensity: cur.master_intensity,
            low_env: rt.low_env,
            mid_env: rt.mid_env,
            high_env: rt.high_env,
            transient_env: rt.transient_env,
            stereo_balance: rt.stereo_balance,
            spring_pos: [rt.spring_pos_x, rt.spring_pos_y],
            phase_accum: rt.phase_accum,
            deflection_shake: cur.deflection_shake,
            stereo_tug: cur.stereo_tug,
            flyback_pump: cur.flyback_pump,
            rgb_deconvergence: cur.rgb_deconvergence,
            scanline_wave: cur.scanline_wave,
            phosphor_surge: cur.phosphor_surge,
            sync_tear: cur.sync_tear,
            warp,
            corner_size,
            filter_type,
            border_crop: [
                state.video.border_crop_left,
                state.video.border_crop_right,
                state.video.border_crop_top,
                state.video.border_crop_bottom,
            ],
            horizontal_stretch: state.video.horizontal_stretch,
        }
    }
}

impl Default for AudioReactiveShaderParams {
    fn default() -> Self {
        Self {
            enabled: false,
            master_intensity: 1.0,
            low_env: 0.0,
            mid_env: 0.0,
            high_env: 0.0,
            transient_env: 0.0,
            stereo_balance: 0.0,
            spring_pos: [0.0, 0.0],
            phase_accum: 0.0,
            deflection_shake: 0.65,
            stereo_tug: 0.50,
            flyback_pump: 0.55,
            rgb_deconvergence: 0.45,
            scanline_wave: 0.35,
            phosphor_surge: 0.42,
            sync_tear: 0.22,
            warp: [0.0, 0.0],
            corner_size: 0.0,
            filter_type: 0,
            border_crop: [0.0, 0.0, 0.0, 0.0],
            horizontal_stretch: 1.0,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct GlassShaderParams {
    pub enabled: bool,
    pub intensity: f32,
    pub glossiness: f32,
    pub ceiling_light_enabled: bool,
    pub photographer_enabled: bool,
    pub photographer_intensity: f32,
    pub flash_enabled: bool,
    pub flash_intensity: f32,
    pub warp: [f32; 2],
    pub corner_size: f32,
    pub filter_type: i32,
    pub border_crop: [f32; 4],
    pub horizontal_stretch: f32,
}

impl GlassShaderParams {
    pub fn from_state(
        state: &AppState,
        warp: [f32; 2],
        corner_size: f32,
        filter_type: i32,
    ) -> Self {
        let crt_active = filter_type != 0;
        let night_mode = crt_active && state.video.effective_lights_off_night_mode();
        Self {
            enabled: crt_active && state.video.crt_glass_enabled,
            intensity: state.video.crt_glass_intensity,
            glossiness: state.video.crt_glass_glossiness,
            ceiling_light_enabled: crt_active
                && !night_mode
                && state.video.crt_glass_ceiling_light_enabled,
            photographer_enabled: crt_active
                && !night_mode
                && state.video.crt_glass_photographer_enabled,
            photographer_intensity: state.video.crt_glass_photographer_intensity,
            flash_enabled: crt_active && !night_mode && state.video.crt_glass_flash_enabled,
            flash_intensity: state.video.crt_glass_flash_intensity,
            warp,
            corner_size,
            filter_type,
            border_crop: [
                state.video.border_crop_left,
                state.video.border_crop_right,
                state.video.border_crop_top,
                state.video.border_crop_bottom,
            ],
            horizontal_stretch: state.video.horizontal_stretch,
        }
    }
}

impl Default for GlassShaderParams {
    fn default() -> Self {
        Self {
            enabled: false,
            intensity: 0.25,
            glossiness: 0.50,
            ceiling_light_enabled: true,
            photographer_enabled: false,
            photographer_intensity: 0.50,
            flash_enabled: false,
            flash_intensity: 0.70,
            warp: [0.0, 0.0],
            corner_size: 0.0,
            filter_type: 0,
            border_crop: [0.0, 0.0, 0.0, 0.0],
            horizontal_stretch: 1.0,
        }
    }
}

impl ShaderParams {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            hard_scan: state.crt.hard_scan,
            warp_x: state.crt.warp_x,
            warp_y: state.crt.warp_y,
            shadow_mask: state.crt.shadow_mask,
            brightboost: state.crt.brightboost,
            hard_bloom_pix: state.crt.hard_bloom_pix,
            hard_bloom_scan: state.crt.hard_bloom_scan,
            bloom_amount: state.crt.bloom_amount,
            shape: state.crt.shape,
            hard_pix: state.crt.hard_pix,
            background_color: if state.video.use_magenta_background {
                [1.0, 0.0, 1.0]
            } else {
                [0.0, 0.0, 0.0]
            },
            horizontal_stretch: state.video.horizontal_stretch,
            median_filter_enabled: state.video.median_filter_enabled,
            median_mix: state.video.median_mix,
            deinterlace_filter_enabled: state.video.deinterlace_filter_enabled,
            deinterlace_mode: state.video.deinterlace_mode,
            deinterlace_blend: state.video.deinterlace_blend,
            deinterlace_motion_threshold: state.video.deinterlace_motion_threshold,
            deinterlace_line_spacing: state.video.deinterlace_line_spacing,
            deinterlace_spatial_mix: state.video.deinterlace_spatial_mix,
            vibrance: state.video.vibrance,
            scaler_filter: state.scaler_filter.load(Ordering::Relaxed),
            overscan_x: state.video.overscan_x,
            overscan_y: state.video.overscan_y,
            underscan_x: state.video.underscan_x,
            underscan_y: state.video.underscan_y,
            border_crop: [
                state.video.border_crop_left,
                state.video.border_crop_right,
                state.video.border_crop_top,
                state.video.border_crop_bottom,
            ],
        }
    }
}

impl Default for ShaderParams {
    fn default() -> Self {
        Self {
            hard_scan: -8.0,
            warp_x: 0.031,
            warp_y: 0.041,
            shadow_mask: 3.0,
            brightboost: 1.0,
            hard_bloom_pix: -1.5,
            hard_bloom_scan: -2.0,
            bloom_amount: 0.15,
            shape: 2.0,
            hard_pix: -3.0,
            background_color: [0.0, 0.0, 0.0],
            horizontal_stretch: 1.0,
            median_filter_enabled: false,
            median_mix: 1.0,
            deinterlace_filter_enabled: false,
            deinterlace_mode: 0,
            deinterlace_blend: 0.5,
            deinterlace_motion_threshold: 0.08,
            deinterlace_line_spacing: 1.0,
            deinterlace_spatial_mix: 0.75,
            vibrance: 1.0,
            scaler_filter: crate::video::types::ScalerFilter::FastBilinear as u8,
            overscan_x: 0.0,
            overscan_y: 0.0,
            underscan_x: 0.0,
            underscan_y: 0.0,
            border_crop: [0.0, 0.0, 0.0, 0.0],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shader_params_default() {
        let params = ShaderParams::default();
        assert_eq!(params.hard_scan, -8.0);
        assert_eq!(params.warp_x, 0.031);
        assert_eq!(params.shadow_mask, 3.0);
        assert!(!params.deinterlace_filter_enabled);
        assert_eq!(params.deinterlace_mode, 0);
        assert_eq!(params.deinterlace_blend, 0.5);
    }

    #[test]
    fn test_shader_params_deinterlace_from_state() {
        let mut state = AppState::default();
        state.video.deinterlace_filter_enabled = true;
        state.video.deinterlace_mode = 2;
        state.video.deinterlace_blend = 0.75;
        state.video.deinterlace_motion_threshold = 0.2;
        state.video.deinterlace_line_spacing = 2.25;
        state.video.deinterlace_spatial_mix = 0.9;

        let params = ShaderParams::from_state(&state);
        assert!(params.deinterlace_filter_enabled);
        assert_eq!(params.deinterlace_mode, 2);
        assert_eq!(params.deinterlace_blend, 0.75);
        assert_eq!(params.deinterlace_motion_threshold, 0.2);
        assert_eq!(params.deinterlace_line_spacing, 2.25);
        assert_eq!(params.deinterlace_spatial_mix, 0.9);
    }

    #[test]
    fn test_shader_params_from_state() {
        let mut state = AppState::default();
        state.crt.hard_scan = -10.0;
        state.video.use_magenta_background = true;

        let params = ShaderParams::from_state(&state);
        assert_eq!(params.hard_scan, -10.0);
        assert_eq!(params.background_color, [1.0, 0.0, 1.0]);
    }

    #[test]
    fn test_shader_params_background_color_combinations() {
        let mut state = AppState::default();

        state.video.use_magenta_background = false;
        let params = ShaderParams::from_state(&state);
        assert_eq!(params.background_color, [0.0, 0.0, 0.0]);

        state.video.use_magenta_background = true;
        let params = ShaderParams::from_state(&state);
        assert_eq!(params.background_color, [1.0, 0.0, 1.0]);
    }

    #[test]
    fn test_shader_params_scaler_filter() {
        let state = AppState::default();
        state.scaler_filter.store(
            crate::video::types::ScalerFilter::Lanczos as u8,
            Ordering::Relaxed,
        );

        let params = ShaderParams::from_state(&state);
        assert_eq!(
            params.scaler_filter,
            crate::video::types::ScalerFilter::Lanczos as u8
        );
    }

    #[test]
    fn test_shader_params_underscan() {
        let mut state = AppState::default();
        state.video.underscan_x = 0.05;
        state.video.underscan_y = -0.03;

        let params = ShaderParams::from_state(&state);
        assert_eq!(params.underscan_x, 0.05);
        assert_eq!(params.underscan_y, -0.03);
    }

    #[test]
    fn test_halo_shader_params_default() {
        let params = HaloShaderParams::default();
        assert_eq!(params.brightboost, 1.30);
        assert_eq!(params.shadow_mask, 6.0);
        assert_eq!(params.halo_zoom, 100.0);
        assert_eq!(params.halo_intensity, 0.95);
        assert_eq!(params.corner_size, 0.0);
        assert!(params.curvature);
        assert_eq!(params.vibrance, 1.0);
    }

    #[test]
    fn test_halo_shader_params_from_state() {
        let mut state = AppState::default();
        state.halo.brightboost = 2.2;
        state.halo.halo_zoom = 90.0;
        state.video.use_magenta_background = true;
        state.video.vibrance = 1.5;

        let params = HaloShaderParams::from_state(&state);
        assert_eq!(params.brightboost, 2.2);
        assert_eq!(params.halo_zoom, 90.0);
        assert_eq!(params.background_color, [1.0, 0.0, 1.0]);
        assert_eq!(params.vibrance, 1.5);
    }

    #[test]
    fn test_cathode_interference_shader_params_default() {
        let params = CathodeInterferenceShaderParams::default();
        assert!(!params.enabled);
        assert_eq!(params.intensity, 0.5);
        assert_eq!(params.frequency, 1.0);
        assert_eq!(params.randomization, 0.5);
        assert_eq!(params.electricity_glow, 0.5);
        assert_eq!(params.flicker_depth, 0.4);
        assert_eq!(params.interference, 0.3);
        assert_eq!(params.lightbulb_effect, 0.35);
    }

    #[test]
    fn test_cathode_interference_shader_params_from_state() {
        let mut state = AppState::default();
        state.cathode_interference.enabled = true;
        state.cathode_interference.intensity = 0.8;
        state.cathode_interference.frequency = 2.5;
        state.cathode_interference.lightbulb_effect = 0.6;

        let params = CathodeInterferenceShaderParams::from_state(&state);
        assert!(params.enabled);
        assert_eq!(params.intensity, 0.8);
        assert_eq!(params.frequency, 2.5);
        assert_eq!(params.randomization, 0.5);
        assert_eq!(params.lightbulb_effect, 0.6);
    }

    #[test]
    fn test_shader_params_border_crop() {
        let mut state = AppState::default();
        state.video.border_crop_left = 0.05;
        state.video.border_crop_right = 0.06;
        state.video.border_crop_top = 0.07;
        state.video.border_crop_bottom = 0.08;

        let shader_params = ShaderParams::from_state(&state);
        assert_eq!(shader_params.border_crop, [0.05, 0.06, 0.07, 0.08]);

        let halo_params = HaloShaderParams::from_state(&state);
        assert_eq!(halo_params.border_crop, [0.05, 0.06, 0.07, 0.08]);

        let cathode_params = CathodeInterferenceShaderParams::from_state(&state);
        assert_eq!(cathode_params.border_crop, [0.05, 0.06, 0.07, 0.08]);

        let glass_params = GlassShaderParams::from_state(&state, [0.03, 0.04], 0.02, 1);
        assert_eq!(glass_params.border_crop, [0.05, 0.06, 0.07, 0.08]);
        assert_eq!(glass_params.warp, [0.03, 0.04]);
        assert_eq!(glass_params.corner_size, 0.02);
        assert_eq!(glass_params.filter_type, 1);
    }

    #[test]
    fn test_glass_shader_params_default_and_from_state() {
        let default_params = GlassShaderParams::default();
        assert!(!default_params.enabled);
        assert_eq!(default_params.intensity, 0.25);
        assert_eq!(default_params.glossiness, 0.50);
        assert!(default_params.ceiling_light_enabled);
        assert!(!default_params.photographer_enabled);
        assert_eq!(default_params.photographer_intensity, 0.50);
        assert!(!default_params.flash_enabled);
        assert_eq!(default_params.flash_intensity, 0.70);
        assert_eq!(default_params.warp, [0.0, 0.0]);
        assert_eq!(default_params.filter_type, 0);

        let mut state = AppState::default();
        state.video.crt_glass_enabled = true;
        state.video.crt_glass_intensity = 0.8;
        state.video.crt_glass_glossiness = 0.75;
        state.video.crt_glass_ceiling_light_enabled = true;
        state.video.crt_glass_photographer_enabled = true;
        state.video.crt_glass_photographer_intensity = 0.65;
        state.video.crt_glass_flash_enabled = true;
        state.video.crt_glass_flash_intensity = 0.85;
        state.video.horizontal_stretch = 1.2;

        let params = GlassShaderParams::from_state(&state, [0.031, 0.041], 0.05, 2);
        assert!(params.enabled);
        assert_eq!(params.intensity, 0.8);
        assert_eq!(params.glossiness, 0.75);
        assert!(params.ceiling_light_enabled);
        assert!(params.photographer_enabled);
        assert_eq!(params.photographer_intensity, 0.65);
        assert!(params.flash_enabled);
        assert_eq!(params.flash_intensity, 0.85);
        assert_eq!(params.warp, [0.031, 0.041]);
        assert_eq!(params.corner_size, 0.05);
        assert_eq!(params.filter_type, 2);
        assert_eq!(params.horizontal_stretch, 1.2);

        // When ceiling light is toggled off in state
        state.video.crt_glass_ceiling_light_enabled = false;
        let params_no_ceiling = GlassShaderParams::from_state(&state, [0.031, 0.041], 0.05, 2);
        assert!(!params_no_ceiling.ceiling_light_enabled);

        // When night mode is enabled (lights off), ceiling and camera/flash reflections are suppressed
        state.video.crt_glass_ceiling_light_enabled = true;
        state.video.lights_off_night_mode = true;
        let params_night = GlassShaderParams::from_state(&state, [0.031, 0.041], 0.05, 2);
        assert!(params_night.enabled);
        assert!(!params_night.ceiling_light_enabled);
        assert!(!params_night.photographer_enabled);
        assert!(!params_night.flash_enabled);

        // When night mode is disabled, reflections are restored
        state.video.lights_off_night_mode = false;
        let params_restored = GlassShaderParams::from_state(&state, [0.031, 0.041], 0.05, 2);
        assert!(params_restored.ceiling_light_enabled);
        assert!(params_restored.photographer_enabled);
        assert!(params_restored.flash_enabled);

        // When CRT is Off (filter_type = 0), glass and reflections must not be active
        let params_off = GlassShaderParams::from_state(&state, [0.0, 0.0], 0.0, 0);
        assert!(!params_off.enabled);
        assert!(!params_off.ceiling_light_enabled);
        assert!(!params_off.photographer_enabled);
        assert!(!params_off.flash_enabled);
    }

    #[test]
    fn test_realistic_light_switch_flicker_timeline_and_params() {
        use crate::app::models::{VideoSettings, LIGHT_SWITCH_FLICKER_DURATION_SECS};
        use std::time::{Duration, Instant};

        // Verify multiple distinct dark/light flicker transitions occur before stabilization
        let sample_times = [
            (0.02, true),  // Initial starter pre-heat (dark)
            (0.10, false), // Strike 1 ON
            (0.20, true),  // Dropout 1 (dark)
            (0.31, false), // Strike 2 ON
            (0.37, true),  // Dropout 2 (dark)
            (0.46, false), // Strike 3 ON
            (0.56, true),  // Dropout 3 (dark)
            (0.65, false), // Strike 4 ON
            (0.70, true),  // Final micro-blink (dark)
            (0.75, false), // Stabilized ON
        ];
        for (t, expected_dark) in sample_times {
            assert_eq!(
                VideoSettings::evaluate_light_switch_flicker_is_dark(t),
                expected_dark,
                "Unexpected flicker state at t={t}s"
            );
        }

        let mut state = AppState::default();
        state.video.crt_glass_enabled = true;
        state.video.crt_glass_ceiling_light_enabled = true;
        state.video.realistic_light_switch = true;
        state.video.lights_off_night_mode = true;

        // Toggling from dark -> light starts the flicker sequence
        state.toggle_lights_off_night_mode();
        assert!(!state.video.lights_off_night_mode);
        assert!(state.video.light_switch_flicker_start.is_some());
        assert!(state.video.advance_light_switch_flicker());

        // Immediately after flipping on (t ~ 0.0s), ceiling light is in starter pre-heat (dark)
        let params_start = GlassShaderParams::from_state(&state, [0.031, 0.041], 0.05, 1);
        assert!(!params_start.ceiling_light_enabled);

        // During Strike 1 (t = 0.10s), ceiling light flashes ON
        state.video.light_switch_flicker_start = Some(Instant::now() - Duration::from_millis(100));
        let params_strike1 = GlassShaderParams::from_state(&state, [0.031, 0.041], 0.05, 1);
        assert!(params_strike1.ceiling_light_enabled);

        // During Dropout 1 (t = 0.20s), ceiling light drops back OFF
        state.video.light_switch_flicker_start = Some(Instant::now() - Duration::from_millis(200));
        let params_dropout1 = GlassShaderParams::from_state(&state, [0.031, 0.041], 0.05, 1);
        assert!(!params_dropout1.ceiling_light_enabled);

        // Once duration has elapsed, ceiling light stabilizes ON and clears flicker_start
        state.video.light_switch_flicker_start = Some(
            Instant::now() - Duration::from_secs_f32(LIGHT_SWITCH_FLICKER_DURATION_SECS + 0.10),
        );
        let params_stable = GlassShaderParams::from_state(&state, [0.031, 0.041], 0.05, 1);
        assert!(params_stable.ceiling_light_enabled);
        assert!(state.video.advance_light_switch_flicker()); // Final stabilizing repaint
        assert!(state.video.light_switch_flicker_start.is_none());
        assert!(!state.video.advance_light_switch_flicker());

        // Toggling from light -> dark extinguishes immediately without flickering
        state.toggle_lights_off_night_mode();
        assert!(state.video.lights_off_night_mode);
        assert!(state.video.light_switch_flicker_start.is_none());
        assert!(!state.video.advance_light_switch_flicker());
    }

    #[test]
    fn test_night_mode_preserves_brightboost() {
        let mut state = AppState::default();
        state.crt_filter.store(
            crate::devices::filter_type::CrtFilter::Lottes as u8,
            Ordering::Relaxed,
        );
        state.crt.brightboost = 1.0;
        state.halo.brightboost = 1.30;
        state.halo.brightboost1 = 1.65;

        // Normal mode: unboosted
        let p_norm = ShaderParams::from_state(&state);
        let hp_norm = HaloShaderParams::from_state(&state);
        assert_eq!(p_norm.brightboost, 1.0);
        assert_eq!(hp_norm.brightboost, 1.30);
        assert_eq!(hp_norm.brightboost1, 1.65);

        // Night mode on: preserves calibrated values without artificial boost
        state.video.lights_off_night_mode = true;
        let p_night = ShaderParams::from_state(&state);
        let hp_night = HaloShaderParams::from_state(&state);
        assert_eq!(p_night.brightboost, 1.0);
        assert_eq!(hp_night.brightboost, 1.30);
        assert_eq!(hp_night.brightboost1, 1.65);

        // If CRT filter is turned off, values remain clean
        state.crt_filter.store(
            crate::devices::filter_type::CrtFilter::Off as u8,
            Ordering::Relaxed,
        );
        let p_off = ShaderParams::from_state(&state);
        let hp_off = HaloShaderParams::from_state(&state);
        assert_eq!(p_off.brightboost, 1.0);
        assert_eq!(hp_off.brightboost, 1.30);
        assert_eq!(hp_off.brightboost1, 1.65);
    }

    #[test]
    fn test_audio_reactive_shader_params_from_state() {
        let mut state = AppState::default();
        state.audio_reactive.enabled = true;
        state.audio_reactive.current.master_intensity = 1.45;
        state.audio_reactive.current.flyback_pump = 0.88;
        state.audio_reactive_runtime.low_env = 0.62;
        state.audio_reactive_runtime.transient_env = 0.91;

        let p = AudioReactiveShaderParams::from_state(&state, [0.031, 0.041], 0.02, 2);
        assert!(p.enabled);
        assert_eq!(p.master_intensity, 1.45);
        assert_eq!(p.flyback_pump, 0.88);
        assert_eq!(p.low_env, 0.62);
        assert_eq!(p.transient_env, 0.91);
        assert_eq!(p.warp, [0.031, 0.041]);
        assert_eq!(p.corner_size, 0.02);
        assert_eq!(p.filter_type, 2);
    }
}
