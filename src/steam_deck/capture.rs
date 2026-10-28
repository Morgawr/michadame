use crate::video::types::{ColorRange, RawFrame};
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

/// Copy only visible pixels, respecting padded scanlines and normalizing BGRx to RGBA.
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
    let mut rgba = Vec::with_capacity(row_bytes.checked_mul(height as usize)?);
    for row in bytes[..needed].chunks(stride) {
        for pixel in row[..row_bytes].chunks_exact(4) {
            let pixel = if bgr {
                [pixel[2], pixel[1], pixel[0], 255]
            } else {
                [pixel[0], pixel[1], pixel[2], 255]
            };
            rgba.extend_from_slice(&pixel);
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
}
