use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use crate::{app::AppState, config::MichadameConfig};

pub const MIN_POOLED_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_POOLED_FRAME_BYTES: usize = 64 * 1024 * 1024;
const MAX_POOLED_FRAME_BUFFERS: usize = 24;
const CAPACITY_SLACK_BYTES: usize = 4096;

struct FrameBufferPool {
    buffers: VecDeque<Vec<u8>>,
}

impl FrameBufferPool {
    fn new() -> Self {
        Self {
            buffers: VecDeque::with_capacity(MAX_POOLED_FRAME_BUFFERS),
        }
    }

    fn take_matching(&mut self, exact_len: usize) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
        if let Some(idx) = self
            .buffers
            .iter()
            .rposition(|buf| buf.capacity() == exact_len)
        {
            return (self.buffers.remove(idx), None);
        }

        let max_cap = exact_len.saturating_add(CAPACITY_SLACK_BYTES);
        if let Some(idx) = self
            .buffers
            .iter()
            .rposition(|buf| buf.capacity() >= exact_len && buf.capacity() <= max_cap)
        {
            return (self.buffers.remove(idx), None);
        }

        let evicted = if self.buffers.len() >= MAX_POOLED_FRAME_BUFFERS {
            self.buffers.pop_front()
        } else {
            None
        };
        (None, evicted)
    }

    fn put(&mut self, buf: Vec<u8>) -> Option<Vec<u8>> {
        let evicted = if self.buffers.len() >= MAX_POOLED_FRAME_BUFFERS {
            self.buffers.pop_front()
        } else {
            None
        };
        self.buffers.push_back(buf);
        evicted
    }
}

fn global_frame_buffer_pool() -> &'static Mutex<FrameBufferPool> {
    static POOL: OnceLock<Mutex<FrameBufferPool>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(FrameBufferPool::new()))
}

/// Checks out a byte buffer of length `exact_len` from the bounded global frame pool,
/// avoiding heap allocation and page-zeroing on steady-state cache hits.
pub fn checkout_frame_buffer(exact_len: usize) -> Vec<u8> {
    if exact_len == 0 {
        return Vec::new();
    }
    if !(MIN_POOLED_FRAME_BYTES..=MAX_POOLED_FRAME_BYTES).contains(&exact_len) {
        return vec![0u8; exact_len];
    }

    let (matched, evicted) = {
        let mut guard = global_frame_buffer_pool()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        guard.take_matching(exact_len)
    };
    drop(evicted);

    if let Some(mut buf) = matched {
        if buf.len() >= exact_len {
            buf.truncate(exact_len);
        } else {
            buf.resize(exact_len, 0);
        }
        buf
    } else {
        vec![0u8; exact_len]
    }
}

/// Returns a frame byte buffer to the bounded global frame pool if its capacity falls within
/// `[MIN_POOLED_FRAME_BYTES, MAX_POOLED_FRAME_BYTES]`. Preserves `buf.len()` so subsequent
/// checkouts of equal or smaller size require zero `memset` initialization.
pub fn recycle_frame_buffer(buf: Vec<u8>) {
    if !(MIN_POOLED_FRAME_BYTES..=MAX_POOLED_FRAME_BYTES).contains(&buf.capacity()) {
        return;
    }

    let evicted = {
        let mut guard = global_frame_buffer_pool()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        guard.put(buf)
    };
    drop(evicted);
}

pub struct RawFrame {
    pub captured_at: i64,
    pub rate: crate::replay::config::Rate,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub format: ffmpeg_next::format::Pixel,
    pub color_range: ColorRange,
}

impl Drop for RawFrame {
    fn drop(&mut self) {
        let buf = std::mem::take(&mut self.data);
        recycle_frame_buffer(buf);
    }
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
    if let Ok(formats) =
        crate::devices::video::find_video_formats(&state.hardware.selected_video_device)
    {
        state.hardware.supported_formats = formats;

        let active_profile = cfg.profiles.get(&cfg.active_profile);
        let saved_fourcc = active_profile.and_then(|p| p.video_format_fourcc.as_ref());
        let saved_res = active_profile
            .and_then(|p| p.video_resolution)
            .or(cfg.video_resolution);
        let saved_fps = active_profile
            .and_then(|p| p.video_framerate)
            .or(cfg.video_framerate);

        if let Some(saved_fourcc) = saved_fourcc {
            if let Some(idx) = state
                .hardware
                .supported_formats
                .iter()
                .position(|f| f.fourcc == *saved_fourcc)
            {
                state.hardware.selected_format_index = idx;
                if let Some(saved_res) = saved_res {
                    if state.hardware.supported_formats[idx]
                        .resolutions
                        .iter()
                        .any(|r| r.width == saved_res.0 && r.height == saved_res.1)
                    {
                        state.hardware.selected_resolution = saved_res;
                        if let Some(saved_fps) = saved_fps {
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
    Anime4kSmall = 5,
    Anime4kMedium = 6,
    Anime4kLarge = 7,
}

impl ScalerFilter {
    pub const ALL: [ScalerFilter; 8] = [
        ScalerFilter::FastBilinear,
        ScalerFilter::Bilinear,
        ScalerFilter::Bicubic,
        ScalerFilter::Point,
        ScalerFilter::Lanczos,
        ScalerFilter::Anime4kSmall,
        ScalerFilter::Anime4kMedium,
        ScalerFilter::Anime4kLarge,
    ];

    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => ScalerFilter::Bilinear,
            2 => ScalerFilter::Bicubic,
            3 => ScalerFilter::Point,
            4 => ScalerFilter::Lanczos,
            5 | 10 => ScalerFilter::Anime4kSmall,
            6 | 11 => ScalerFilter::Anime4kMedium,
            7 | 12 => ScalerFilter::Anime4kLarge,
            _ => ScalerFilter::FastBilinear,
        }
    }

    pub fn into_ffmpeg_flag(self) -> ffmpeg_next::software::scaling::flag::Flags {
        match self {
            ScalerFilter::FastBilinear => {
                ffmpeg_next::software::scaling::flag::Flags::FAST_BILINEAR
            }
            ScalerFilter::Bilinear => ffmpeg_next::software::scaling::flag::Flags::BILINEAR,
            ScalerFilter::Bicubic => ffmpeg_next::software::scaling::flag::Flags::BICUBIC,
            ScalerFilter::Point => ffmpeg_next::software::scaling::flag::Flags::POINT,
            ScalerFilter::Lanczos => ffmpeg_next::software::scaling::flag::Flags::LANCZOS,
            ScalerFilter::Anime4kSmall
            | ScalerFilter::Anime4kMedium
            | ScalerFilter::Anime4kLarge => {
                ffmpeg_next::software::scaling::flag::Flags::FAST_BILINEAR
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
            ScalerFilter::Anime4kSmall => "Anime4K S (Very Fast)",
            ScalerFilter::Anime4kMedium => "Anime4K M (Fast)",
            ScalerFilter::Anime4kLarge => "Anime4K L (Medium)",
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
        assert_eq!(ScalerFilter::from_u8(5), ScalerFilter::Anime4kSmall);
        assert_eq!(ScalerFilter::from_u8(6), ScalerFilter::Anime4kMedium);
        assert_eq!(ScalerFilter::from_u8(7), ScalerFilter::Anime4kLarge);
        assert_eq!(ScalerFilter::from_u8(10), ScalerFilter::Anime4kSmall);
        assert_eq!(ScalerFilter::from_u8(11), ScalerFilter::Anime4kMedium);
        assert_eq!(ScalerFilter::from_u8(12), ScalerFilter::Anime4kLarge);
        assert_eq!(ScalerFilter::from_u8(13), ScalerFilter::FastBilinear); // Default
    }

    #[test]
    fn test_scaler_filter_into_ffmpeg_flag() {
        assert_eq!(
            ScalerFilter::FastBilinear.into_ffmpeg_flag(),
            ffmpeg_next::software::scaling::flag::Flags::FAST_BILINEAR
        );
        assert_eq!(
            ScalerFilter::Bilinear.into_ffmpeg_flag(),
            ffmpeg_next::software::scaling::flag::Flags::BILINEAR
        );
        assert_eq!(
            ScalerFilter::Bicubic.into_ffmpeg_flag(),
            ffmpeg_next::software::scaling::flag::Flags::BICUBIC
        );
        assert_eq!(
            ScalerFilter::Point.into_ffmpeg_flag(),
            ffmpeg_next::software::scaling::flag::Flags::POINT
        );
        assert_eq!(
            ScalerFilter::Lanczos.into_ffmpeg_flag(),
            ffmpeg_next::software::scaling::flag::Flags::LANCZOS
        );
        assert_eq!(
            ScalerFilter::Anime4kSmall.into_ffmpeg_flag(),
            ffmpeg_next::software::scaling::flag::Flags::FAST_BILINEAR
        );
        assert_eq!(
            ScalerFilter::Anime4kMedium.into_ffmpeg_flag(),
            ffmpeg_next::software::scaling::flag::Flags::FAST_BILINEAR
        );
        assert_eq!(
            ScalerFilter::Anime4kLarge.into_ffmpeg_flag(),
            ffmpeg_next::software::scaling::flag::Flags::FAST_BILINEAR
        );
    }

    #[test]
    fn test_color_range_from_u8() {
        assert_eq!(ColorRange::from_u8(0), ColorRange::Full);
        assert_eq!(ColorRange::from_u8(1), ColorRange::Limited);
        assert_eq!(ColorRange::from_u8(2), ColorRange::Full); // Default
    }

    #[test]
    fn test_checkout_and_raw_frame_drop_reuses_buffer_allocation() {
        assert!(checkout_frame_buffer(0).is_empty());

        let exact_len = 197_653;
        let mut first_buf = checkout_frame_buffer(exact_len);
        assert_eq!(first_buf.len(), exact_len);
        first_buf.fill(0xA5);
        let original_ptr = first_buf.as_ptr();
        let original_cap = first_buf.capacity();

        {
            let _frame = RawFrame {
                captured_at: 42,
                rate: crate::replay::config::Rate { num: 60, den: 1 },
                width: 320,
                height: 240,
                data: first_buf,
                format: ffmpeg_next::format::Pixel::RGBA,
                color_range: ColorRange::Full,
            };
        }

        let reused_buf = checkout_frame_buffer(exact_len);
        assert_eq!(reused_buf.len(), exact_len);
        assert_eq!(reused_buf.capacity(), original_cap);
        assert_eq!(reused_buf.as_ptr(), original_ptr);
        // Existing initialized bytes were preserved without zero-filling.
        assert_eq!(reused_buf[0], 0xA5);
        assert_eq!(reused_buf[exact_len - 1], 0xA5);
    }

    #[test]
    fn test_frame_buffer_pool_evicts_oldest_stale_sizes_when_full() {
        let mut pool = FrameBufferPool::new();
        let stale_len = 96 * 1024;
        let mut stale_ptrs = Vec::with_capacity(MAX_POOLED_FRAME_BUFFERS);

        for _ in 0..MAX_POOLED_FRAME_BUFFERS {
            let buf = vec![1u8; stale_len];
            stale_ptrs.push(buf.as_ptr());
            assert!(pool.put(buf).is_none());
        }
        assert_eq!(pool.buffers.len(), MAX_POOLED_FRAME_BUFFERS);

        // A miss at a new resolution while the pool is full must evict the oldest stale buffer.
        let new_len = 256 * 1024;
        let (matched, evicted) = pool.take_matching(new_len);
        assert!(matched.is_none());
        let evicted = evicted.expect("oldest stale buffer should be evicted on miss when full");
        assert_eq!(evicted.as_ptr(), stale_ptrs[0]);
        assert_eq!(pool.buffers.len(), MAX_POOLED_FRAME_BUFFERS - 1);

        // Recycling the new-resolution buffer and filling again keeps the new buffer and evicts stale ones.
        let new_buf = vec![2u8; new_len];
        let new_ptr = new_buf.as_ptr();
        assert!(pool.put(new_buf).is_none());
        assert_eq!(pool.buffers.len(), MAX_POOLED_FRAME_BUFFERS);

        let extra_buf = vec![3u8; new_len];
        let evicted_on_put = pool
            .put(extra_buf)
            .expect("oldest remaining stale buffer should be evicted when full");
        assert_eq!(evicted_on_put.as_ptr(), stale_ptrs[1]);

        // Most recently recycled buffer of `new_len` is reused without reallocation.
        let (hit1, ev1) = pool.take_matching(new_len);
        assert!(ev1.is_none());
        assert!(hit1.is_some());
        let (hit2, ev2) = pool.take_matching(new_len);
        assert!(ev2.is_none());
        let hit2_buf = hit2.expect("second hit");
        assert_eq!(hit2_buf.as_ptr(), new_ptr);
    }

    #[test]
    fn test_checkout_slack_and_bounds_handling() {
        let small = checkout_frame_buffer(1024);
        assert_eq!(small.len(), 1024);

        let base_len = 160 * 1024;
        let buf = vec![0x5Au8; base_len + 2048];
        let ptr = buf.as_ptr();
        let cap = buf.capacity();
        recycle_frame_buffer(buf);

        // Within CAPACITY_SLACK_BYTES (4096) reuses the buffer and truncates in O(1).
        let checked_out = checkout_frame_buffer(base_len);
        assert_eq!(checked_out.len(), base_len);
        assert_eq!(checked_out.capacity(), cap);
        assert_eq!(checked_out.as_ptr(), ptr);
    }
}
