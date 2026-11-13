use crate::video::types::{checkout_frame_buffer, ColorRange, RawFrame};
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{
    atomic::{AtomicU64, AtomicU8, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

extern "C" {
    fn michadame_deck_capture_start(
        target: *const c_char,
        frame: unsafe extern "C" fn(*mut c_void, *const u8, u32, u32, u32, u32, u32),
        state: unsafe extern "C" fn(*mut c_void, i32, *const c_char),
        data: *mut c_void,
    ) -> *mut c_void;
    fn michadame_deck_capture_stop(capture: *mut c_void);
}

struct Callbacks {
    tx: crossbeam_channel::Sender<Arc<RawFrame>>,
    repaint: Arc<dyn Fn() + Send + Sync>,
    drops: Arc<AtomicU64>,
    color_range: Arc<AtomicU8>,
    received: AtomicU64,
    error: Mutex<Option<String>>,
    started: Instant,
    limiter: Mutex<FrameLimiter>,
}

/// Gamescope advertises variable-rate (0/1) PipeWire video. Limit expensive
/// copies/repaints as well as presentation, while preserving 60 FPS cadence
/// from 90/120/144 Hz sources and tolerating normal callback jitter.
#[derive(Default)]
struct FrameLimiter {
    next: Option<Duration>,
}

impl FrameLimiter {
    fn accept(&mut self, now: Duration) -> bool {
        let period = super::frame_interval(true);
        if self
            .next
            .is_some_and(|next| now + Duration::from_millis(1) < next)
        {
            return false;
        }
        let next = self.next.unwrap_or(now) + period;
        self.next = Some(if next <= now { now + period } else { next });
        true
    }
}

pub struct Capture {
    handle: *mut c_void,
    callbacks: Box<Callbacks>,
    pub rx: crossbeam_channel::Receiver<Arc<RawFrame>>,
    started: Instant,
}

unsafe impl Send for Capture {}
unsafe impl Sync for Capture {}

impl Capture {
    pub fn start(
        target: &str,
        repaint: Arc<dyn Fn() + Send + Sync>,
        drops: Arc<AtomicU64>,
        color_range: Arc<AtomicU8>,
    ) -> Option<Self> {
        let target = CString::new(target).ok()?;
        let (tx, rx) = crossbeam_channel::bounded(2);
        let mut callbacks = Box::new(Callbacks {
            tx,
            repaint,
            drops,
            color_range,
            received: AtomicU64::new(0),
            error: Mutex::new(None),
            started: Instant::now(),
            limiter: Mutex::new(FrameLimiter::default()),
        });
        let handle = unsafe {
            michadame_deck_capture_start(
                target.as_ptr(),
                on_frame,
                on_state,
                (&mut *callbacks as *mut Callbacks).cast(),
            )
        };
        if handle.is_null() {
            return None;
        }
        tracing::info!(
            "Steam Deck shared-memory PipeWire capture connected to {}",
            target.to_string_lossy()
        );
        Some(Self {
            handle,
            callbacks,
            rx,
            started: Instant::now(),
        })
    }

    pub fn failure(&self) -> Option<String> {
        self.callbacks
            .error
            .lock()
            .ok()
            .and_then(|e| e.clone())
            .or_else(|| {
                (self.callbacks.received.load(Ordering::Acquire) == 0
                    && self.started.elapsed() > Duration::from_secs(30))
                .then(|| {
                    "PipeWire connected but delivered no video frames within 30 seconds".into()
                })
            })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            michadame_deck_capture_stop(self.handle);
        }
    }
}

unsafe extern "C" fn on_state(data: *mut c_void, state: i32, error: *const c_char) {
    let callbacks = &*data.cast::<Callbacks>();
    if state == -1 {
        let message = if error.is_null() {
            "Unknown PipeWire capture error".into()
        } else {
            CStr::from_ptr(error).to_string_lossy().into_owned()
        };
        if let Ok(mut stored) = callbacks.error.lock() {
            *stored = Some(message);
        }
        (callbacks.repaint)();
    }
}

#[inline]
fn swizzle_bgrx_word(px: u32) -> u32 {
    0xFF00_0000 | (px & 0x0000_FF00) | ((px & 0x00FF_0000) >> 16) | ((px & 0x0000_00FF) << 16)
}

#[inline]
fn convert_xrgb_slice_to_rgba(src: &[u8], dst: &mut [u8], bgr: bool) {
    let (src_chunks, src_rem) = src.as_chunks::<16>();
    let (dst_chunks, dst_rem) = dst.as_chunks_mut::<16>();

    if bgr {
        for (in_blk, out_blk) in src_chunks.iter().zip(dst_chunks) {
            let p0 = swizzle_bgrx_word(u32::from_le_bytes([
                in_blk[0], in_blk[1], in_blk[2], in_blk[3],
            ]));
            let p1 = swizzle_bgrx_word(u32::from_le_bytes([
                in_blk[4], in_blk[5], in_blk[6], in_blk[7],
            ]));
            let p2 = swizzle_bgrx_word(u32::from_le_bytes([
                in_blk[8], in_blk[9], in_blk[10], in_blk[11],
            ]));
            let p3 = swizzle_bgrx_word(u32::from_le_bytes([
                in_blk[12], in_blk[13], in_blk[14], in_blk[15],
            ]));
            out_blk[0..4].copy_from_slice(&p0.to_le_bytes());
            out_blk[4..8].copy_from_slice(&p1.to_le_bytes());
            out_blk[8..12].copy_from_slice(&p2.to_le_bytes());
            out_blk[12..16].copy_from_slice(&p3.to_le_bytes());
        }
        for (in_px, out_px) in src_rem.chunks_exact(4).zip(dst_rem.chunks_exact_mut(4)) {
            let px =
                swizzle_bgrx_word(u32::from_le_bytes([in_px[0], in_px[1], in_px[2], in_px[3]]));
            out_px.copy_from_slice(&px.to_le_bytes());
        }
    } else {
        for (in_blk, out_blk) in src_chunks.iter().zip(dst_chunks) {
            let p0 = u32::from_le_bytes([in_blk[0], in_blk[1], in_blk[2], in_blk[3]]) | 0xFF00_0000;
            let p1 = u32::from_le_bytes([in_blk[4], in_blk[5], in_blk[6], in_blk[7]]) | 0xFF00_0000;
            let p2 =
                u32::from_le_bytes([in_blk[8], in_blk[9], in_blk[10], in_blk[11]]) | 0xFF00_0000;
            let p3 =
                u32::from_le_bytes([in_blk[12], in_blk[13], in_blk[14], in_blk[15]]) | 0xFF00_0000;
            out_blk[0..4].copy_from_slice(&p0.to_le_bytes());
            out_blk[4..8].copy_from_slice(&p1.to_le_bytes());
            out_blk[8..12].copy_from_slice(&p2.to_le_bytes());
            out_blk[12..16].copy_from_slice(&p3.to_le_bytes());
        }
        for (in_px, out_px) in src_rem.chunks_exact(4).zip(dst_rem.chunks_exact_mut(4)) {
            let px = u32::from_le_bytes([in_px[0], in_px[1], in_px[2], in_px[3]]) | 0xFF00_0000;
            out_px.copy_from_slice(&px.to_le_bytes());
        }
    }
}

/// Copy only visible pixels into a pooled frame buffer, respecting padded scanlines and
/// normalizing BGRx/RGBx to opaque RGBA using SIMD-vectorizable `u32` chunked operations.
pub fn rgba_pixels(
    bytes: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    bgr: bool,
) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return None;
    }
    let row_bytes = (width as usize).checked_mul(4)?;
    let stride = stride as usize;
    if stride < row_bytes {
        return None;
    }
    let needed = (height as usize - 1)
        .checked_mul(stride)?
        .checked_add(row_bytes)?;
    if bytes.len() < needed {
        return None;
    }
    let total_bytes = row_bytes.checked_mul(height as usize)?;
    let mut rgba = checkout_frame_buffer(total_bytes);

    if stride == row_bytes {
        convert_xrgb_slice_to_rgba(&bytes[..total_bytes], &mut rgba, bgr);
    } else {
        for y in 0..height as usize {
            let src_start = y * stride;
            let dst_start = y * row_bytes;
            convert_xrgb_slice_to_rgba(
                &bytes[src_start..src_start + row_bytes],
                &mut rgba[dst_start..dst_start + row_bytes],
                bgr,
            );
        }
    }
    Some(rgba)
}

unsafe extern "C" fn on_frame(
    data: *mut c_void,
    pixels: *const u8,
    size: u32,
    width: u32,
    height: u32,
    stride: u32,
    bgr: u32,
) {
    let callbacks = &*data.cast::<Callbacks>();
    if pixels.is_null() || size > 256 * 1024 * 1024 {
        return;
    }
    if callbacks.tx.is_full() {
        callbacks.drops.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let Ok(mut limiter) = callbacks.limiter.lock() else {
        return;
    };
    if !limiter.accept(callbacks.started.elapsed()) {
        return;
    }
    drop(limiter);
    let Some(rgba) = rgba_pixels(
        std::slice::from_raw_parts(pixels, size as usize),
        width,
        height,
        stride,
        bgr != 0,
    ) else {
        return;
    };
    let frame = Arc::new(RawFrame {
        captured_at: crate::replay::now_us(),
        rate: crate::replay::config::Rate { num: 60, den: 1 },
        width,
        height,
        data: rgba,
        format: ffmpeg_next::format::Pixel::RGBA,
        color_range: ColorRange::from_u8(callbacks.color_range.load(Ordering::Relaxed)),
    });
    if callbacks.received.fetch_add(1, Ordering::AcqRel) == 0 {
        tracing::info!("Steam Deck video received first frame: {width}x{height}");
    }
    let _ = callbacks.tx.try_send(frame);
    (callbacks.repaint)();
}

pub fn poll(state: &mut crate::app::AppState, ctx: &eframe::egui::Context) {
    let Some(deck) = &mut state.steam_deck else {
        return;
    };
    if let Some(capture) = &deck.video_capture {
        while let Ok(frame) = capture.rx.try_recv() {
            if let Some(hook) = &mut state.gamescope_hook {
                hook.initial_resolution = (frame.width, frame.height);
            }
            state.latest_frame = Some(frame);
            state.video_frames_since_last_check += 1;
            deck.capture_failures = 0;
        }
        if let Some(error) = capture.failure() {
            tracing::warn!("Steam Deck video capture failed: {error}");
            deck.capture_error = Some(error);
            deck.video_capture = None;
            deck.capture_target = None;
            deck.capture_scan = None;
            deck.capture_failures += 1;
        }
    }
    if deck.video_capture.is_none() && state.gamescope_hook.is_some() {
        if let Some(target) = &deck.capture_target {
            let repaint_ctx = ctx.clone();
            deck.video_capture = Capture::start(
                target,
                Arc::new(move || repaint_ctx.request_repaint()),
                state.hardware.video_queue_drops.clone(),
                state.color_range.clone(),
            );
            if deck.video_capture.is_none() {
                deck.capture_error =
                    Some("Could not connect to the game's PipeWire video stream".into());
                deck.capture_failures += 1;
            }
        }
    }
    if deck.capture_failures >= 3 {
        let error = deck.capture_error.clone().unwrap_or_default();
        state.stop_stream(ctx);
        state.error(format!("Steam Deck video capture failed: {error}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_limit_preserves_sixty_fps_across_source_rates() {
        for hz in [30u64, 60, 90, 120, 144, 240] {
            let mut limiter = FrameLimiter::default();
            let accepted = (0..hz * 10)
                .filter(|i| limiter.accept(Duration::from_nanos(i * 1_000_000_000 / hz)))
                .count();
            assert!(
                (accepted as i64 - hz.min(60) as i64 * 10).abs() <= 1,
                "{hz} Hz: {accepted} frames"
            );
        }
    }

    #[test]
    fn capture_limit_handles_jitter_and_does_not_replay_a_backlog() {
        let mut limiter = FrameLimiter::default();
        for i in 0..600u64 {
            let timestamp = i * 1_000_000_000 / 60 + if i % 2 == 0 { 0 } else { 400_000 };
            assert!(
                limiter.accept(Duration::from_nanos(timestamp)),
                "Dropped legal frame {i}"
            );
        }
        assert!(limiter.accept(Duration::from_secs(20)));
        for _ in 0..100 {
            assert!(!limiter.accept(Duration::from_secs(20)));
        }
    }

    #[test]
    fn rgba_pixels_swizzles_bgrx_and_rgbx_across_tight_and_padded_strides() {
        // 5 pixels wide (tests 4-pixel 16-byte chunk + 1-pixel tail) x 2 rows with 8-byte stride padding
        let width = 5u32;
        let height = 2u32;
        let stride = 28u32; // 20 visible bytes + 8 padding bytes
        let mut src = vec![0xEEu8; (stride * height) as usize];

        for y in 0..height as usize {
            for x in 0..width as usize {
                let off = y * stride as usize + x * 4;
                src[off] = (10 + y * 20 + x) as u8; // B or R
                src[off + 1] = (50 + y * 10 + x) as u8; // G
                src[off + 2] = (150 + y * 5 + x) as u8; // R or B
                src[off + 3] = 0; // X=0 must become A=255
            }
        }

        let bgr_out = rgba_pixels(&src, width, height, stride, true).unwrap();
        assert_eq!(bgr_out.len(), (width * height * 4) as usize);
        for y in 0..height as usize {
            for x in 0..width as usize {
                let dst_off = (y * width as usize + x) * 4;
                assert_eq!(bgr_out[dst_off], (150 + y * 5 + x) as u8);
                assert_eq!(bgr_out[dst_off + 1], (50 + y * 10 + x) as u8);
                assert_eq!(bgr_out[dst_off + 2], (10 + y * 20 + x) as u8);
                assert_eq!(bgr_out[dst_off + 3], 255);
            }
        }

        let rgb_out = rgba_pixels(&src, width, height, stride, false).unwrap();
        assert_eq!(rgb_out.len(), (width * height * 4) as usize);
        for y in 0..height as usize {
            for x in 0..width as usize {
                let dst_off = (y * width as usize + x) * 4;
                assert_eq!(rgb_out[dst_off], (10 + y * 20 + x) as u8);
                assert_eq!(rgb_out[dst_off + 1], (50 + y * 10 + x) as u8);
                assert_eq!(rgb_out[dst_off + 2], (150 + y * 5 + x) as u8);
                assert_eq!(rgb_out[dst_off + 3], 255);
            }
        }
    }
}
