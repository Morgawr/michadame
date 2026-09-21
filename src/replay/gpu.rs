//! Read back the already-shaded game rectangle, before egui overlays. No second shader render.
//! Three fenced PBOs; an unfinished transfer is skipped, never waited upon.
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
#[derive(Default)]
pub struct Readback {
    pending: VecDeque<Slot>,
    free: Vec<glow::Buffer>,
    size: (u32, u32),
    changed_at: i64,
    last_at: i64,
    active: Option<std::sync::Weak<super::Shared>>,
}
impl Readback {
    pub fn destroy(&mut self, gl: &glow::Context) {
        unsafe {
            for slot in self.pending.drain(..) {
                gl.delete_sync(slot.fence);
                gl.delete_buffer(slot.buffer);
            }
            for buffer in self.free.drain(..) {
                gl.delete_buffer(buffer);
            }
        }
        self.size = (0, 0);
        self.last_at = 0;
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
        if self.size != (width, height) || at < self.last_at - 250_000 {
            self.destroy(gl);
            self.active = Some(std::sync::Arc::downgrade(&runtime.shared));
            runtime.shared.generation.fetch_add(1, Ordering::AcqRel);
            self.size = (width, height);
            self.changed_at = super::now_us();
        }
        if width == 0 || height == 0 || super::now_us() - self.changed_at < 300_000 {
            return;
        }
        let Some(bytes) = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
        else {
            return;
        };
        if bytes > 256 * 1024 * 1024
            || super::config::overhead(width, height).unwrap_or(usize::MAX) >= runtime.budget / 2
        {
            runtime.shared.message("Replay stopped: surface staging exceeds half the RAM budget; increase budget or reduce window size");
            runtime.shared.stop.store(true, Ordering::Release);
            return;
        }
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
            if let Some(slot) = self.pending.front() {
                let result = gl.client_wait_sync(slot.fence, 0, 0);
                if result == glow::WAIT_FAILED {
                    runtime
                        .shared
                        .message("Replay stopped: GPU readback fence failed");
                    runtime.shared.stop.store(true, Ordering::Release);
                } else if result == glow::ALREADY_SIGNALED || result == glow::CONDITION_SATISFIED {
                    let slot = self.pending.pop_front().unwrap();
                    if !runtime.video.is_full() {
                        let mut rgba = runtime.recycled.try_recv().unwrap_or_default();
                        rgba.resize(bytes, 0);
                        gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(slot.buffer));
                        gl.get_buffer_sub_data(glow::PIXEL_PACK_BUFFER, 0, &mut rgba);
                        if runtime
                            .video
                            .try_send(VideoFrame {
                                rgba,
                                width,
                                height,
                                at: slot.at,
                                rate: slot.rate,
                                generation: runtime.shared.generation.load(Ordering::Acquire),
                            })
                            .is_err()
                        {
                            runtime.shared.dropped.fetch_add(1, Ordering::Relaxed);
                        }
                    } else {
                        runtime.shared.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                    gl.delete_sync(slot.fence);
                    self.free.push(slot.buffer);
                }
            }
            if at > self.last_at && !runtime.shared.stop.load(Ordering::Relaxed) {
                self.last_at = at;
                let buffer = self.free.pop().or_else(|| {
                    if self.pending.len() < 3 {
                        gl.create_buffer().ok().inspect(|b| {
                            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(*b));
                            gl.buffer_data_size(
                                glow::PIXEL_PACK_BUFFER,
                                bytes as i32,
                                glow::STREAM_READ,
                            );
                        })
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
                    runtime.shared.dropped.fetch_add(1, Ordering::Relaxed);
                }
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
    video: crossbeam_channel::Sender<VideoFrame>,
    recycled: crossbeam_channel::Receiver<Vec<u8>>,
    shared: std::sync::Arc<super::Shared>,
    budget: usize,
}
impl RuntimeView {
    pub fn new(runtime: &Runtime, budget: usize) -> Self {
        Self {
            video: runtime.video.clone(),
            recycled: runtime.recycled.clone(),
            shared: runtime.shared.clone(),
            budget,
        }
    }
}
