//! Read back the already-shaded game rectangle, before egui overlays. No second shader render.
//! Budgeted fenced PBO queue; retain unfinished/blocked transfers without waiting.
use super::{config::Rate, Runtime, VideoFrame};
use crate::video::gpu::geometry::RenderedArea;
use eframe::glow::{self, HasContext};
use std::{collections::VecDeque, sync::atomic::Ordering};
struct Slot {
    buffer: glow::Buffer,
    fence: glow::Fence,
    at: i64,
    rate: Rate,
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
#[derive(Default)]
pub struct Readback {
    pending: VecDeque<Slot>,
    free: Vec<glow::Buffer>,
    surface: SurfaceState,
    last_at: i64,
    active: Option<std::sync::Weak<super::Shared>>,
    requested: Option<u64>,
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
        }
        self.last_at = 0;
        self.requested = None;
    }
    pub fn destroy(&mut self, gl: &glow::Context) {
        self.release_buffers(gl);
        self.surface = SurfaceState::default();
        self.active = None;
    }
    pub fn capture(
        &mut self,
        gl: &glow::Context,
        runtime: Option<&RuntimeView>,
        area: RenderedArea,
        at: i64,
        rate: Rate,
    ) {
        let RenderedArea {
            x,
            y,
            width,
            height,
        } = area;
        let Some(runtime) = runtime.filter(|r| !r.shared.stop.load(Ordering::Acquire)) else {
            self.destroy(gl);
            return;
        };
        if self
            .active
            .as_ref()
            .and_then(|w| w.upgrade())
            .is_none_or(|old| !std::sync::Arc::ptr_eq(&old, &runtime.shared))
        {
            self.destroy(gl);
            self.active = Some(std::sync::Arc::downgrade(&runtime.shared));
        }
        // Missing/stale capture timestamps cannot establish a new recording
        // epoch. Real stream restarts already disable replay. In particular,
        // never confuse an out-of-order frame with a backwards system clock.
        let Some(resized) = self.observe_surface((width, height), at, super::now_us()) else {
            return;
        };
        if resized {
            self.release_buffers(gl);
            runtime.shared.generation.fetch_add(1, Ordering::AcqRel);
        }
        let Some(plan) = runtime.config.queue_plan(width, height) else {
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
        let generation = runtime.shared.generation.load(Ordering::Acquire);
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
        if self.requested != Some(generation)
            && runtime
                .queue
                .request(super::queue::PoolRequest { generation, plan })
        {
            self.requested = Some(generation);
        }
        let bytes = plan.frame_bytes;
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
                    runtime
                        .shared
                        .message("Replay stopped: GPU readback fence failed");
                    runtime.shared.stop.store(true, Ordering::Release);
                    break;
                }
                if result != glow::ALREADY_SIGNALED && result != glow::CONDITION_SATISFIED {
                    break;
                }
                let Some(mut rgba) = runtime.queue.buffer(generation) else {
                    break;
                };
                let slot = self.pending.pop_front().unwrap();
                debug_assert_eq!(rgba.len(), bytes);
                gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(slot.buffer));
                gl.get_buffer_sub_data(glow::PIXEL_PACK_BUFFER, 0, &mut rgba);
                if let Err(error) = runtime.queue.submit(VideoFrame {
                    rgba,
                    width,
                    height,
                    at: slot.at,
                    rate: slot.rate,
                    generation,
                }) {
                    runtime.queue.recycle(generation, error.into_inner().rgba);
                    runtime.shared.video_dropped.fetch_add(1, Ordering::Relaxed);
                }
                gl.delete_sync(slot.fence);
                self.free.push(slot.buffer);
                if copying_started.elapsed() >= std::time::Duration::from_millis(2) {
                    break;
                }
            }
            if at > self.last_at && !runtime.shared.stop.load(Ordering::Relaxed) {
                self.last_at = at;
                runtime.shared.captured_at.store(at, Ordering::Relaxed);
                let buffer = self.free.pop().or_else(|| {
                    if self.pending.len() < plan.slots {
                        match gl.create_buffer() {
                            Ok(buffer) => {
                                gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(buffer));
                                gl.buffer_data_size(glow::PIXEL_PACK_BUFFER, bytes as i32, glow::STREAM_READ);
                                let error = gl.get_error();
                                if error == glow::NO_ERROR { Some(buffer) } else {
                                    gl.delete_buffer(buffer);
                                    runtime.shared.message(format!("Replay stopped: cannot allocate GPU work buffer (GL {error:#x})"));
                                    runtime.shared.stop.store(true, Ordering::Release);
                                    None
                                }
                            }
                            Err(error) => {
                                runtime.shared.message(format!("Replay stopped: cannot create GPU work buffer: {error}"));
                                runtime.shared.stop.store(true, Ordering::Release);
                                None
                            }
                        }
                    } else {
                        None
                    }
                });
                if let Some(buffer) = buffer {
                    gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(buffer));
                    gl.read_pixels(
                        x as i32,
                        y as i32,
                        width as i32,
                        height as i32,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelPackData::BufferOffset(0),
                    );
                    match gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) {
                        Ok(fence) => {
                            self.pending.push_back(Slot {
                                buffer,
                                fence,
                                at,
                                rate,
                            });
                            self.last_at = at;
                        }
                        Err(e) => {
                            gl.delete_buffer(buffer);
                            runtime.shared.message(format!("Replay stopped: {e}"));
                            runtime.shared.stop.store(true, Ordering::Release);
                        }
                    }
                } else {
                    runtime.shared.video_dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
            runtime
                .shared
                .gpu_pending
                .store(self.pending.len(), Ordering::Relaxed);
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
                            rate,
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
                let (pts, key) = schedule.accept(frame.at).unwrap();
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
}
