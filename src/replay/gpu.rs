//! Read back the already-shaded game rectangle, before egui overlays. No second shader render.
//! Budgeted fenced PBO queue; retain unfinished/blocked transfers without waiting.
use super::{Runtime, VideoFrame};
use crate::video::gpu::geometry::RenderedArea;
use eframe::glow::{self, HasContext};
use std::{cell::Cell, collections::VecDeque, sync::atomic::Ordering};

#[derive(Clone)]
pub struct BroadcastSink {
    pub tx: crossbeam_channel::Sender<VideoFrame>,
    pub free_rx: crossbeam_channel::Receiver<Vec<u8>>,
    pub dropped: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

#[derive(Clone, Copy)]
pub(crate) enum SlotFence {
    Gl(glow::Fence),
    #[cfg(test)]
    Mock(u64),
}

struct Slot {
    buffer: glow::Buffer,
    fence: SlotFence,
    at: i64,
    generation: u64,
    broadcast_sent: bool,
}
// SAFETY: fence handles are opaque. Slots are only accessed by egui paint/on_exit
// with the owning GL context current; no GL operation happens in Drop or a worker.
unsafe impl Send for Slot {}

pub(crate) unsafe trait GlBackend {
    unsafe fn delete_sync(&self, fence: SlotFence);
    unsafe fn delete_buffer(&self, buffer: glow::Buffer);
    unsafe fn delete_framebuffer(&self, fbo: glow::Framebuffer);
    unsafe fn delete_texture(&self, tex: glow::Texture);
    unsafe fn get_parameter_i32(&self, parameter: u32) -> i32;
    unsafe fn pixel_store_i32(&self, parameter: u32, value: i32);
    unsafe fn client_wait_sync(&self, fence: SlotFence, flags: u32, timeout: i32) -> u32;
    unsafe fn bind_buffer(&self, target: u32, buffer: Option<glow::Buffer>);
    unsafe fn get_buffer_sub_data(&self, target: u32, offset: i32, dst: &mut [u8]);
    unsafe fn create_buffer(&self) -> Result<glow::Buffer, String>;
    unsafe fn buffer_data_size(&self, target: u32, size: i32, usage: u32);
    unsafe fn get_error(&self) -> u32;
    unsafe fn create_texture(&self) -> Result<glow::Texture, String>;
    unsafe fn bind_texture(&self, target: u32, texture: Option<glow::Texture>);
    unsafe fn tex_image_2d_empty(&self, width: i32, height: i32);
    unsafe fn tex_parameter_i32(&self, target: u32, parameter: u32, value: i32);
    unsafe fn create_framebuffer(&self) -> Result<glow::Framebuffer, String>;
    unsafe fn bind_framebuffer(&self, target: u32, framebuffer: Option<glow::Framebuffer>);
    unsafe fn framebuffer_texture_2d(
        &self,
        target: u32,
        attachment: u32,
        textarget: u32,
        texture: Option<glow::Texture>,
        level: i32,
    );
    unsafe fn check_framebuffer_status(&self, target: u32) -> u32;
    unsafe fn is_enabled(&self, parameter: u32) -> bool;
    unsafe fn disable(&self, parameter: u32);
    unsafe fn enable(&self, parameter: u32);
    unsafe fn blit_framebuffer(
        &self,
        src_x0: i32,
        src_y0: i32,
        src_x1: i32,
        src_y1: i32,
        dst_x0: i32,
        dst_y0: i32,
        dst_x1: i32,
        dst_y1: i32,
        mask: u32,
        filter: u32,
    );
    unsafe fn read_pixels_to_pbo(&self, x: i32, y: i32, width: i32, height: i32);
    unsafe fn fence_sync(&self, condition: u32, flags: u32) -> Result<SlotFence, String>;
}

unsafe impl GlBackend for glow::Context {
    unsafe fn delete_sync(&self, fence: SlotFence) {
        match fence {
            SlotFence::Gl(f) => HasContext::delete_sync(self, f),
            #[cfg(test)]
            SlotFence::Mock(_) => {}
        }
    }
    unsafe fn delete_buffer(&self, buffer: glow::Buffer) {
        HasContext::delete_buffer(self, buffer);
    }
    unsafe fn delete_framebuffer(&self, fbo: glow::Framebuffer) {
        HasContext::delete_framebuffer(self, fbo);
    }
    unsafe fn delete_texture(&self, tex: glow::Texture) {
        HasContext::delete_texture(self, tex);
    }
    unsafe fn get_parameter_i32(&self, parameter: u32) -> i32 {
        HasContext::get_parameter_i32(self, parameter)
    }
    unsafe fn pixel_store_i32(&self, parameter: u32, value: i32) {
        HasContext::pixel_store_i32(self, parameter, value);
    }
    unsafe fn client_wait_sync(&self, fence: SlotFence, flags: u32, timeout: i32) -> u32 {
        match fence {
            SlotFence::Gl(f) => HasContext::client_wait_sync(self, f, flags, timeout),
            #[cfg(test)]
            SlotFence::Mock(_) => glow::WAIT_FAILED,
        }
    }
    unsafe fn bind_buffer(&self, target: u32, buffer: Option<glow::Buffer>) {
        HasContext::bind_buffer(self, target, buffer);
    }
    unsafe fn get_buffer_sub_data(&self, target: u32, offset: i32, dst: &mut [u8]) {
        HasContext::get_buffer_sub_data(self, target, offset, dst);
    }
    unsafe fn create_buffer(&self) -> Result<glow::Buffer, String> {
        HasContext::create_buffer(self)
    }
    unsafe fn buffer_data_size(&self, target: u32, size: i32, usage: u32) {
        HasContext::buffer_data_size(self, target, size, usage);
    }
    unsafe fn get_error(&self) -> u32 {
        HasContext::get_error(self)
    }
    unsafe fn create_texture(&self) -> Result<glow::Texture, String> {
        HasContext::create_texture(self)
    }
    unsafe fn bind_texture(&self, target: u32, texture: Option<glow::Texture>) {
        HasContext::bind_texture(self, target, texture);
    }
    unsafe fn tex_image_2d_empty(&self, width: i32, height: i32) {
        HasContext::tex_image_2d(
            self,
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            width,
            height,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            None,
        );
    }
    unsafe fn tex_parameter_i32(&self, target: u32, parameter: u32, value: i32) {
        HasContext::tex_parameter_i32(self, target, parameter, value);
    }
    unsafe fn create_framebuffer(&self) -> Result<glow::Framebuffer, String> {
        HasContext::create_framebuffer(self)
    }
    unsafe fn bind_framebuffer(&self, target: u32, framebuffer: Option<glow::Framebuffer>) {
        HasContext::bind_framebuffer(self, target, framebuffer);
    }
    unsafe fn framebuffer_texture_2d(
        &self,
        target: u32,
        attachment: u32,
        textarget: u32,
        texture: Option<glow::Texture>,
        level: i32,
    ) {
        HasContext::framebuffer_texture_2d(self, target, attachment, textarget, texture, level);
    }
    unsafe fn check_framebuffer_status(&self, target: u32) -> u32 {
        HasContext::check_framebuffer_status(self, target)
    }
    unsafe fn is_enabled(&self, parameter: u32) -> bool {
        HasContext::is_enabled(self, parameter)
    }
    unsafe fn disable(&self, parameter: u32) {
        HasContext::disable(self, parameter);
    }
    unsafe fn enable(&self, parameter: u32) {
        HasContext::enable(self, parameter);
    }
    unsafe fn blit_framebuffer(
        &self,
        src_x0: i32,
        src_y0: i32,
        src_x1: i32,
        src_y1: i32,
        dst_x0: i32,
        dst_y0: i32,
        dst_x1: i32,
        dst_y1: i32,
        mask: u32,
        filter: u32,
    ) {
        HasContext::blit_framebuffer(
            self, src_x0, src_y0, src_x1, src_y1, dst_x0, dst_y0, dst_x1, dst_y1, mask, filter,
        );
    }
    unsafe fn read_pixels_to_pbo(&self, x: i32, y: i32, width: i32, height: i32) {
        HasContext::read_pixels(
            self,
            x,
            y,
            width,
            height,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::BufferOffset(0),
        );
    }
    unsafe fn fence_sync(&self, condition: u32, flags: u32) -> Result<SlotFence, String> {
        HasContext::fence_sync(self, condition, flags).map(SlotFence::Gl)
    }
}
/// A transient/minimized viewport is not a new recording format. Commit only
/// valid dimensions that remain stable for 300 ms, retaining history meanwhile.
#[derive(Default)]
struct SurfaceState {
    size: (u32, u32),
    candidate: Option<((u32, u32), i64)>,
}
impl SurfaceState {
    /// None pauses readback; Some(true) commits a new recording size.
    fn observe(&mut self, size: (u32, u32), now: i64) -> Option<bool> {
        if size.0 == 0 || size.1 == 0 {
            self.candidate = None;
            return None;
        }
        if size == self.size {
            self.candidate = None;
            return Some(false);
        }
        let since = match self.candidate {
            Some((candidate, since)) if candidate == size => since,
            _ => {
                self.candidate = Some((size, now));
                return None;
            }
        };
        if now - since < 300_000 {
            return None;
        }
        self.size = size;
        self.candidate = None;
        Some(true)
    }
}

pub const MAX_REPLAY_WIDTH: u32 = 1920;
pub const MAX_REPLAY_HEIGHT: u32 = 1080;

pub fn target_capture_dimensions(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (0, 0);
    }
    if width <= MAX_REPLAY_WIDTH && height <= MAX_REPLAY_HEIGHT {
        return (width, height);
    }
    let scale_w = MAX_REPLAY_WIDTH as f32 / width as f32;
    let scale_h = MAX_REPLAY_HEIGHT as f32 / height as f32;
    let scale = scale_w.min(scale_h);
    // Keep even dimensions for NV12 video encoding compatibility
    let w = ((width as f32 * scale).round() as u32).max(2) & !1;
    let h = ((height as f32 * scale).round() as u32).max(2) & !1;
    (w, h)
}

fn fbo_from_param(param: i32) -> Option<glow::Framebuffer> {
    if param > 0 {
        std::num::NonZeroU32::new(param as u32).map(glow::NativeFramebuffer)
    } else {
        None
    }
}

fn tex_from_param(param: i32) -> Option<glow::Texture> {
    if param > 0 {
        std::num::NonZeroU32::new(param as u32).map(glow::NativeTexture)
    } else {
        None
    }
}

const TARGET_CAPTURE_INTERVAL_US: f64 = 1_000_000.0 / 60.0;
const CAPTURE_DISCONTINUITY_US: i64 = 100_000;

#[derive(Default)]
pub struct Readback {
    pending: VecDeque<Slot>,
    free: Vec<glow::Buffer>,
    surface: SurfaceState,
    last_at: i64,
    last_seen_at: Cell<i64>,
    last_accepted_at: Cell<i64>,
    next_target_us: Cell<f64>,
    active: Option<std::sync::Weak<super::Shared>>,
    requested: Option<u64>,
    downscale_fbo: Option<glow::Framebuffer>,
    downscale_tex: Option<glow::Texture>,
    downscale_size: (u32, u32),
}
impl Readback {
    fn observe_surface(&mut self, size: (u32, u32), at: i64, now: i64) -> Option<bool> {
        if at <= 0 {
            return None;
        }
        self.surface.observe(size, now)
    }
    fn release_buffers_with<G: GlBackend>(&mut self, gl: &G) {
        if let Some(shared) = self.active.as_ref().and_then(|w| w.upgrade()) {
            shared.gpu_pending.store(0, Ordering::Relaxed);
        }
        unsafe {
            for slot in self.pending.drain(..) {
                gl.delete_sync(slot.fence);
                gl.delete_buffer(slot.buffer);
            }
            for buffer in self.free.drain(..) {
                gl.delete_buffer(buffer);
            }
            if let Some(fbo) = self.downscale_fbo.take() {
                gl.delete_framebuffer(fbo);
            }
            if let Some(tex) = self.downscale_tex.take() {
                gl.delete_texture(tex);
            }
            self.downscale_size = (0, 0);
        }
        self.last_at = 0;
        self.last_seen_at.set(0);
        self.last_accepted_at.set(0);
        self.next_target_us.set(0.0);
        self.requested = None;
    }
    pub fn destroy(&mut self, gl: &glow::Context) {
        self.destroy_with(gl);
    }
    pub(crate) fn destroy_with<G: GlBackend>(&mut self, gl: &G) {
        self.release_buffers_with(gl);
        self.surface = SurfaceState::default();
        self.active = None;
    }

    /// Rate-limit capture requests to ~60 FPS (16,666.67 us target period)
    /// while preserving fractional phase across arbitrary display refresh rates
    /// (59.94, 60, 65, 75, 90, 120, 144 Hz).
    pub fn should_capture(&self, at: i64) -> bool {
        let prev_accepted = self.last_at.max(self.last_accepted_at.get());
        let prev_seen = self.last_seen_at.get().max(prev_accepted);
        if at <= 0 || at <= prev_accepted || at <= prev_seen {
            return false;
        }
        self.last_seen_at.set(at);
        if prev_seen <= 0 || (at - prev_seen) > CAPTURE_DISCONTINUITY_US {
            self.last_accepted_at.set(at);
            self.next_target_us
                .set(at as f64 + TARGET_CAPTURE_INTERVAL_US);
            return true;
        }
        let dt = (at - prev_seen) as f64;
        let target = self.next_target_us.get();
        if (at as f64) + 0.5 * dt >= target {
            self.last_accepted_at.set(at);
            let mut next = target + TARGET_CAPTURE_INTERVAL_US;
            if next < (at as f64) + 0.5 * TARGET_CAPTURE_INTERVAL_US {
                next = (at as f64) + TARGET_CAPTURE_INTERVAL_US;
            }
            self.next_target_us.set(next);
            true
        } else {
            false
        }
    }

    pub fn capture(
        &mut self,
        gl: &glow::Context,
        runtime: Option<&RuntimeView>,
        broadcast: Option<&BroadcastSink>,
        area: RenderedArea,
        at: i64,
    ) {
        self.capture_with(gl, runtime, broadcast, area, at);
    }

    pub(crate) fn capture_with<G: GlBackend>(
        &mut self,
        gl: &G,
        runtime: Option<&RuntimeView>,
        broadcast: Option<&BroadcastSink>,
        area: RenderedArea,
        at: i64,
    ) {
        let RenderedArea {
            x,
            y,
            width,
            height,
        } = area;
        let running_replay = runtime.filter(|r| !r.shared.stop.load(Ordering::Acquire));
        if running_replay.is_none() && broadcast.is_none() {
            self.destroy_with(gl);
            return;
        }
        if let Some(runtime) = running_replay {
            if self
                .active
                .as_ref()
                .and_then(|w| w.upgrade())
                .is_none_or(|old| !std::sync::Arc::ptr_eq(&old, &runtime.shared))
            {
                self.destroy_with(gl);
                self.active = Some(std::sync::Arc::downgrade(&runtime.shared));
            }
        } else if self.active.is_some() {
            self.active = None;
        }

        let (capture_w, capture_h) = target_capture_dimensions(width, height);

        // Missing/stale capture timestamps cannot establish a new recording
        // epoch. Real stream restarts already disable replay. In particular,
        // never confuse an out-of-order frame with a backwards system clock.
        let Some(resized) = self.observe_surface((capture_w, capture_h), at, super::now_us())
        else {
            return;
        };
        if resized {
            self.release_buffers_with(gl);
            if let Some(runtime) = running_replay {
                runtime.shared.generation.fetch_add(1, Ordering::AcqRel);
            }
        }

        let (slots, bytes, generation) = if let Some(runtime) = running_replay {
            let Some(plan) = runtime.config.queue_plan(capture_w, capture_h) else {
                runtime.shared.message(
                    "Replay stopped: work queue or RAM budget is too small for this image size",
                );
                runtime.shared.stop.store(true, Ordering::Release);
                return;
            };
            if plan
                .overhead
                .saturating_add(runtime.shared.clipboard.bytes())
                >= runtime.config.budget()
            {
                runtime.shared.message(
                    "Replay stopped: RAM budget is too small for this surface and the clipboard clip",
                );
                runtime.shared.stop.store(true, Ordering::Release);
                return;
            }
            let gen = runtime.shared.generation.load(Ordering::Acquire);
            runtime
                .shared
                .staging_bytes
                .fetch_max(plan.overhead, Ordering::AcqRel);
            runtime
                .shared
                .queue_slots
                .store(plan.slots, Ordering::Relaxed);
            runtime
                .shared
                .queue_bytes
                .store(plan.queue_bytes, Ordering::Relaxed);
            if self.requested != Some(gen)
                && runtime.queue.request(super::queue::PoolRequest {
                    generation: gen,
                    plan,
                })
            {
                self.requested = Some(gen);
            }
            (plan.slots, plan.frame_bytes, gen)
        } else {
            (4, capture_w as usize * capture_h as usize * 4, 0)
        };

        unsafe {
            let old = gl.get_parameter_i32(glow::PIXEL_PACK_BUFFER_BINDING);
            let alignment = gl.get_parameter_i32(glow::PACK_ALIGNMENT);
            let row_length = gl.get_parameter_i32(glow::PACK_ROW_LENGTH);
            let skip_rows = gl.get_parameter_i32(glow::PACK_SKIP_ROWS);
            let skip_pixels = gl.get_parameter_i32(glow::PACK_SKIP_PIXELS);
            gl.pixel_store_i32(glow::PACK_ROW_LENGTH, 0);
            gl.pixel_store_i32(glow::PACK_SKIP_ROWS, 0);
            gl.pixel_store_i32(glow::PACK_SKIP_PIXELS, 0);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            // Drain a small bounded batch so readback can catch up after a
            // stall. Never wait on a fence or discard a completed frame merely
            // because the encoder/CPU pool is busy.
            let copying_started = std::time::Instant::now();
            for _ in 0..4 {
                let Some(front_slot) = self.pending.front() else {
                    break;
                };
                if running_replay.is_some() && front_slot.generation != generation {
                    let stale = self.pending.pop_front().unwrap();
                    gl.delete_sync(stale.fence);
                    self.free.push(stale.buffer);
                    continue;
                }
                let result = gl.client_wait_sync(front_slot.fence, 0, 0);
                if result == glow::WAIT_FAILED {
                    if let Some(runtime) = running_replay {
                        runtime
                            .shared
                            .message("Replay stopped: GPU readback fence failed");
                        runtime.shared.stop.store(true, Ordering::Release);
                    }
                    break;
                }
                if result != glow::ALREADY_SIGNALED && result != glow::CONDITION_SATISFIED {
                    break;
                }

                if let Some(runtime) = running_replay {
                    if let Some(mut rgba) = runtime.queue.buffer(generation) {
                        let slot = self.pending.pop_front().unwrap();
                        debug_assert_eq!(rgba.len(), bytes);
                        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(slot.buffer));
                        gl.get_buffer_sub_data(glow::PIXEL_PACK_BUFFER, 0, &mut rgba);

                        if !slot.broadcast_sent {
                            if let Some(bc) = broadcast {
                                if let Ok(mut b_buf) = bc.free_rx.try_recv() {
                                    if b_buf.len() != bytes {
                                        b_buf.resize(bytes, 0);
                                    }
                                    b_buf.copy_from_slice(&rgba);
                                    if bc
                                        .tx
                                        .try_send(VideoFrame {
                                            rgba: b_buf,
                                            width: capture_w,
                                            height: capture_h,
                                            at: slot.at,
                                            generation: 0,
                                        })
                                        .is_err()
                                    {
                                        bc.dropped.fetch_add(1, Ordering::Relaxed);
                                    }
                                } else {
                                    bc.dropped.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }

                        if let Err(error) = runtime.queue.submit(VideoFrame {
                            rgba,
                            width: capture_w,
                            height: capture_h,
                            at: slot.at,
                            generation,
                        }) {
                            runtime.queue.recycle(generation, error.into_inner().rgba);
                            runtime.shared.video_dropped.fetch_add(1, Ordering::Relaxed);
                        }

                        gl.delete_sync(slot.fence);
                        self.free.push(slot.buffer);
                    } else {
                        // Replay CPU buffer pool is temporarily busy. Retain the
                        // completed GPU slot at the head of `pending` until a CPU
                        // buffer returns, while still delivering each signaled
                        // slot to broadcast at most once.
                        if let Some(bc) = broadcast {
                            for pending_slot in self.pending.iter_mut() {
                                if pending_slot.broadcast_sent {
                                    continue;
                                }
                                let sync_res = gl.client_wait_sync(pending_slot.fence, 0, 0);
                                if sync_res != glow::ALREADY_SIGNALED
                                    && sync_res != glow::CONDITION_SATISFIED
                                {
                                    break;
                                }
                                pending_slot.broadcast_sent = true;
                                if let Ok(mut b_buf) = bc.free_rx.try_recv() {
                                    if b_buf.len() != bytes {
                                        b_buf.resize(bytes, 0);
                                    }
                                    gl.bind_buffer(
                                        glow::PIXEL_PACK_BUFFER,
                                        Some(pending_slot.buffer),
                                    );
                                    gl.get_buffer_sub_data(glow::PIXEL_PACK_BUFFER, 0, &mut b_buf);
                                    if bc
                                        .tx
                                        .try_send(VideoFrame {
                                            rgba: b_buf,
                                            width: capture_w,
                                            height: capture_h,
                                            at: pending_slot.at,
                                            generation: 0,
                                        })
                                        .is_err()
                                    {
                                        bc.dropped.fetch_add(1, Ordering::Relaxed);
                                    }
                                } else {
                                    bc.dropped.fetch_add(1, Ordering::Relaxed);
                                }
                                if copying_started.elapsed() >= std::time::Duration::from_millis(4)
                                {
                                    break;
                                }
                            }
                        }
                        break;
                    }
                } else {
                    let slot = self.pending.pop_front().unwrap();
                    if let Some(bc) = broadcast {
                        if let Ok(mut b_buf) = bc.free_rx.try_recv() {
                            if b_buf.len() != bytes {
                                b_buf.resize(bytes, 0);
                            }
                            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(slot.buffer));
                            gl.get_buffer_sub_data(glow::PIXEL_PACK_BUFFER, 0, &mut b_buf);
                            if bc
                                .tx
                                .try_send(VideoFrame {
                                    rgba: b_buf,
                                    width: capture_w,
                                    height: capture_h,
                                    at: slot.at,
                                    generation: 0,
                                })
                                .is_err()
                            {
                                bc.dropped.fetch_add(1, Ordering::Relaxed);
                            }
                        } else {
                            bc.dropped.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    gl.delete_sync(slot.fence);
                    self.free.push(slot.buffer);
                }

                if copying_started.elapsed() >= std::time::Duration::from_millis(4) {
                    break;
                }
            }

            let stop_flag = running_replay.map_or(false, |r| r.shared.stop.load(Ordering::Relaxed));
            if !stop_flag && self.should_capture(at) {
                self.last_at = at;
                if let Some(runtime) = running_replay {
                    runtime.shared.captured_at.store(at, Ordering::Relaxed);
                }
                let buffer = self.free.pop().or_else(|| {
                    if self.pending.len() < slots {
                        match gl.create_buffer() {
                            Ok(buffer) => {
                                gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(buffer));
                                gl.buffer_data_size(
                                    glow::PIXEL_PACK_BUFFER,
                                    bytes as i32,
                                    glow::STREAM_READ,
                                );
                                let error = gl.get_error();
                                if error == glow::NO_ERROR {
                                    Some(buffer)
                                } else {
                                    gl.delete_buffer(buffer);
                                    if let Some(r) = running_replay {
                                        r.shared.message(format!(
                                            "Replay stopped: cannot allocate GPU work buffer (GL {error:#x})"
                                        ));
                                        r.shared.stop.store(true, Ordering::Release);
                                    }
                                    None
                                }
                            }
                            Err(error) => {
                                if let Some(r) = running_replay {
                                    r.shared.message(format!(
                                        "Replay stopped: cannot create GPU work buffer: {error}"
                                    ));
                                    r.shared.stop.store(true, Ordering::Release);
                                }
                                None
                            }
                        }
                    } else {
                        None
                    }
                });
                if let Some(buffer) = buffer {
                    gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(buffer));
                    let mut read_issued = true;
                    let is_downscaling = capture_w != width || capture_h != height;
                    if is_downscaling {
                        let old_read_fbo =
                            fbo_from_param(gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING));
                        let old_draw_fbo =
                            fbo_from_param(gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING));

                        if self.downscale_size != (capture_w, capture_h)
                            || self.downscale_fbo.is_none()
                        {
                            if let Some(fbo) = self.downscale_fbo.take() {
                                gl.delete_framebuffer(fbo);
                            }
                            if let Some(tex) = self.downscale_tex.take() {
                                gl.delete_texture(tex);
                            }
                            self.downscale_size = (0, 0);

                            let old_tex =
                                tex_from_param(gl.get_parameter_i32(glow::TEXTURE_BINDING_2D));
                            if let Ok(tex) = gl.create_texture() {
                                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                                gl.tex_image_2d_empty(capture_w as i32, capture_h as i32);
                                gl.tex_parameter_i32(
                                    glow::TEXTURE_2D,
                                    glow::TEXTURE_MIN_FILTER,
                                    glow::LINEAR as i32,
                                );
                                gl.tex_parameter_i32(
                                    glow::TEXTURE_2D,
                                    glow::TEXTURE_MAG_FILTER,
                                    glow::LINEAR as i32,
                                );
                                gl.tex_parameter_i32(
                                    glow::TEXTURE_2D,
                                    glow::TEXTURE_WRAP_S,
                                    glow::CLAMP_TO_EDGE as i32,
                                );
                                gl.tex_parameter_i32(
                                    glow::TEXTURE_2D,
                                    glow::TEXTURE_WRAP_T,
                                    glow::CLAMP_TO_EDGE as i32,
                                );

                                if let Ok(fbo) = gl.create_framebuffer() {
                                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                                    gl.framebuffer_texture_2d(
                                        glow::FRAMEBUFFER,
                                        glow::COLOR_ATTACHMENT0,
                                        glow::TEXTURE_2D,
                                        Some(tex),
                                        0,
                                    );
                                    let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
                                    gl.bind_framebuffer(glow::READ_FRAMEBUFFER, old_read_fbo);
                                    gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, old_draw_fbo);

                                    if status == glow::FRAMEBUFFER_COMPLETE {
                                        self.downscale_fbo = Some(fbo);
                                        self.downscale_tex = Some(tex);
                                        self.downscale_size = (capture_w, capture_h);
                                    } else {
                                        gl.delete_framebuffer(fbo);
                                        gl.delete_texture(tex);
                                    }
                                } else {
                                    gl.delete_texture(tex);
                                }
                            }
                            gl.bind_texture(glow::TEXTURE_2D, old_tex);
                            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, old_read_fbo);
                            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, old_draw_fbo);
                        }

                        if let Some(target_fbo) = self.downscale_fbo {
                            let scissor_enabled = gl.is_enabled(glow::SCISSOR_TEST);
                            if scissor_enabled {
                                gl.disable(glow::SCISSOR_TEST);
                            }

                            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, old_draw_fbo);
                            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(target_fbo));
                            gl.blit_framebuffer(
                                x as i32,
                                y as i32,
                                (x + width) as i32,
                                (y + height) as i32,
                                0,
                                0,
                                capture_w as i32,
                                capture_h as i32,
                                glow::COLOR_BUFFER_BIT,
                                glow::LINEAR,
                            );

                            if scissor_enabled {
                                gl.enable(glow::SCISSOR_TEST);
                            }

                            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(target_fbo));
                            gl.read_pixels_to_pbo(0, 0, capture_w as i32, capture_h as i32);

                            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, old_read_fbo);
                            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, old_draw_fbo);
                        } else {
                            read_issued = false;
                            self.free.push(buffer);
                            if let Some(r) = running_replay {
                                r.shared.video_dropped.fetch_add(1, Ordering::Relaxed);
                            }
                            if let Some(bc) = broadcast {
                                bc.dropped.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    } else {
                        gl.read_pixels_to_pbo(x as i32, y as i32, width as i32, height as i32);
                    }
                    if read_issued {
                        match gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) {
                            Ok(fence) => {
                                self.pending.push_back(Slot {
                                    buffer,
                                    fence,
                                    at,
                                    generation,
                                    broadcast_sent: false,
                                });
                                self.last_at = at;
                            }
                            Err(e) => {
                                gl.delete_buffer(buffer);
                                if let Some(r) = running_replay {
                                    r.shared.message(format!("Replay stopped: {e}"));
                                    r.shared.stop.store(true, Ordering::Release);
                                }
                            }
                        }
                    }
                } else {
                    if let Some(r) = running_replay {
                        r.shared.video_dropped.fetch_add(1, Ordering::Relaxed);
                    }
                    if let Some(bc) = broadcast {
                        bc.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            if let Some(r) = running_replay {
                r.shared
                    .gpu_pending
                    .store(self.pending.len(), Ordering::Relaxed);
            }
            gl.bind_buffer(
                glow::PIXEL_PACK_BUFFER,
                std::num::NonZeroU32::new(old as u32).map(glow::NativeBuffer),
            );
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, alignment);
            gl.pixel_store_i32(glow::PACK_ROW_LENGTH, row_length);
            gl.pixel_store_i32(glow::PACK_SKIP_ROWS, skip_rows);
            gl.pixel_store_i32(glow::PACK_SKIP_PIXELS, skip_pixels);
        }
    }
}
#[derive(Clone)]
pub struct RuntimeView {
    queue: std::sync::Arc<super::queue::WorkQueue>,
    shared: std::sync::Arc<super::Shared>,
    config: std::sync::Arc<super::config::ReplayConfig>,
}
impl RuntimeView {
    pub fn new(runtime: &Runtime) -> Self {
        Self {
            queue: runtime.queue.clone(),
            shared: runtime.shared.clone(),
            config: runtime.config.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started() -> Readback {
        let mut readback = Readback::default();
        assert_eq!(readback.observe_surface((1920, 1080), 10_000_000, 0), None);
        assert_eq!(
            readback.observe_surface((1920, 1080), 10_300_000, 300_000),
            Some(true)
        );
        readback.last_at = 10_300_000;
        readback
    }

    #[test]
    fn late_frames_and_missing_timestamps_do_not_invalidate_recording() {
        let mut readback = started();
        // A late frame used to trip the 250 ms clock-reset heuristic before
        // the existing duplicate/out-of-order timestamp check could reject it.
        for at in [10_316_667, 9_000_000, 10_300_000, 0, 10_350_000] {
            assert_eq!(
                readback.observe_surface((1920, 1080), at, 400_000),
                if at > 0 { Some(false) } else { None }
            );
        }
        assert_eq!(readback.last_at, 10_300_000);
    }

    #[test]
    fn minimized_and_transient_sizes_preserve_committed_surface() {
        let mut readback = started();
        for size in [(0, 0), (1920, 0), (0, 1080), (1919, 1080)] {
            assert_eq!(readback.observe_surface(size, 11_000_000, 1_000_000), None);
            assert_eq!(readback.surface.size, (1920, 1080));
            assert_eq!(
                readback.observe_surface((1920, 1080), 11_010_000, 1_010_000),
                Some(false)
            );
        }
        assert_eq!(
            readback.observe_surface((0, 0), 12_000_000, 2_000_000),
            None
        );
        assert_eq!(
            readback.observe_surface((1920, 1080), 20_000_000, 10_000_000),
            Some(false)
        );
    }

    #[test]
    fn actual_resize_commits_once_after_stable_dimensions() {
        let mut readback = started();
        assert_eq!(
            readback.observe_surface((1280, 720), 11_000_000, 1_000_000),
            None
        );
        assert_eq!(
            readback.observe_surface((1281, 720), 11_200_000, 1_200_000),
            None
        );
        assert_eq!(
            readback.observe_surface((1280, 720), 11_400_000, 1_400_000),
            None
        );
        assert_eq!(
            readback.observe_surface((1280, 720), 11_699_999, 1_699_999),
            None
        );
        assert_eq!(readback.surface.size, (1920, 1080));
        assert_eq!(
            readback.observe_surface((1280, 720), 11_700_000, 1_700_000),
            Some(true)
        );
        assert_eq!(
            readback.observe_surface((1280, 720), 11_720_000, 1_720_000),
            Some(false)
        );
        assert_eq!(readback.surface.size, (1280, 720));
    }

    #[test]
    fn ten_minutes_of_stalls_and_transient_inputs_retain_a_full_history() {
        use crate::replay::{
            config::{QueuePlan, Rate},
            queue::{PoolRequest, WorkQueue},
            ring::{Encoded, History},
            worker::VideoSchedule,
        };
        use std::sync::atomic::{AtomicBool, AtomicU64};
        use std::time::Duration;

        let mut readback = started();
        let epoch = 11_000_000;
        let rate = Rate::new(60, 1);
        let generation = AtomicU64::new(1);
        let queue = WorkQueue::new();
        let request = PoolRequest {
            generation: 1,
            // Tiny stand-in image storage; exercise the real FIFO and history
            // without a GL context, VAAPI encoder, display or capture device.
            plan: QueuePlan {
                slots: 4,
                frame_bytes: 16,
                queue_bytes: 128,
                overhead: 128,
            },
        };
        assert!(queue
            .prepare(request, &generation, &AtomicBool::new(false))
            .unwrap());
        let mut history = History::new(300);
        let mut schedule = VideoSchedule::new(epoch);
        let mut dropped = 0;
        for tick in 0..36_000 {
            let now = epoch + rate.us(tick);
            let size = match tick % 600 {
                1 => (0, 0),
                2 => (1919, 1080),
                _ => (1920, 1080),
            };
            let at = if tick % 600 == 3 {
                now - 1_000_000
            } else {
                now
            };
            let observed = readback.observe_surface(size, at, now);
            assert_ne!(
                observed,
                Some(true),
                "Transient input reset recording at tick {tick}"
            );
            if observed == Some(false) && at > readback.last_at {
                readback.last_at = at;
                if let Some(rgba) = queue.buffer(1) {
                    queue
                        .submit(VideoFrame {
                            rgba,
                            width: 2,
                            height: 2,
                            at,
                            generation: 1,
                        })
                        .unwrap();
                } else {
                    dropped += 1;
                }
            }
            // A one-second encoder stall every two seconds exceeds this tiny
            // pool. It must drop only new arrivals, never already saved history.
            if tick % 120 < 60 {
                continue;
            }
            while let Some(frame) = queue.receive(Duration::ZERO) {
                let output = schedule.accept(frame.at).unwrap();
                for (pts, key) in output.all_frames() {
                    for video in [true, false] {
                        let mut packet = ffmpeg_next::Packet::copy(&[0; 16]);
                        packet.set_pts(Some(pts));
                        packet.set_dts(Some(pts));
                        packet.set_duration(rate.us(1));
                        if video && key {
                            packet.set_flags(ffmpeg_next::packet::Flags::KEY);
                        }
                        history.push(Encoded::new(packet, video), 64 * 1024 * 1024);
                    }
                }
                queue.recycle(1, frame.rgba);
            }
        }
        assert!(
            dropped > 10_000,
            "The fixture did not stress queue saturation"
        );
        assert!(history.duration() > 297. && history.duration() <= 300.);
        let clip = history.clip(300, epoch + 600_000_000).unwrap();
        assert!(clip[0].key);
        assert!(clip.last().unwrap().end - clip[0].start > 297_000_000);
    }

    #[test]
    fn target_capture_dimensions_scales_down_4k_and_preserves_1080p() {
        assert_eq!(target_capture_dimensions(0, 0), (0, 0));
        assert_eq!(target_capture_dimensions(1920, 1080), (1920, 1080));
        assert_eq!(target_capture_dimensions(1280, 720), (1280, 720));
        assert_eq!(target_capture_dimensions(3840, 2160), (1920, 1080));
        assert_eq!(target_capture_dimensions(2560, 1440), (1920, 1080));
        let (uw_w, uw_h) = target_capture_dimensions(3440, 1440);
        assert_eq!(uw_w, 1920);
        assert_eq!(uw_h % 2, 0);
        assert!(uw_h <= 1080);
    }

    #[test]
    fn should_capture_throttles_120fps_input_to_clean_60fps() {
        let mut readback = Readback::default();
        // Frame interval at 120 FPS is ~8,333.3 us
        let frame_interval_120 = 8_333i64;
        let mut t = 10_000_000i64;

        // First frame (initial state, last_at is 0) should be captured
        assert!(readback.should_capture(t));
        readback.last_at = t;

        // Frame 1 (+8.33ms) must be dropped
        t += frame_interval_120;
        assert!(!readback.should_capture(t));

        // Frame 2 (+16.67ms total) must be captured
        t += frame_interval_120;
        assert!(readback.should_capture(t));
        readback.last_at = t;

        // Frame 3 (+8.33ms) must be dropped
        t += frame_interval_120;
        assert!(!readback.should_capture(t));

        // Frame 4 (+16.67ms total) must be captured
        t += frame_interval_120;
        assert!(readback.should_capture(t));
        readback.last_at = t;

        // Out-of-order or duplicate timestamp must be rejected
        assert!(!readback.should_capture(t));
        assert!(!readback.should_capture(t - 1_000));
    }

    #[test]
    fn should_capture_maintains_target_rate_across_common_refresh_rates_and_stalls() {
        for (hz, min_expected, max_expected) in [
            (59.94_f64, 598usize, 601usize),
            (60.0, 599, 601),
            (65.0, 599, 601),
            (75.0, 599, 601),
            (90.0, 599, 601),
            (120.0, 599, 601),
            (144.0, 599, 601),
        ] {
            let readback = Readback::default();
            let total_frames = (10.0 * hz).round() as usize;
            let epoch = 10_000_000i64;
            let mut accepted = 0usize;
            for i in 0..total_frames {
                let at = epoch + ((i as f64) * 1_000_000.0 / hz).round() as i64;
                if readback.should_capture(at) {
                    accepted += 1;
                }
            }
            assert!(
                (min_expected..=max_expected).contains(&accepted),
                "At {hz} Hz over 10s, expected {min_expected}..={max_expected} captures, got {accepted}"
            );
        }

        // Jitter around 60 Hz (+/- 2.5 ms) must still accept every frame without drops.
        let jittered = Readback::default();
        let epoch = 20_000_000i64;
        let mut accepted_jitter = 0usize;
        for i in 0..600 {
            let offset = if i == 0 {
                0
            } else if i % 2 == 0 {
                2_500
            } else {
                -2_500
            };
            let at = epoch + (i as i64 * 1_000_000) / 60 + offset;
            if jittered.should_capture(at) {
                accepted_jitter += 1;
            }
        }
        assert_eq!(accepted_jitter, 600);

        // Long stall (> 100 ms) must re-anchor cleanly without a catch-up burst
        // that accepts back-to-back 144 Hz frames.
        let stalled = Readback::default();
        assert!(stalled.should_capture(30_000_000));
        assert!(stalled.should_capture(30_500_000)); // 500ms stall -> accepted immediately
        assert!(!stalled.should_capture(30_506_944)); // +6.94ms (144 Hz next frame) -> throttled
        assert!(stalled.should_capture(30_513_889)); // +13.89ms -> accepted
    }

    struct MockGl {
        read_fbo: Cell<u32>,
        draw_fbo: Cell<u32>,
        tex_2d: Cell<u32>,
        pack_buffer: Cell<u32>,
        pack_alignment: Cell<i32>,
        pack_row_length: Cell<i32>,
        pack_skip_rows: Cell<i32>,
        pack_skip_pixels: Cell<i32>,
        scissor_enabled: Cell<bool>,
        next_id: Cell<u32>,
        next_fence_id: Cell<u64>,
        fbo_status: Cell<u32>,
        fail_create_fbo: Cell<bool>,
        fail_create_tex: Cell<bool>,
        next_pixel_byte: Cell<u8>,
        last_blit_src_fbo: Cell<u32>,
        last_read_pixels_fbo: Cell<u32>,
        buffers: std::cell::RefCell<std::collections::HashMap<u32, Vec<u8>>>,
        textures: std::cell::RefCell<std::collections::HashSet<u32>>,
        fbos: std::cell::RefCell<std::collections::HashSet<u32>>,
        fences: std::cell::RefCell<std::collections::HashMap<u64, u32>>,
        default_fence_state: Cell<u32>,
    }

    impl Default for MockGl {
        fn default() -> Self {
            Self {
                read_fbo: Cell::new(0),
                draw_fbo: Cell::new(0),
                tex_2d: Cell::new(0),
                pack_buffer: Cell::new(0),
                pack_alignment: Cell::new(4),
                pack_row_length: Cell::new(0),
                pack_skip_rows: Cell::new(0),
                pack_skip_pixels: Cell::new(0),
                scissor_enabled: Cell::new(false),
                next_id: Cell::new(100),
                next_fence_id: Cell::new(1),
                fbo_status: Cell::new(glow::FRAMEBUFFER_COMPLETE),
                fail_create_fbo: Cell::new(false),
                fail_create_tex: Cell::new(false),
                next_pixel_byte: Cell::new(0x11),
                last_blit_src_fbo: Cell::new(0),
                last_read_pixels_fbo: Cell::new(0),
                buffers: Default::default(),
                textures: Default::default(),
                fbos: Default::default(),
                fences: Default::default(),
                default_fence_state: Cell::new(glow::ALREADY_SIGNALED),
            }
        }
    }

    impl MockGl {
        fn alloc_id(&self) -> std::num::NonZeroU32 {
            let id = self.next_id.get();
            self.next_id.set(id + 1);
            std::num::NonZeroU32::new(id).unwrap()
        }
    }

    unsafe impl GlBackend for MockGl {
        unsafe fn delete_sync(&self, fence: SlotFence) {
            if let SlotFence::Mock(id) = fence {
                self.fences.borrow_mut().remove(&id);
            }
        }
        unsafe fn delete_buffer(&self, buffer: glow::Buffer) {
            let id = buffer.0.get();
            self.buffers.borrow_mut().remove(&id);
            if self.pack_buffer.get() == id {
                self.pack_buffer.set(0);
            }
        }
        unsafe fn delete_framebuffer(&self, fbo: glow::Framebuffer) {
            let id = fbo.0.get();
            self.fbos.borrow_mut().remove(&id);
            if self.read_fbo.get() == id {
                self.read_fbo.set(0);
            }
            if self.draw_fbo.get() == id {
                self.draw_fbo.set(0);
            }
        }
        unsafe fn delete_texture(&self, tex: glow::Texture) {
            let id = tex.0.get();
            self.textures.borrow_mut().remove(&id);
            if self.tex_2d.get() == id {
                self.tex_2d.set(0);
            }
        }
        unsafe fn get_parameter_i32(&self, parameter: u32) -> i32 {
            match parameter {
                glow::READ_FRAMEBUFFER_BINDING => self.read_fbo.get() as i32,
                glow::DRAW_FRAMEBUFFER_BINDING => self.draw_fbo.get() as i32,
                glow::TEXTURE_BINDING_2D => self.tex_2d.get() as i32,
                glow::PIXEL_PACK_BUFFER_BINDING => self.pack_buffer.get() as i32,
                glow::PACK_ALIGNMENT => self.pack_alignment.get(),
                glow::PACK_ROW_LENGTH => self.pack_row_length.get(),
                glow::PACK_SKIP_ROWS => self.pack_skip_rows.get(),
                glow::PACK_SKIP_PIXELS => self.pack_skip_pixels.get(),
                _ => 0,
            }
        }
        unsafe fn pixel_store_i32(&self, parameter: u32, value: i32) {
            match parameter {
                glow::PACK_ALIGNMENT => self.pack_alignment.set(value),
                glow::PACK_ROW_LENGTH => self.pack_row_length.set(value),
                glow::PACK_SKIP_ROWS => self.pack_skip_rows.set(value),
                glow::PACK_SKIP_PIXELS => self.pack_skip_pixels.set(value),
                _ => {}
            }
        }
        unsafe fn client_wait_sync(&self, fence: SlotFence, _flags: u32, _timeout: i32) -> u32 {
            match fence {
                SlotFence::Mock(id) => self
                    .fences
                    .borrow()
                    .get(&id)
                    .copied()
                    .unwrap_or(glow::WAIT_FAILED),
                SlotFence::Gl(_) => glow::WAIT_FAILED,
            }
        }
        unsafe fn bind_buffer(&self, target: u32, buffer: Option<glow::Buffer>) {
            if target == glow::PIXEL_PACK_BUFFER {
                self.pack_buffer.set(buffer.map_or(0, |b| b.0.get()));
            }
        }
        unsafe fn get_buffer_sub_data(&self, target: u32, offset: i32, dst: &mut [u8]) {
            if target == glow::PIXEL_PACK_BUFFER {
                let bound = self.pack_buffer.get();
                let buffers = self.buffers.borrow();
                if let Some(src) = buffers.get(&bound) {
                    let start = offset as usize;
                    dst.copy_from_slice(&src[start..start + dst.len()]);
                }
            }
        }
        unsafe fn create_buffer(&self) -> Result<glow::Buffer, String> {
            let id = self.alloc_id();
            self.buffers.borrow_mut().insert(id.get(), Vec::new());
            Ok(glow::NativeBuffer(id))
        }
        unsafe fn buffer_data_size(&self, target: u32, size: i32, _usage: u32) {
            if target == glow::PIXEL_PACK_BUFFER {
                let bound = self.pack_buffer.get();
                if let Some(buf) = self.buffers.borrow_mut().get_mut(&bound) {
                    buf.resize(size.max(0) as usize, 0);
                }
            }
        }
        unsafe fn get_error(&self) -> u32 {
            glow::NO_ERROR
        }
        unsafe fn create_texture(&self) -> Result<glow::Texture, String> {
            if self.fail_create_tex.get() {
                return Err("mock texture alloc failed".into());
            }
            let id = self.alloc_id();
            self.textures.borrow_mut().insert(id.get());
            Ok(glow::NativeTexture(id))
        }
        unsafe fn bind_texture(&self, target: u32, texture: Option<glow::Texture>) {
            if target == glow::TEXTURE_2D {
                self.tex_2d.set(texture.map_or(0, |t| t.0.get()));
            }
        }
        unsafe fn tex_image_2d_empty(&self, _width: i32, _height: i32) {}
        unsafe fn tex_parameter_i32(&self, _target: u32, _parameter: u32, _value: i32) {}
        unsafe fn create_framebuffer(&self) -> Result<glow::Framebuffer, String> {
            if self.fail_create_fbo.get() {
                return Err("mock fbo alloc failed".into());
            }
            let id = self.alloc_id();
            self.fbos.borrow_mut().insert(id.get());
            Ok(glow::NativeFramebuffer(id))
        }
        unsafe fn bind_framebuffer(&self, target: u32, framebuffer: Option<glow::Framebuffer>) {
            let id = framebuffer.map_or(0, |f| f.0.get());
            match target {
                glow::FRAMEBUFFER => {
                    self.read_fbo.set(id);
                    self.draw_fbo.set(id);
                }
                glow::READ_FRAMEBUFFER => self.read_fbo.set(id),
                glow::DRAW_FRAMEBUFFER => self.draw_fbo.set(id),
                _ => {}
            }
        }
        unsafe fn framebuffer_texture_2d(
            &self,
            _target: u32,
            _attachment: u32,
            _textarget: u32,
            _texture: Option<glow::Texture>,
            _level: i32,
        ) {
        }
        unsafe fn check_framebuffer_status(&self, _target: u32) -> u32 {
            self.fbo_status.get()
        }
        unsafe fn is_enabled(&self, parameter: u32) -> bool {
            parameter == glow::SCISSOR_TEST && self.scissor_enabled.get()
        }
        unsafe fn disable(&self, parameter: u32) {
            if parameter == glow::SCISSOR_TEST {
                self.scissor_enabled.set(false);
            }
        }
        unsafe fn enable(&self, parameter: u32) {
            if parameter == glow::SCISSOR_TEST {
                self.scissor_enabled.set(true);
            }
        }
        unsafe fn blit_framebuffer(
            &self,
            _src_x0: i32,
            _src_y0: i32,
            _src_x1: i32,
            _src_y1: i32,
            _dst_x0: i32,
            _dst_y0: i32,
            _dst_x1: i32,
            _dst_y1: i32,
            _mask: u32,
            _filter: u32,
        ) {
            self.last_blit_src_fbo.set(self.read_fbo.get());
        }
        unsafe fn read_pixels_to_pbo(&self, _x: i32, _y: i32, _width: i32, _height: i32) {
            self.last_read_pixels_fbo.set(self.read_fbo.get());
            let bound = self.pack_buffer.get();
            let fill = self.next_pixel_byte.get();
            if let Some(buf) = self.buffers.borrow_mut().get_mut(&bound) {
                buf.fill(fill);
            }
        }
        unsafe fn fence_sync(&self, _condition: u32, _flags: u32) -> Result<SlotFence, String> {
            let id = self.next_fence_id.get();
            self.next_fence_id.set(id + 1);
            self.fences
                .borrow_mut()
                .insert(id, self.default_fence_state.get());
            Ok(SlotFence::Mock(id))
        }
    }

    #[test]
    fn downscale_fbo_init_resize_and_failures_preserve_read_and_draw_fbo_bindings() {
        let gl = MockGl::default();
        gl.read_fbo.set(11);
        gl.draw_fbo.set(22);
        gl.tex_2d.set(33);
        gl.pack_buffer.set(44);
        gl.pack_alignment.set(8);
        gl.pack_row_length.set(64);
        gl.pack_skip_rows.set(2);
        gl.pack_skip_pixels.set(4);
        gl.scissor_enabled.set(true);

        let (bc_tx, _bc_rx) = crossbeam_channel::bounded(4);
        let (free_tx, free_rx) = crossbeam_channel::bounded(4);
        for _ in 0..4 {
            let _ = free_tx.send(Vec::new());
        }
        let bc = BroadcastSink {
            tx: bc_tx,
            free_rx,
            dropped: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };

        let mut readback = Readback::default();
        // Stabilize 4K surface (downscales 3840x2160 -> 1920x1080)
        readback.surface.size = (1920, 1080);

        let area_4k = RenderedArea {
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
        };
        readback.capture_with(&gl, None, Some(&bc), area_4k, 10_000_000);

        assert!(readback.downscale_fbo.is_some());
        assert_eq!(readback.downscale_size, (1920, 1080));
        assert_eq!(gl.last_blit_src_fbo.get(), 22);
        assert_eq!(
            gl.last_read_pixels_fbo.get(),
            readback.downscale_fbo.unwrap().0.get()
        );
        // Sentinel bindings and state must be restored after first downscale FBO init + blit
        assert_eq!(gl.read_fbo.get(), 11);
        assert_eq!(gl.draw_fbo.get(), 22);
        assert_eq!(gl.tex_2d.get(), 33);
        assert_eq!(gl.pack_buffer.get(), 44);
        assert_eq!(gl.pack_alignment.get(), 8);
        assert_eq!(gl.pack_row_length.get(), 64);
        assert_eq!(gl.pack_skip_rows.get(), 2);
        assert_eq!(gl.pack_skip_pixels.get(), 4);
        assert!(gl.scissor_enabled.get());

        // Resize to ultrawide 3440x1440 (downscales to 1920x804)
        let first_fbo_id = readback.downscale_fbo.unwrap().0.get();
        let first_tex_id = readback.downscale_tex.unwrap().0.get();
        readback.surface.size = (1920, 804);
        let area_uw = RenderedArea {
            x: 0,
            y: 0,
            width: 3440,
            height: 1440,
        };
        readback.capture_with(&gl, None, Some(&bc), area_uw, 10_020_000);
        assert_eq!(readback.downscale_size, (1920, 804));
        assert!(!gl.fbos.borrow().contains(&first_fbo_id));
        assert!(!gl.textures.borrow().contains(&first_tex_id));
        assert_eq!(gl.read_fbo.get(), 11);
        assert_eq!(gl.draw_fbo.get(), 22);
        assert_eq!(gl.tex_2d.get(), 33);
        assert_eq!(gl.pack_buffer.get(), 44);

        // Incomplete FBO path must also restore both READ and DRAW FBO bindings and clean up
        readback.downscale_size = (0, 0);
        gl.fbo_status.set(glow::FRAMEBUFFER_INCOMPLETE_ATTACHMENT);
        let dropped_before = bc.dropped.load(Ordering::Relaxed);
        readback.capture_with(&gl, None, Some(&bc), area_uw, 10_040_000);
        assert!(readback.downscale_fbo.is_none());
        assert!(readback.downscale_tex.is_none());
        assert!(gl.fbos.borrow().is_empty());
        assert!(gl.textures.borrow().is_empty());
        assert_eq!(bc.dropped.load(Ordering::Relaxed), dropped_before + 1);
        assert_eq!(gl.read_fbo.get(), 11);
        assert_eq!(gl.draw_fbo.get(), 22);
        assert_eq!(gl.tex_2d.get(), 33);
        assert_eq!(gl.pack_buffer.get(), 44);

        // FBO creation failure path must also restore bindings and delete the texture
        gl.fail_create_fbo.set(true);
        readback.capture_with(&gl, None, Some(&bc), area_uw, 10_060_000);
        assert!(readback.downscale_fbo.is_none());
        assert!(gl.textures.borrow().is_empty());
        assert_eq!(gl.read_fbo.get(), 11);
        assert_eq!(gl.draw_fbo.get(), 22);
        assert_eq!(gl.tex_2d.get(), 33);
        assert_eq!(gl.pack_buffer.get(), 44);
    }

    #[test]
    fn completed_gpu_frames_are_retained_when_cpu_pool_is_busy_and_not_resent_to_broadcast() {
        use crate::replay::{
            clipboard::Clipboard,
            config::{QueuePlan, ReplayConfig},
            queue::{PoolRequest, WorkQueue},
            Shared, Status,
        };
        use std::sync::{
            atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize},
            Arc, Mutex,
        };
        use std::time::Duration;

        let gl = MockGl::default();
        // Start fences unsignaled so we control when GPU transfers complete
        gl.default_fence_state.set(glow::TIMEOUT_EXPIRED);

        let queue = Arc::new(WorkQueue::new());
        let config = Arc::new(ReplayConfig::default());
        let shared = Arc::new(Shared {
            clipboard: Arc::new(Clipboard::default()),
            generation: AtomicU64::new(0),
            stop: Arc::new(AtomicBool::new(false)),
            audio_dropped: Arc::new(AtomicU64::new(0)),
            video_dropped: AtomicU64::new(0),
            gpu_pending: AtomicUsize::new(0),
            queue_slots: AtomicUsize::new(0),
            queue_bytes: AtomicUsize::new(0),
            staging_bytes: AtomicUsize::new(0),
            captured_at: Default::default(),
            processed_at: Default::default(),
            status: Mutex::new(Status::default()),
            saving_bytes: AtomicUsize::new(0),
            game_volume: Arc::new(AtomicU32::new(1.0f32.to_bits())),
        });
        let runtime = RuntimeView {
            queue: queue.clone(),
            shared: shared.clone(),
            config,
        };

        let area = RenderedArea {
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        };
        let frame_bytes = 4 * 2 * 4;
        let plan = QueuePlan {
            slots: 2,
            frame_bytes,
            queue_bytes: frame_bytes * 4,
            overhead: frame_bytes * 4,
        };
        assert!(queue
            .prepare(
                PoolRequest {
                    generation: 0,
                    plan,
                },
                &shared.generation,
                &shared.stop
            )
            .unwrap());

        // Exhaust all CPU buffers in the pool to simulate a busy encoder/CPU stall
        let held_cpu_0 = queue.buffer(0).unwrap();
        let held_cpu_1 = queue.buffer(0).unwrap();
        assert!(queue.buffer(0).is_none());

        let (bc_tx, bc_rx) = crossbeam_channel::bounded(8);
        let (bc_free_tx, bc_free_rx) = crossbeam_channel::bounded(8);
        for _ in 0..4 {
            bc_free_tx.send(vec![0u8; frame_bytes]).unwrap();
        }
        let bc = BroadcastSink {
            tx: bc_tx,
            free_rx: bc_free_rx,
            dropped: Arc::new(AtomicU64::new(0)),
        };

        let mut readback = Readback::default();
        readback.active = Some(Arc::downgrade(&shared));
        readback.surface.size = (4, 2);

        // Capture frame 1 (0xA1) and frame 2 (0xB2) into GPU PBOs
        gl.next_pixel_byte.set(0xA1);
        readback.capture_with(&gl, Some(&runtime), Some(&bc), area, 10_000_000);
        gl.next_pixel_byte.set(0xB2);
        readback.capture_with(&gl, Some(&runtime), Some(&bc), area, 10_020_000);
        assert_eq!(readback.pending.len(), 2);

        // Signal both GPU fences while the CPU pool is still completely full
        for status in gl.fences.borrow_mut().values_mut() {
            *status = glow::ALREADY_SIGNALED;
        }

        // Poll drain loop at t = 10_025_000 (not a new capture tick, so only drain runs):
        // Broadcast should receive both frames once, while replay retains both in `pending`
        // without dropping them!
        readback.capture_with(&gl, Some(&runtime), Some(&bc), area, 10_025_000);
        assert_eq!(
            shared.video_dropped.load(Ordering::Relaxed),
            0,
            "Signaled GPU slots must not be dropped when CPU pool is temporarily busy"
        );
        assert_eq!(readback.pending.len(), 2);
        let bc_f1 = bc_rx.try_recv().expect("Broadcast should receive frame 1");
        let bc_f2 = bc_rx.try_recv().expect("Broadcast should receive frame 2");
        assert_eq!(bc_f1.rgba[0], 0xA1);
        assert_eq!(bc_f2.rgba[0], 0xB2);
        assert!(bc_rx.try_recv().is_err());

        // Another drain pass while still blocked must NOT resend frames to broadcast
        readback.capture_with(&gl, Some(&runtime), Some(&bc), area, 10_030_000);
        assert!(bc_rx.try_recv().is_err());
        assert_eq!(shared.video_dropped.load(Ordering::Relaxed), 0);
        assert_eq!(readback.pending.len(), 2);

        // Now release CPU buffers and verify the retained GPU slots drain in FIFO order
        queue.recycle(0, held_cpu_0);
        queue.recycle(0, held_cpu_1);
        readback.capture_with(&gl, Some(&runtime), Some(&bc), area, 10_032_000);
        assert_eq!(readback.pending.len(), 0);
        assert_eq!(shared.video_dropped.load(Ordering::Relaxed), 0);
        assert!(
            bc_rx.try_recv().is_err(),
            "Retained replay slots must not be resent to broadcast when drained"
        );

        let replay_f1 = queue
            .receive(Duration::ZERO)
            .expect("Replay should receive retained frame 1");
        let replay_f2 = queue
            .receive(Duration::ZERO)
            .expect("Replay should receive retained frame 2");
        assert_eq!(replay_f1.at, 10_000_000);
        assert_eq!(replay_f1.rgba[0], 0xA1);
        assert_eq!(replay_f2.at, 10_020_000);
        assert_eq!(replay_f2.rgba[0], 0xB2);
    }
}
