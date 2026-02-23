use crate::{app::AppState, config::MichadameConfig};

pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub format: ffmpeg_next::format::Pixel,
    pub color_range: ColorRange,
}

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
        Self {
            fourcc: "0000".to_string(),
            description: "None".to_string(),
            resolutions: vec![],
        }
    }
}

pub fn apply_saved_format_config(state: &mut AppState, cfg: &MichadameConfig) {
    if let Ok(formats) = crate::devices::video::find_video_formats(&state.hardware.selected_video_device) {
        state.hardware.supported_formats = formats;

        let saved_fourcc = cfg
            .profiles
            .get(&cfg.active_profile)
            .and_then(|p| p.video_format_fourcc.as_ref());

        if let Some(saved_fourcc) = saved_fourcc {
            if let Some(idx) = state
                .hardware.supported_formats
                .iter()
                .position(|f| f.fourcc == *saved_fourcc)
            {
                state.hardware.selected_format_index = idx;
                if let Some(saved_res) = cfg.video_resolution {
                    if state.hardware.supported_formats[idx]
                        .resolutions
                        .iter()
                        .any(|r| r.width == saved_res.0 && r.height == saved_res.1)
                    {
                        state.hardware.selected_resolution = saved_res;
                        if let Some(saved_fps) = cfg.video_framerate {
                            if let Some(res_info) = state.hardware.supported_formats[idx]
                                .resolutions
                                .iter()
                                .find(|r| r.width == saved_res.0 && r.height == saved_res.1)
                            {
                                if res_info.framerates.contains(&saved_fps) {
                                    state.hardware.selected_framerate = saved_fps;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalerFilter {
    FastBilinear = 0,
    Bilinear = 1,
    Bicubic = 2,
    Point = 3,
    Lanczos = 4,
}

impl ScalerFilter {
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => ScalerFilter::Bilinear,
            2 => ScalerFilter::Bicubic,
            3 => ScalerFilter::Point,
            4 => ScalerFilter::Lanczos,
            _ => ScalerFilter::FastBilinear,
        }
    }
    pub fn to_string(&self) -> String {
        match self {
            ScalerFilter::FastBilinear => "Fast Bilinear".to_string(),
            ScalerFilter::Bilinear => "Bilinear".to_string(),
            ScalerFilter::Bicubic => "Bicubic".to_string(),
            ScalerFilter::Point => "Point (Nearest)".to_string(),
            ScalerFilter::Lanczos => "Lanczos".to_string(),
        }
    }
    pub fn to_ffmpeg_flag(&self) -> ffmpeg_next::software::scaling::flag::Flags {
        match self {
            ScalerFilter::FastBilinear => ffmpeg_next::software::scaling::flag::Flags::FAST_BILINEAR,
            ScalerFilter::Bilinear => ffmpeg_next::software::scaling::flag::Flags::BILINEAR,
            ScalerFilter::Bicubic => ffmpeg_next::software::scaling::flag::Flags::BICUBIC,
            ScalerFilter::Point => ffmpeg_next::software::scaling::flag::Flags::POINT,
            ScalerFilter::Lanczos => ffmpeg_next::software::scaling::flag::Flags::LANCZOS,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorRange {
    Full = 0,
    Limited = 1,
}

impl ColorRange {
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => ColorRange::Limited,
            _ => ColorRange::Full,
        }
    }
    pub fn to_string(&self) -> String {
        match self {
            ColorRange::Full => "Full (PC)".to_string(),
            ColorRange::Limited => "Limited (TV)".to_string(),
        }
    }
}
