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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScalerFilter {
    FastBilinear = 0,
    Bilinear = 1,
    Bicubic = 2,
    Point = 3,
    Lanczos = 4,
    BuNNy = 5,
    BuNNyMedium = 6,
    BuNNyHigh = 7,
}

impl ScalerFilter {
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => ScalerFilter::Bilinear,
            2 => ScalerFilter::Bicubic,
            3 => ScalerFilter::Point,
            4 => ScalerFilter::Lanczos,
            5 => ScalerFilter::BuNNy,
            6 => ScalerFilter::BuNNyMedium,
            7 => ScalerFilter::BuNNyHigh,
            _ => ScalerFilter::FastBilinear,
        }
    }

    pub fn into_ffmpeg_flag(self) -> ffmpeg_next::software::scaling::flag::Flags {
        match self {
            ScalerFilter::FastBilinear => ffmpeg_next::software::scaling::flag::Flags::FAST_BILINEAR,
            ScalerFilter::Bilinear => ffmpeg_next::software::scaling::flag::Flags::BILINEAR,
            ScalerFilter::Bicubic => ffmpeg_next::software::scaling::flag::Flags::BICUBIC,
            ScalerFilter::Point => ffmpeg_next::software::scaling::flag::Flags::POINT,
            ScalerFilter::Lanczos => ffmpeg_next::software::scaling::flag::Flags::LANCZOS,
            ScalerFilter::BuNNy | ScalerFilter::BuNNyMedium | ScalerFilter::BuNNyHigh => {
                ffmpeg_next::software::scaling::flag::Flags::LANCZOS
            }
        }
    }
}

impl std::fmt::Display for ScalerFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            ScalerFilter::FastBilinear => "Fast Bilinear",
            ScalerFilter::Bilinear => "Bilinear",
            ScalerFilter::Bicubic => "Bicubic",
            ScalerFilter::Point => "Point (Nearest)",
            ScalerFilter::Lanczos => "Lanczos",
            ScalerFilter::BuNNy => "BuNNy (CNN Fast)",
            ScalerFilter::BuNNyMedium => "BuNNy (CNN Medium)",
            ScalerFilter::BuNNyHigh => "BuNNy (CNN High)",
        };
        write!(f, "{}", text)
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
}

impl std::fmt::Display for ColorRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            ColorRange::Full => "Full (PC)",
            ColorRange::Limited => "Limited (TV)",
        };
        write!(f, "{}", text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_video_format_default() {
        let default = VideoFormat::default();
        assert_eq!(default.fourcc, "0000");
        assert_eq!(default.description, "None");
        assert!(default.resolutions.is_empty());
    }

    #[test]
    fn test_scaler_filter_from_u8() {
        assert_eq!(ScalerFilter::from_u8(0), ScalerFilter::FastBilinear);
        assert_eq!(ScalerFilter::from_u8(1), ScalerFilter::Bilinear);
        assert_eq!(ScalerFilter::from_u8(2), ScalerFilter::Bicubic);
        assert_eq!(ScalerFilter::from_u8(3), ScalerFilter::Point);
        assert_eq!(ScalerFilter::from_u8(4), ScalerFilter::Lanczos);
        assert_eq!(ScalerFilter::from_u8(5), ScalerFilter::BuNNy);
        assert_eq!(ScalerFilter::from_u8(6), ScalerFilter::BuNNyMedium);
        assert_eq!(ScalerFilter::from_u8(7), ScalerFilter::BuNNyHigh);
        assert_eq!(ScalerFilter::from_u8(10), ScalerFilter::FastBilinear); // Default
    }

    #[test]
    fn test_scaler_filter_into_ffmpeg_flag() {
        assert_eq!(ScalerFilter::FastBilinear.into_ffmpeg_flag(), ffmpeg_next::software::scaling::flag::Flags::FAST_BILINEAR);
        assert_eq!(ScalerFilter::Bilinear.into_ffmpeg_flag(), ffmpeg_next::software::scaling::flag::Flags::BILINEAR);
        assert_eq!(ScalerFilter::Bicubic.into_ffmpeg_flag(), ffmpeg_next::software::scaling::flag::Flags::BICUBIC);
        assert_eq!(ScalerFilter::Point.into_ffmpeg_flag(), ffmpeg_next::software::scaling::flag::Flags::POINT);
        assert_eq!(ScalerFilter::Lanczos.into_ffmpeg_flag(), ffmpeg_next::software::scaling::flag::Flags::LANCZOS);
        assert_eq!(ScalerFilter::BuNNy.into_ffmpeg_flag(), ffmpeg_next::software::scaling::flag::Flags::LANCZOS);
        assert_eq!(ScalerFilter::BuNNyMedium.into_ffmpeg_flag(), ffmpeg_next::software::scaling::flag::Flags::LANCZOS);
        assert_eq!(ScalerFilter::BuNNyHigh.into_ffmpeg_flag(), ffmpeg_next::software::scaling::flag::Flags::LANCZOS);
    }

    #[test]
    fn test_color_range_from_u8() {
        assert_eq!(ColorRange::from_u8(0), ColorRange::Full);
        assert_eq!(ColorRange::from_u8(1), ColorRange::Limited);
        assert_eq!(ColorRange::from_u8(2), ColorRange::Full); // Default
    }
}
