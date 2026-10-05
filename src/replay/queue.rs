//! Bounded, reusable CPU frames. Only the recording worker allocates/prefaults
//! image storage; the GL callback borrows a ready buffer or keeps its GPU copy.
use super::{
    config::{QueuePlan, MAX_WORK_FRAMES},
    VideoFrame,
};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

#[derive(Clone, Copy)]
pub struct PoolRequest {
    pub generation: u64,
    pub plan: QueuePlan,
}

pub struct WorkQueue {
    ready_tx: Sender<VideoFrame>,
    ready: Receiver<VideoFrame>,
    free_tx: Sender<Vec<u8>>,
    free: Receiver<Vec<u8>>,
    requests_tx: Sender<PoolRequest>,
    requests: Receiver<PoolRequest>,
    generation: AtomicU64,
}
impl WorkQueue {
    pub fn new() -> Self {
        let (ready_tx, ready) = bounded(MAX_WORK_FRAMES);
        let (free_tx, free) = bounded(MAX_WORK_FRAMES);
        let (requests_tx, requests) = bounded(1);
        Self {
            ready_tx,
            ready,
            free_tx,
            free,
            requests_tx,
            requests,
            generation: AtomicU64::new(u64::MAX),
        }
    }
    pub fn request(&self, request: PoolRequest) -> bool {
        self.requests_tx.try_send(request).is_ok()
    }
    pub fn allocation_request(&self) -> Option<PoolRequest> {
        self.requests.try_recv().ok()
    }
    /// Worker only. Invalidate and release the old generation before allocating
    /// its replacement, so resizing cannot duplicate two CPU pools in memory.
    pub fn prepare(
        &self,
        request: PoolRequest,
        current: &AtomicU64,
        stop: &AtomicBool,
    ) -> anyhow::Result<bool> {
        if stop.load(Ordering::Acquire) {
            self.clear();
            return Ok(false);
        }
        if current.load(Ordering::Acquire) != request.generation {
            return Ok(false);
        }
        if self.generation.load(Ordering::Acquire) == request.generation {
            // Retried allocation notifications must never discard queued work.
            return Ok(true);
        }
        self.generation.store(u64::MAX, Ordering::Release);
        while self.ready.try_recv().is_ok() {}
        while self.free.try_recv().is_ok() {}
        for _ in 0..request.plan.slots {
            if current.load(Ordering::Acquire) != request.generation || stop.load(Ordering::Acquire)
            {
                return Ok(false);
            }
            let mut rgba = Vec::new();
            rgba.try_reserve_exact(request.plan.frame_bytes)?;
            // Touch the pages here, not lazily on the rendering thread's copy.
            rgba.resize(request.plan.frame_bytes, 0xff);
            self.free_tx
                .try_send(rgba)
                .map_err(|_| anyhow::anyhow!("Replay frame pool overflow"))?;
        }
        self.generation.store(request.generation, Ordering::Release);
        Ok(true)
    }
    pub fn buffer(&self, generation: u64) -> Option<Vec<u8>> {
        if self.generation.load(Ordering::Acquire) != generation {
            return None;
        }
        self.free.try_recv().ok()
    }
    pub fn submit(
        &self,
        frame: VideoFrame,
    ) -> Result<(), crossbeam_channel::TrySendError<VideoFrame>> {
        self.ready_tx.try_send(frame)
    }
    pub fn receive(&self, timeout: Duration) -> Option<VideoFrame> {
        self.ready.recv_timeout(timeout).ok()
    }
    pub fn recycle(&self, generation: u64, rgba: Vec<u8>) {
        if self.generation.load(Ordering::Acquire) == generation {
            let _ = self.free_tx.try_send(rgba);
        }
    }
    pub fn len(&self) -> usize {
        self.ready.len()
    }
    pub fn clear(&self) {
        self.generation.store(u64::MAX, Ordering::Release);
        while self.ready.try_recv().is_ok() {}
        while self.free.try_recv().is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::config::{Rate, ReplayConfig};

    fn setup() -> (WorkQueue, PoolRequest, AtomicU64) {
        let queue = WorkQueue::new();
        let request = PoolRequest {
            generation: 1,
            plan: QueuePlan {
                slots: 32,
                frame_bytes: 16,
                queue_bytes: 1024,
                overhead: 1024,
            },
        };
        let generation = AtomicU64::new(1);
        assert!(queue.request(request));
        let allocation = queue.allocation_request().unwrap();
        assert!(queue
            .prepare(allocation, &generation, &AtomicBool::new(false))
            .unwrap());
        (queue, request, generation)
    }
    fn frame(queue: &WorkQueue, generation: u64, tick: u8) -> VideoFrame {
        let mut rgba = queue.buffer(generation).unwrap();
        rgba.fill(tick);
        let rate = Rate::new(60, 1);
        VideoFrame {
            rgba,
            width: 2,
            height: 2,
            at: 10_000_000 + rate.us(tick as i64),
            rate,
            generation,
        }
    }

    #[test]
    fn encoder_stall_retains_every_frame_in_order_and_reuses_allocations() {
        let (queue, _, _) = setup();
        let mut addresses = Vec::new();
        // Half a second of rendered frames accumulates without a consumer.
        for tick in 0..30 {
            let frame = frame(&queue, 1, tick);
            addresses.push(frame.rgba.as_ptr() as usize);
            queue.submit(frame).unwrap();
        }
        let mut schedule = super::super::worker::VideoSchedule::new(10_000_000);
        for tick in 0..30 {
            let frame = queue.receive(Duration::ZERO).unwrap();
            assert_eq!(
                frame.rgba,
                vec![tick; 16],
                "Queued shader output was lost or overwritten"
            );
            assert_eq!(schedule.accept(frame.at).unwrap().pts, frame.at);
            queue.recycle(frame.generation, frame.rgba);
        }
        assert_eq!(queue.len(), 0);
        let mut reused = 0;
        for _ in 0..32 {
            let buffer = queue.buffer(1).unwrap();
            reused += usize::from(addresses.contains(&(buffer.as_ptr() as usize)));
        }
        assert_eq!(reused, 30);
    }

    #[test]
    fn full_pool_applies_backpressure_without_evicting_queued_frames() {
        let (queue, _, _) = setup();
        for tick in 0..32 {
            queue.submit(frame(&queue, 1, tick)).unwrap();
        }
        assert!(
            queue.buffer(1).is_none(),
            "GPU must retain its pending frame until a CPU buffer returns"
        );
        let oldest = queue.receive(Duration::ZERO).unwrap();
        assert_eq!(oldest.rgba[0], 0);
        assert!(
            queue.buffer(1).is_none(),
            "In-flight encoding also owns a pool slot"
        );
        queue.recycle(1, oldest.rgba);
        queue.submit(frame(&queue, 1, 32)).unwrap();
        for tick in 1..=32 {
            let frame = queue.receive(Duration::ZERO).unwrap();
            assert_eq!(frame.rgba[0], tick);
        }
    }

    #[test]
    fn allocation_retry_and_stale_request_preserve_queued_frames() {
        let (queue, request, generation) = setup();
        queue.submit(frame(&queue, 1, 1)).unwrap();
        let held = frame(&queue, 1, 2);
        let stop = AtomicBool::new(false);
        assert!(queue.prepare(request, &generation, &stop).unwrap());
        assert_eq!(queue.len(), 1, "A repeated request discarded pending work");
        let stale = PoolRequest {
            generation: 0,
            ..request
        };
        assert!(!queue.prepare(stale, &generation, &stop).unwrap());
        assert_eq!(queue.len(), 1, "A stale request discarded current work");
        let oldest = queue.receive(Duration::ZERO).unwrap();
        assert_eq!(oldest.rgba[0], 1);
        queue.recycle(1, oldest.rgba);
        queue.recycle(1, held.rgba);
        for _ in 0..request.plan.slots {
            assert!(queue.buffer(1).is_some());
        }
        assert!(queue.buffer(1).is_none(), "Retry allocated a second pool");
    }

    #[test]
    fn resize_discards_only_old_generation_and_stop_cancels_allocation() {
        let (queue, mut request, generation) = setup();
        let held = frame(&queue, 1, 0);
        queue.submit(frame(&queue, 1, 1)).unwrap();
        generation.store(2, Ordering::Release);
        request.generation = 2;
        request.plan.frame_bytes = 32;
        assert!(queue
            .prepare(request, &generation, &AtomicBool::new(false))
            .unwrap());
        queue.recycle(1, held.rgba); // A late old buffer must not enter the new pool.
        assert!(queue.buffer(1).is_none());
        assert_eq!(queue.len(), 0);
        for _ in 0..32 {
            assert_eq!(queue.buffer(2).unwrap().len(), 32);
        }
        assert!(queue.buffer(2).is_none());
        assert!(!queue
            .prepare(request, &generation, &AtomicBool::new(true))
            .unwrap());
        assert!(queue.buffer(2).is_none());
        queue.clear();
    }

    #[test]
    fn queue_memory_scales_with_resolution_and_never_consumes_entire_history_budget() {
        let config = ReplayConfig::default();
        assert_eq!(config.queue_plan(1920, 1080).unwrap().slots, 32);
        assert_eq!(config.queue_plan(3840, 2160).unwrap().slots, 8);
        for budget in [256, 512, 1024, 4096, 32768] {
            let config = ReplayConfig {
                memory_mib: budget,
                work_queue_mib: 8192,
                ..Default::default()
            };
            for (w, h) in [(640, 480), (1920, 1080), (3840, 2160), (7680, 4320)] {
                if let Some(plan) = config.queue_plan(w, h) {
                    assert!(plan.slots >= 2 && plan.slots <= MAX_WORK_FRAMES);
                    assert_eq!(plan.queue_bytes, 2 * plan.slots * plan.frame_bytes);
                    assert!(plan.queue_bytes <= config.budget() / 2);
                    assert!(plan.overhead < config.budget());
                }
            }
        }
        assert!(config.queue_plan(0, 1080).is_none());
        assert!(config.queue_plan(u32::MAX, u32::MAX).is_none());
    }
}
