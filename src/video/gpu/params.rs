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
    pub vibrance: f32,
    pub scaler_filter: u8,
    pub overscan_x: f32,
    pub overscan_y: f32,
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
            vibrance: state.video.vibrance,
            scaler_filter: state.scaler_filter.load(Ordering::Relaxed),
            overscan_x: state.video.overscan_x,
            overscan_y: state.video.overscan_y,
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
            vibrance: 1.0,
            scaler_filter: crate::video::types::ScalerFilter::FastBilinear as u8,
            overscan_x: 0.0,
            overscan_y: 0.0,
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
        state.scaler_filter.store(crate::video::types::ScalerFilter::Lanczos as u8, Ordering::Relaxed);
        
        let params = ShaderParams::from_state(&state);
        assert_eq!(params.scaler_filter, crate::video::types::ScalerFilter::Lanczos as u8);
    }
}
