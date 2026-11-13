use crossbeam_channel::{Receiver, Sender};
use std::ffi::{c_char, c_void, CString};
use std::os::unix::io::{FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

pub const MAX_DMA_PLANES: usize = 4;
pub const DMA_CHANNEL_CAPACITY: usize = 4;
const MAX_REMOVED_BUFFER_HISTORY: usize = 128;

const DRM_FORMAT_MOD_LINEAR: u64 = 0;
const DRM_FORMAT_MOD_INVALID: u64 = (1u64 << 56) - 1;

/// Sticky safety fallback: set automatically if EGL fails to import a non-linear modifier so the
/// next stream connection negotiates `DRM_FORMAT_MOD_LINEAR` directly.
static FORCE_LINEAR_MODIFIER: AtomicBool = AtomicBool::new(false);

pub fn is_linear_modifier_forced() -> bool {
    FORCE_LINEAR_MODIFIER.load(Ordering::Acquire)
}

#[repr(C)]
struct GamescopeDmaStreamOpaque {
    _private: [u8; 0],
}

type DmaFrameCb = unsafe extern "C" fn(
    user_data: *mut c_void,
    buffer_id: u32,
    pool_generation: u64,
    n_planes: u32,
    fds: *const i32,
    offsets: *const u32,
    strides: *const u32,
    width: u32,
    height: u32,
    drm_fourcc: u32,
    modifier: u64,
);

type DmaStateCb =
    unsafe extern "C" fn(user_data: *mut c_void, state_code: i32, error_msg: *const c_char);

type DmaRemoveBufferCb =
    unsafe extern "C" fn(user_data: *mut c_void, buffer_id: u32, pool_generation: u64);

extern "C" {
    fn gamescope_dma_stream_start(
        target_object: *const c_char,
        modifiers: *const u64,
        n_modifiers: usize,
        on_frame: DmaFrameCb,
        on_state: DmaStateCb,
        on_remove_buffer: DmaRemoveBufferCb,
        user_data: *mut c_void,
    ) -> *mut GamescopeDmaStreamOpaque;

    fn gamescope_dma_stream_stop(stream: *mut GamescopeDmaStreamOpaque);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DmaCaptureHealth {
    Connecting,
    Streaming,
    Failed(String),
}

#[derive(Debug)]
pub struct DmaBufPlane {
    pub fd: OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

#[derive(Debug)]
pub struct DmaBufFrame {
    pub buffer_id: u32,
    pub pool_generation: u64,
    pub n_planes: u32,
    pub planes: [Option<DmaBufPlane>; MAX_DMA_PLANES],
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub drm_fourcc: u32,
    pub modifier: u64,
    pub captured_at: i64,
}

impl DmaBufFrame {
    pub fn plane(&self, index: usize) -> Option<&DmaBufPlane> {
        if index < (self.n_planes as usize).min(MAX_DMA_PLANES) {
            self.planes[index].as_ref()
        } else {
            None
        }
    }
}

static ACTIVE_STREAM_CONTEXT: Mutex<Option<Weak<StreamContext>>> = Mutex::new(None);

pub(crate) fn notify_active_import_failure(reason: &str) {
    let ctx = ACTIVE_STREAM_CONTEXT
        .lock()
        .ok()
        .and_then(|g| g.as_ref().and_then(|w| w.upgrade()));
    if let Some(ctx) = ctx {
        ctx.record_import_failure(reason);
    }
}

pub(crate) fn notify_active_import_success() {
    let ctx = ACTIVE_STREAM_CONTEXT
        .lock()
        .ok()
        .and_then(|g| g.as_ref().and_then(|w| w.upgrade()));
    if let Some(ctx) = ctx {
        ctx.record_import_success();
    }
}

fn effective_stream_modifiers(mut queried: Vec<u64>, force_linear: bool) -> Vec<u64> {
    if force_linear {
        return vec![DRM_FORMAT_MOD_LINEAR];
    }
    queried.retain(|&m| m != DRM_FORMAT_MOD_INVALID);
    let mut deduped = Vec::with_capacity(queried.len() + 1);
    for m in queried {
        if !deduped.contains(&m) {
            deduped.push(m);
        }
    }
    if !deduped.contains(&DRM_FORMAT_MOD_LINEAR) {
        deduped.push(DRM_FORMAT_MOD_LINEAR);
    }
    deduped
}

pub(crate) struct StreamContext {
    tx: Sender<DmaBufFrame>,
    request_repaint: Arc<dyn Fn() + Send + Sync>,
    queue_drops: Arc<AtomicU64>,
    stopping: AtomicBool,
    health: Mutex<DmaCaptureHealth>,
    frames_received: AtomicU64,
    frames_imported: AtomicU64,
    import_failures: AtomicU64,
    pool_generation: AtomicU64,
    last_modifier: AtomicU64,
    removed_buffers: Mutex<Vec<u32>>,
}

impl StreamContext {
    fn new(
        capacity: usize,
        request_repaint: Arc<dyn Fn() + Send + Sync>,
        queue_drops: Arc<AtomicU64>,
    ) -> (Arc<Self>, Receiver<DmaBufFrame>) {
        let (tx, rx) = crossbeam_channel::bounded(capacity.max(1));
        let ctx = Arc::new(Self {
            tx,
            request_repaint,
            queue_drops,
            stopping: AtomicBool::new(false),
            health: Mutex::new(DmaCaptureHealth::Connecting),
            frames_received: AtomicU64::new(0),
            frames_imported: AtomicU64::new(0),
            import_failures: AtomicU64::new(0),
            pool_generation: AtomicU64::new(1),
            last_modifier: AtomicU64::new(DRM_FORMAT_MOD_LINEAR),
            removed_buffers: Mutex::new(Vec::new()),
        });
        (ctx, rx)
    }

    fn enqueue_frame(&self, frame: DmaBufFrame) {
        if self.stopping.load(Ordering::Acquire) {
            return;
        }

        self.last_modifier.store(frame.modifier, Ordering::Release);
        self.pool_generation
            .store(frame.pool_generation.max(1), Ordering::Release);
        let prev = self.frames_received.fetch_add(1, Ordering::Relaxed);
        if prev == 0 {
            if let Ok(mut health) = self.health.lock() {
                if matches!(*health, DmaCaptureHealth::Connecting) {
                    *health = DmaCaptureHealth::Streaming;
                }
            }
        }

        // Try sending frame to UI, tracking drops if channel is full
        match self.tx.try_send(frame) {
            Ok(()) => (self.request_repaint)(),
            Err(crossbeam_channel::TrySendError::Full(_)) => {
                self.queue_drops.fetch_add(1, Ordering::Relaxed);
            }
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => {}
        }
    }

    fn handle_state_change(&self, state_code: i32, err_str: &str) {
        if self.stopping.load(Ordering::Acquire) {
            return;
        }
        match state_code {
            3 => {
                if let Ok(mut health) = self.health.lock() {
                    if !matches!(*health, DmaCaptureHealth::Failed(_)) {
                        *health = DmaCaptureHealth::Streaming;
                    }
                }
            }
            2 | 1 | 0 => {
                if let Ok(mut health) = self.health.lock() {
                    if !matches!(
                        *health,
                        DmaCaptureHealth::Failed(_) | DmaCaptureHealth::Streaming
                    ) {
                        *health = DmaCaptureHealth::Connecting;
                    }
                }
            }
            -1 => {
                let msg = if !err_str.is_empty() && err_str != "none" {
                    format!("PipeWire DMA-BUF stream error: {err_str}")
                } else {
                    "PipeWire DMA-BUF stream error".to_string()
                };
                if let Ok(mut health) = self.health.lock() {
                    *health = DmaCaptureHealth::Failed(msg);
                }
                (self.request_repaint)();
            }
            _ => {}
        }
    }

    fn handle_buffer_removed(&self, buffer_id: u32, pool_generation: u64) {
        if buffer_id == 0 {
            return;
        }
        if pool_generation > 0 {
            self.pool_generation
                .store(pool_generation, Ordering::Release);
        }
        if let Ok(mut removed) = self.removed_buffers.lock() {
            if !removed.contains(&buffer_id) {
                if removed.len() >= MAX_REMOVED_BUFFER_HISTORY {
                    removed.remove(0);
                }
                removed.push(buffer_id);
            }
        }
        crate::gamescope::egl::notify_dma_buffer_removed(buffer_id);
    }

    fn record_import_failure(&self, reason: &str) {
        let failures = self.import_failures.fetch_add(1, Ordering::AcqRel) + 1;
        let active_mod = self.last_modifier.load(Ordering::Acquire);
        if active_mod != DRM_FORMAT_MOD_LINEAR && active_mod != DRM_FORMAT_MOD_INVALID {
            if !FORCE_LINEAR_MODIFIER.swap(true, Ordering::AcqRel) {
                tracing::warn!(
                    "DMA-BUF import failed with non-linear modifier 0x{:016x} ({reason}); forcing linear fallback on stream restart",
                    active_mod
                );
            }
        }
        if failures > 3 && self.frames_imported.load(Ordering::Acquire) == 0 {
            if let Ok(mut health) = self.health.lock() {
                if !matches!(*health, DmaCaptureHealth::Failed(_)) {
                    *health = DmaCaptureHealth::Failed(format!(
                        "DMA-BUF import failed {failures} times without a valid frame: {reason}"
                    ));
                }
            }
        }
    }

    fn record_import_success(&self) {
        self.frames_imported.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut health) = self.health.lock() {
            if matches!(*health, DmaCaptureHealth::Connecting) {
                *health = DmaCaptureHealth::Streaming;
            }
        }
    }

    fn health(&self) -> DmaCaptureHealth {
        self.health
            .lock()
            .map(|h| h.clone())
            .unwrap_or(DmaCaptureHealth::Connecting)
    }

    fn take_removed_buffers(&self) -> Vec<u32> {
        self.removed_buffers
            .lock()
            .map(|mut list| std::mem::take(&mut *list))
            .unwrap_or_default()
    }
}

unsafe fn handle_raw_frame_with_dup<F>(
    ctx: &StreamContext,
    buffer_id: u32,
    pool_generation: u64,
    n_planes: u32,
    fds: *const i32,
    offsets: *const u32,
    strides: *const u32,
    width: u32,
    height: u32,
    drm_fourcc: u32,
    modifier: u64,
    mut dup_fn: F,
) where
    F: FnMut(i32) -> i32,
{
    if ctx.stopping.load(Ordering::Acquire) {
        return;
    }
    let plane_count = n_planes as usize;
    if plane_count == 0 || plane_count > MAX_DMA_PLANES {
        return;
    }
    if fds.is_null() || offsets.is_null() || strides.is_null() {
        return;
    }

    ctx.last_modifier.store(modifier, Ordering::Release);

    // Check channel fullness BEFORE issuing dup() syscalls so dropped frames never pay
    // dup() + close() kernel file-table overhead.
    if ctx.tx.is_full() {
        ctx.queue_drops.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let fds_slice = std::slice::from_raw_parts(fds, plane_count);
    let offsets_slice = std::slice::from_raw_parts(offsets, plane_count);
    let strides_slice = std::slice::from_raw_parts(strides, plane_count);

    let mut planes: [Option<DmaBufPlane>; MAX_DMA_PLANES] = [None, None, None, None];
    for i in 0..plane_count {
        let raw_fd = fds_slice[i];
        if raw_fd < 0 {
            // Dropping `planes` automatically closes any fds already duplicated in 0..i.
            return;
        }
        let dup_fd = dup_fn(raw_fd);
        if dup_fd < 0 {
            // Dropping `planes` automatically closes any fds already duplicated in 0..i.
            return;
        }
        planes[i] = Some(DmaBufPlane {
            fd: OwnedFd::from_raw_fd(dup_fd),
            offset: offsets_slice[i],
            stride: strides_slice[i],
        });
    }

    let frame = DmaBufFrame {
        buffer_id,
        pool_generation: pool_generation.max(1),
        n_planes,
        planes,
        width,
        height,
        stride: strides_slice[0],
        drm_fourcc,
        modifier,
        captured_at: crate::replay::now_us(),
    };

    ctx.enqueue_frame(frame);
}

unsafe extern "C" fn on_frame_trampoline(
    user_data: *mut c_void,
    buffer_id: u32,
    pool_generation: u64,
    n_planes: u32,
    fds: *const i32,
    offsets: *const u32,
    strides: *const u32,
    width: u32,
    height: u32,
    drm_fourcc: u32,
    modifier: u64,
) {
    let ctx = &*(user_data as *const StreamContext);
    handle_raw_frame_with_dup(
        ctx,
        buffer_id,
        pool_generation,
        n_planes,
        fds,
        offsets,
        strides,
        width,
        height,
        drm_fourcc,
        modifier,
        |fd| libc::dup(fd),
    );
}

unsafe extern "C" fn on_state_trampoline(
    user_data: *mut c_void,
    state_code: i32,
    error_msg: *const c_char,
) {
    let ctx = &*(user_data as *const StreamContext);
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
    ctx.handle_state_change(state_code, &err_str);
}

unsafe extern "C" fn on_remove_buffer_trampoline(
    user_data: *mut c_void,
    buffer_id: u32,
    pool_generation: u64,
) {
    let ctx = &*(user_data as *const StreamContext);
    ctx.handle_buffer_removed(buffer_id, pool_generation);
}

pub struct GamescopeDmaBufHandle {
    stream: *mut GamescopeDmaStreamOpaque,
    ctx: Arc<StreamContext>,
    pub rx: Receiver<DmaBufFrame>,
}

pub type PwDmaCapture = GamescopeDmaBufHandle;

unsafe impl Send for GamescopeDmaBufHandle {}
unsafe impl Sync for GamescopeDmaBufHandle {}

impl GamescopeDmaBufHandle {
    pub fn health(&self) -> DmaCaptureHealth {
        self.ctx.health()
    }

    pub fn record_import_failure(&self, reason: &str) {
        self.ctx.record_import_failure(reason);
    }

    pub fn record_import_success(&self) {
        self.ctx.record_import_success();
    }

    pub fn frames_received(&self) -> u64 {
        self.ctx.frames_received.load(Ordering::Acquire)
    }

    pub fn frames_imported(&self) -> u64 {
        self.ctx.frames_imported.load(Ordering::Acquire)
    }

    pub fn import_failures(&self) -> u64 {
        self.ctx.import_failures.load(Ordering::Acquire)
    }

    pub fn dropped_frames(&self) -> u64 {
        self.ctx.queue_drops.load(Ordering::Relaxed)
    }

    pub fn pool_generation(&self) -> u64 {
        self.ctx.pool_generation.load(Ordering::Acquire)
    }

    pub fn take_removed_buffers(&self) -> Vec<u32> {
        self.ctx.take_removed_buffers()
    }

    pub fn failure_reason(&self) -> Option<String> {
        match self.health() {
            DmaCaptureHealth::Failed(reason) => Some(reason),
            _ => None,
        }
    }
}

impl Drop for GamescopeDmaBufHandle {
    fn drop(&mut self) {
        self.ctx.stopping.store(true, Ordering::Release);
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
    let (ctx, rx) = StreamContext::new(DMA_CHANNEL_CAPACITY, request_repaint, queue_drops);

    if let Ok(mut active) = ACTIVE_STREAM_CONTEXT.lock() {
        *active = Some(Arc::downgrade(&ctx));
    }

    let force_linear = FORCE_LINEAR_MODIFIER.load(Ordering::Acquire);
    let modifiers = if force_linear {
        tracing::info!(
            "FORCE_LINEAR_MODIFIER active; starting Gamescope DMA-BUF stream with DRM_FORMAT_MOD_LINEAR"
        );
        vec![DRM_FORMAT_MOD_LINEAR]
    } else {
        effective_stream_modifiers(
            crate::gamescope::egl::query_supported_dmabuf_modifiers(),
            false,
        )
    };

    let target_c = target_object.and_then(|s| CString::new(s).ok());
    let target_ptr = target_c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());

    let stream = unsafe {
        gamescope_dma_stream_start(
            target_ptr,
            modifiers.as_ptr(),
            modifiers.len(),
            on_frame_trampoline,
            on_state_trampoline,
            on_remove_buffer_trampoline,
            Arc::as_ptr(&ctx) as *mut c_void,
        )
    };

    if stream.is_null() {
        ctx.stopping.store(true, Ordering::Release);
        tracing::warn!("Failed to start native PipeWire DMA-BUF stream");
        return None;
    }

    Some(GamescopeDmaBufHandle { stream, ctx, rx })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::io::AsRawFd;
    use std::sync::atomic::AtomicUsize;

    static TEST_GLOBAL_LOCK: Mutex<()> = Mutex::new(());

    fn make_dummy_fd() -> OwnedFd {
        let fd = unsafe { libc::dup(0) };
        assert!(fd >= 0, "dup(0) failed");
        unsafe { OwnedFd::from_raw_fd(fd) }
    }

    fn make_test_frame(buffer_id: u32, pool_generation: u64) -> DmaBufFrame {
        DmaBufFrame {
            buffer_id,
            pool_generation,
            n_planes: 1,
            planes: [
                Some(DmaBufPlane {
                    fd: make_dummy_fd(),
                    offset: 0,
                    stride: 7680,
                }),
                None,
                None,
                None,
            ],
            width: 1920,
            height: 1080,
            stride: 7680,
            drm_fourcc: 0x34325258,
            modifier: DRM_FORMAT_MOD_LINEAR,
            captured_at: 100,
        }
    }

    #[test]
    fn test_dma_capture_health_transitions() {
        let (ctx, rx) = StreamContext::new(
            DMA_CHANNEL_CAPACITY,
            Arc::new(|| {}),
            Arc::new(AtomicU64::new(0)),
        );
        let handle = GamescopeDmaBufHandle {
            stream: std::ptr::null_mut(),
            ctx: ctx.clone(),
            rx,
        };

        assert_eq!(handle.health(), DmaCaptureHealth::Connecting);
        assert!(handle.failure_reason().is_none());

        // Transient UNCONNECTED (0) while waiting for gamescope node stays Connecting
        ctx.handle_state_change(1, "none");
        ctx.handle_state_change(0, "none");
        assert_eq!(handle.health(), DmaCaptureHealth::Connecting);

        // Receiving a frame transitions to Streaming
        ctx.enqueue_frame(make_test_frame(1, 1));
        assert_eq!(handle.health(), DmaCaptureHealth::Streaming);

        // 4 consecutive import failures with 0 imported frames transitions to Failed
        handle.record_import_failure("unsupported modifier");
        handle.record_import_failure("unsupported modifier");
        handle.record_import_failure("unsupported modifier");
        assert_eq!(handle.import_failures(), 3);
        handle.record_import_failure("unsupported modifier");
        assert_eq!(handle.import_failures(), 4);
        assert!(matches!(handle.health(), DmaCaptureHealth::Failed(_)));
        assert!(handle.failure_reason().is_some());

        // Stream state error callback (-1) also transitions health to Failed
        let (ctx2, rx2) = StreamContext::new(
            DMA_CHANNEL_CAPACITY,
            Arc::new(|| {}),
            Arc::new(AtomicU64::new(0)),
        );
        let handle2 = GamescopeDmaBufHandle {
            stream: std::ptr::null_mut(),
            ctx: ctx2.clone(),
            rx: rx2,
        };
        ctx2.handle_state_change(-1, "pipewire error");
        assert!(matches!(handle2.health(), DmaCaptureHealth::Failed(_)));
    }

    #[test]
    fn test_removed_buffer_tracking_and_generation() {
        let (ctx, rx) = StreamContext::new(
            DMA_CHANNEL_CAPACITY,
            Arc::new(|| {}),
            Arc::new(AtomicU64::new(0)),
        );
        let handle = GamescopeDmaBufHandle {
            stream: std::ptr::null_mut(),
            ctx: ctx.clone(),
            rx,
        };

        ctx.handle_buffer_removed(4, 2);
        ctx.handle_buffer_removed(5, 2);
        ctx.handle_buffer_removed(4, 2); // duplicate ignored
        assert_eq!(handle.pool_generation(), 2);
        assert_eq!(handle.take_removed_buffers(), vec![4, 5]);
        assert!(handle.take_removed_buffers().is_empty());
    }

    #[test]
    fn test_drop_before_dup_when_channel_is_full() {
        let queue_drops = Arc::new(AtomicU64::new(0));
        let (ctx, rx) = StreamContext::new(2, Arc::new(|| {}), queue_drops.clone());
        let dup_calls = AtomicUsize::new(0);

        let fds = [0i32, 0i32];
        let offsets = [0u32, 8_294_400u32];
        let strides = [7680u32, 256u32];

        unsafe {
            // Fill capacity (2 multi-plane frames -> 4 dup calls)
            for buf_id in 1..=2 {
                handle_raw_frame_with_dup(
                    &ctx,
                    buf_id,
                    1,
                    2,
                    fds.as_ptr(),
                    offsets.as_ptr(),
                    strides.as_ptr(),
                    1920,
                    1080,
                    0x34325258,
                    0x0200_0000_0020_6b03,
                    |fd| {
                        dup_calls.fetch_add(1, Ordering::SeqCst);
                        libc::dup(fd)
                    },
                );
            }
            assert_eq!(dup_calls.load(Ordering::SeqCst), 4);
            assert_eq!(queue_drops.load(Ordering::Relaxed), 0);
            assert!(ctx.tx.is_full());

            // 3rd frame arrives while channel is full: MUST drop BEFORE calling dup()!
            handle_raw_frame_with_dup(
                &ctx,
                3,
                1,
                2,
                fds.as_ptr(),
                offsets.as_ptr(),
                strides.as_ptr(),
                1920,
                1080,
                0x34325258,
                0x0200_0000_0020_6b03,
                |fd| {
                    dup_calls.fetch_add(1, Ordering::SeqCst);
                    libc::dup(fd)
                },
            );
        }

        // Verify zero extra dup() calls occurred and drop counter incremented
        assert_eq!(dup_calls.load(Ordering::SeqCst), 4);
        assert_eq!(queue_drops.load(Ordering::Relaxed), 1);

        let f1 = rx.try_recv().expect("expected first multi-plane frame");
        assert_eq!(f1.n_planes, 2);
        assert_eq!(f1.plane(0).unwrap().offset, 0);
        assert_eq!(f1.plane(0).unwrap().stride, 7680);
        assert_eq!(f1.plane(1).unwrap().offset, 8_294_400);
        assert_eq!(f1.plane(1).unwrap().stride, 256);
        assert!(f1.plane(2).is_none());
    }

    #[test]
    fn test_partial_dup_failure_closes_already_duped_fds() {
        let (ctx, rx) = StreamContext::new(2, Arc::new(|| {}), Arc::new(AtomicU64::new(0)));
        let fds = [0i32, 0i32, 0i32];
        let offsets = [0u32, 4096u32, 8192u32];
        let strides = [7680u32, 512u32, 64u32];
        let mut recorded_dups = Vec::new();
        let mut step = 0usize;

        unsafe {
            handle_raw_frame_with_dup(
                &ctx,
                7,
                1,
                3,
                fds.as_ptr(),
                offsets.as_ptr(),
                strides.as_ptr(),
                1920,
                1080,
                0x34325258,
                1,
                |fd| {
                    step += 1;
                    if step == 3 {
                        -1
                    } else {
                        let d = libc::dup(fd);
                        recorded_dups.push(d);
                        d
                    }
                },
            );
        }

        assert!(rx.try_recv().is_err());
        assert_eq!(recorded_dups.len(), 2);
        // Both fds from planes 0 and 1 must have been closed when `planes` dropped on error.
        for dup_fd in recorded_dups {
            let flags = unsafe { libc::fcntl(dup_fd, libc::F_GETFD) };
            assert_eq!(flags, -1, "expected leaked dup_fd {dup_fd} to be closed");
        }
    }

    #[test]
    fn test_nonlinear_import_failure_forces_linear_fallback() {
        let _guard = TEST_GLOBAL_LOCK.lock().unwrap();
        FORCE_LINEAR_MODIFIER.store(false, Ordering::SeqCst);

        let (ctx, _rx) = StreamContext::new(4, Arc::new(|| {}), Arc::new(AtomicU64::new(0)));
        if let Ok(mut active) = ACTIVE_STREAM_CONTEXT.lock() {
            *active = Some(Arc::downgrade(&ctx));
        }

        let mut tiled_frame = make_test_frame(1, 1);
        tiled_frame.modifier = 0x0200_0000_0020_6b03; // Non-linear AMD tiled/DCC modifier
        ctx.enqueue_frame(tiled_frame);
        assert!(!is_linear_modifier_forced());

        notify_active_import_failure("eglCreateImageKHR failed");
        assert!(is_linear_modifier_forced());
        assert_eq!(
            effective_stream_modifiers(vec![0x0200_0000_0020_6b03, 0], is_linear_modifier_forced()),
            vec![DRM_FORMAT_MOD_LINEAR]
        );

        let dummy_probe = make_dummy_fd();
        assert!(dummy_probe.as_raw_fd() >= 0);

        FORCE_LINEAR_MODIFIER.store(false, Ordering::SeqCst);
        if let Ok(mut active) = ACTIVE_STREAM_CONTEXT.lock() {
            *active = None;
        }
    }
}
