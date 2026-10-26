//! Read back the already-shaded game rectangle, before egui overlays. No second shader render.
//! Budgeted fenced PBO queue; retain unfinished/blocked transfers without waiting.
use super::{Runtime, VideoFrame};
use crate::video::gpu::geometry::RenderedArea;
use eframe::glow::{self, HasContext};
use std::{collections::VecDeque, sync::atomic::Ordering};

#[derive(Clone)]
pub struct BroadcastSink {
    pub tx: crossbeam_channel::Sender<VideoFrame>,
    pub free_rx: crossbeam_channel::Receiver<Vec<u8>>,
    pub dropped: std::sync::Arc<std::sync::atomic::AtomicU64>,
}
struct Slot {
    buffer: glow::Buffer,
    fence: glow::Fence,
    at: i64,
}
// SAFETY: fence handles are opaque. Slots are only accessed by egui paint/on_exit
// with the owning GL context current; no GL operation happens in Drop or a worker.
unsafe impl Send for Slot {}
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

#[derive(Default)]
pub struct Readback {
    pending: VecDeque<Slot>,
    free: Vec<glow::Buffer>,
    surface: SurfaceState,
    last_at: i64,
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
    fn release_buffers(&mut self, gl: &glow::Context) {
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
        self.requested = None;
    }
    pub fn destroy(&mut self, gl: &glow::Context) {
        self.release_buffers(gl);
        self.surface = SurfaceState::default();
        self.active = None;
    }

    /// Rate-limit capture requests to <= 60 FPS (approx 16.6ms intervals)
    /// to allow Gamescope to render at high refresh (120 FPS) while recording/streaming
    /// at a clean 60 FPS without wasting GPU downscaling or PBO readback transfers.
    pub fn should_capture(&self, at: i64) -> bool {
        const MIN_CAPTURE_INTERVAL_US: i64 = 15_000;
        at > self.last_at && (self.last_at <= 0 || (at - self.last_at) >= MIN_CAPTURE_INTERVAL_US)
    }

    pub fn capture(
        &mut self,
        gl: &glow::Context,
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
            self.destroy(gl);
            return;
        }
        if let Some(runtime) = running_replay {
            if self
                .active
                .as_ref()
                .and_then(|w| w.upgrade())
                .is_none_or(|old| !std::sync::Arc::ptr_eq(&old, &runtime.shared))
            {
                self.destroy(gl);
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
            self.release_buffers(gl);
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
            for _ in 0..2 {
                let Some(slot) = self.pending.front() else {
                    break;
                };
                let result = gl.client_wait_sync(slot.fence, 0, 0);
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
                let slot = self.pending.pop_front().unwrap();

                if let Some(runtime) = running_replay {
                    if let Some(mut rgba) = runtime.queue.buffer(generation) {
                        debug_assert_eq!(rgba.len(), bytes);
                        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(slot.buffer));
                        gl.get_buffer_sub_data(glow::PIXEL_PACK_BUFFER, 0, &mut rgba);

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
                    } else {
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
                        runtime.shared.video_dropped.fetch_add(1, Ordering::Relaxed);
                    }
                } else if let Some(bc) = broadcast {
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
                if copying_started.elapsed() >= std::time::Duration::from_millis(4) {
                    break;
                }
            }

            let stop_flag = running_replay.map_or(false, |r| r.shared.stop.load(Ordering::Relaxed));
            if self.should_capture(at) && !stop_flag {
                self.last_at = at;
                if let Some(runtime) = running_replay {
                    runtime.shared.captured_at.store(at, Ordering::Relaxed);
                }
                let buffer = self.free.pop().or_else(|| {
                    if self.pending.len() < slots {
                        match gl.create_buffer() {
                            Ok(buffer) => {
                                gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(buffer));
                                gl.buffer_data_size(glow::PIXEL_PACK_BUFFER, bytes as i32, glow::STREAM_READ);
                                let error = gl.get_error();
                                if error == glow::NO_ERROR { Some(buffer) } else {
                                    gl.delete_buffer(buffer);
                                    if let Some(r) = running_replay {
                                        r.shared.message(format!("Replay stopped: cannot allocate GPU work buffer (GL {error:#x})"));
                                        r.shared.stop.store(true, Ordering::Release);
                                    }
                                    None
                                }
                            }
                            Err(error) => {
                                if let Some(r) = running_replay {
                                    r.shared.message(format!("Replay stopped: cannot create GPU work buffer: {error}"));
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
                                gl.tex_image_2d(
                                    glow::TEXTURE_2D,
                                    0,
                                    glow::RGBA8 as i32,
                                    capture_w as i32,
                                    capture_h as i32,
                                    0,
                                    glow::RGBA,
                                    glow::UNSIGNED_BYTE,
                                    None,
                                );
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
                                    let old_draw = fbo_from_param(
                                        gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING),
                                    );
                                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                                    gl.framebuffer_texture_2d(
                                        glow::FRAMEBUFFER,
                                        glow::COLOR_ATTACHMENT0,
                                        glow::TEXTURE_2D,
                                        Some(tex),
                                        0,
                                    );
                                    let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
                                    gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, old_draw);

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
                        }

                        if let Some(target_fbo) = self.downscale_fbo {
                            let old_read_fbo = fbo_from_param(
                                gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING),
                            );
                            let old_draw_fbo = fbo_from_param(
                                gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING),
                            );
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
                            gl.read_pixels(
                                0,
                                0,
                                capture_w as i32,
                                capture_h as i32,
                                glow::RGBA,
                                glow::UNSIGNED_BYTE,
                                glow::PixelPackData::BufferOffset(0),
                            );

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
                        gl.read_pixels(
                            x as i32,
                            y as i32,
                            width as i32,
                            height as i32,
                            glow::RGBA,
                            glow::UNSIGNED_BYTE,
                            glow::PixelPackData::BufferOffset(0),
                        );
                    }
                    if read_issued {
                        match gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) {
                            Ok(fence) => {
                                self.pending.push_back(Slot { buffer, fence, at });
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
}
