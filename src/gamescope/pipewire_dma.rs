use std::ffi::{c_char, c_void, CString};
use std::os::unix::io::{FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use crossbeam_channel::{Receiver, Sender};

#[repr(C)]
struct GamescopeDmaStreamOpaque {
    _private: [u8; 0],
}

type DmaFrameCb = unsafe extern "C" fn(
    user_data: *mut c_void,
    fd: i32,
    width: u32,
    height: u32,
    stride: u32,
    drm_fourcc: u32,
    modifier: u64,
);

type DmaStateCb = unsafe extern "C" fn(
    user_data: *mut c_void,
    state_code: i32,
    error_msg: *const c_char,
);

extern "C" {
    fn gamescope_dma_stream_start(
        target_object: *const c_char,
        on_frame: DmaFrameCb,
        on_state: DmaStateCb,
        user_data: *mut c_void,
    ) -> *mut GamescopeDmaStreamOpaque;

    fn gamescope_dma_stream_stop(stream: *mut GamescopeDmaStreamOpaque);
}

#[derive(Debug)]
pub struct DmaBufFrame {
    pub fd: OwnedFd,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub drm_fourcc: u32,
    pub modifier: u64,
    pub captured_at: i64,
}

struct StreamContext {
    tx: Sender<DmaBufFrame>,
    request_repaint: Arc<dyn Fn() + Send + Sync>,
    queue_drops: Arc<AtomicU64>,
}

unsafe extern "C" fn on_frame_trampoline(
    user_data: *mut c_void,
    fd: i32,
    width: u32,
    height: u32,
    stride: u32,
    drm_fourcc: u32,
    modifier: u64,
) {
    let ctx = &*(user_data as *const StreamContext);

    // Duplicate the DMA-BUF file descriptor so it remains valid in Rust
    let dup_fd = libc::dup(fd);
    if dup_fd < 0 {
        return;
    }

    let frame = DmaBufFrame {
        fd: OwnedFd::from_raw_fd(dup_fd),
        width,
        height,
        stride,
        drm_fourcc,
        modifier,
        captured_at: crate::replay::now_us(),
    };

    // Try sending frame to UI, tracking drops if channel is full
    match ctx.tx.try_send(frame) {
        Ok(()) => (ctx.request_repaint)(),
        Err(crossbeam_channel::TrySendError::Full(_)) => {
            ctx.queue_drops.fetch_add(1, Ordering::Relaxed);
        }
        Err(crossbeam_channel::TrySendError::Disconnected(_)) => {}
    }
}

unsafe extern "C" fn on_state_trampoline(
    _user_data: *mut c_void,
    state_code: i32,
    error_msg: *const c_char,
) {
    let err_str = if !error_msg.is_null() {
        std::ffi::CStr::from_ptr(error_msg).to_string_lossy()
    } else {
        std::borrow::Cow::Borrowed("none")
    };
    match state_code {
        3 => tracing::info!("Gamescope DMA-BUF PipeWire stream active and streaming"),
        2 => tracing::debug!("Gamescope DMA-BUF PipeWire stream paused"),
        1 => tracing::debug!("Gamescope DMA-BUF PipeWire stream connecting..."),
        0 => tracing::debug!("Gamescope DMA-BUF PipeWire stream unconnected"),
        -1 => tracing::warn!("Gamescope DMA-BUF PipeWire stream error: {}", err_str),
        _ => {}
    }
}

pub struct GamescopeDmaBufHandle {
    stream: *mut GamescopeDmaStreamOpaque,
    _ctx: Box<StreamContext>,
    pub rx: Receiver<DmaBufFrame>,
}

unsafe impl Send for GamescopeDmaBufHandle {}
unsafe impl Sync for GamescopeDmaBufHandle {}

impl Drop for GamescopeDmaBufHandle {
    fn drop(&mut self) {
        if !self.stream.is_null() {
            unsafe {
                gamescope_dma_stream_stop(self.stream);
            }
            self.stream = std::ptr::null_mut();
        }
    }
}

pub fn start_gamescope_dma_stream(
    target_object: Option<&str>,
    request_repaint: Arc<dyn Fn() + Send + Sync>,
    queue_drops: Arc<AtomicU64>,
) -> Option<GamescopeDmaBufHandle> {
    let (tx, rx) = crossbeam_channel::bounded(4);
    let mut ctx = Box::new(StreamContext {
        tx,
        request_repaint,
        queue_drops,
    });

    let target_c = target_object.and_then(|s| CString::new(s).ok());
    let target_ptr = target_c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());

    let stream = unsafe {
        gamescope_dma_stream_start(
            target_ptr,
            on_frame_trampoline,
            on_state_trampoline,
            &mut *ctx as *mut StreamContext as *mut c_void,
        )
    };

    if stream.is_null() {
        tracing::warn!("Failed to start native PipeWire DMA-BUF stream");
        return None;
    }

    Some(GamescopeDmaBufHandle {
        stream,
        _ctx: ctx,
        rx,
    })
}
