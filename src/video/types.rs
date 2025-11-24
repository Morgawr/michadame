use crate::{app::AppState, config::MichadameConfig};

#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
    pub framerates: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoFormat {
    pub fourcc: String,
    pub description: String,
    pub resolutions: Vec<Resolution>,
}

impl Default for VideoFormat {
    fn default() -> Self {
        Self { fourcc: "0000".to_string(), description: "None".to_string(), resolutions: vec![] }
    }
}

pub fn apply_saved_format_config(state: &mut AppState, cfg: &MichadameConfig) {
    if let Ok(formats) = crate::devices::video::find_video_formats(&state.selected_video_device)
    {
        state.supported_formats = formats;
        if let Some(saved_fourcc) = &cfg.video_format_fourcc {
            if let Some(idx) = state.supported_formats.iter().position(|f| f.fourcc == *saved_fourcc)
            {
                state.selected_format_index = idx;
                if let Some(saved_res) = cfg.video_resolution {
                    if state.supported_formats[idx].resolutions.iter().any(|r| r.width == saved_res.0 && r.height == saved_res.1) {
                        state.selected_resolution = saved_res;
                        if let Some(saved_fps) = cfg.video_framerate {
                            if let Some(res_info) = state.supported_formats[idx].resolutions.iter().find(|r| r.width == saved_res.0 && r.height == saved_res.1) {
                                if res_info.framerates.contains(&saved_fps) {
                                    state.selected_framerate = saved_fps;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}