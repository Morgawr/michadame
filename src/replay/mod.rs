pub mod config;
mod encoder;
pub mod gpu;
mod queue;
mod ring;
pub mod ui;
mod worker;

use config::{Rate, ReplayConfig};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::{
    atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex,
};

pub fn now_us() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // CLOCK_MONOTONIC is also the normal V4L2 timestamp domain on Linux.
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
    }
    ts.tv_sec * 1_000_000 + ts.tv_nsec / 1000
}

pub struct VideoFrame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub at: i64,
    pub rate: Rate,
    pub generation: u64,
}
const AUDIO_SAMPLES: usize = 8192;
const AUDIO_BLOCKS: usize = 256;
struct AudioBlock {
    samples: Box<[f32; AUDIO_SAMPLES]>,
    len: usize,
    rate: u32,
    channels: u16,
    at: i64,
    /// All samples since the preceding block were captured and queued.
    continuous: bool,
}
struct AudioRoute {
    ready: Sender<AudioBlock>,
    free: Receiver<AudioBlock>,
    stop: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
    partial: Option<AudioBlock>,
    next_continuous: bool,
    format: Option<(u32, u16)>,
}
/// Only the ALSA capture thread touches this tap; the playback callback is unchanged.
#[derive(Default)]
pub struct AudioTap {
    enabled: AtomicBool,
    discontinuity: AtomicBool,
    route: Mutex<Option<AudioRoute>>,
}
impl AudioTap {
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    /// Called on ALSA restart/recovery, independently of whether replay is enabled.
    pub fn mark_discontinuity(&self) {
        self.discontinuity.store(true, Ordering::Relaxed);
    }
    pub fn submit(&self, samples: &[f32], rate: u32, channels: u16, at: i64) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let Ok(mut route) = self.route.try_lock() else {
            self.mark_discontinuity();
            return;
        };
        let Some(route) = route.as_mut() else {
            return;
        };
        if route.stop.load(Ordering::Relaxed) || channels == 0 || rate == 0 {
            return;
        }
        let channels_usize = channels as usize;
        // Batch short ALSA reads into ~40ms recording blocks. Pool capacity is
        // measured in audio time, not in unpredictable read calls (1.28s at
        // 48kHz stereo). This never delays samples going to live playback.
        let frames = (rate as usize / 25)
            .max(1)
            .min(AUDIO_SAMPLES / channels_usize);
        let chunk_len = frames * channels_usize;
        if chunk_len == 0 {
            return;
        }
        let changed = route
            .format
            .is_some_and(|format| format != (rate, channels));
        route.format = Some((rate, channels));
        if self.discontinuity.swap(false, Ordering::Relaxed) || changed {
            if let Some(block) = route.partial.take() {
                if route.ready.try_send(block).is_err() {
                    route.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
            route.next_continuous = false;
        }
        let mut offset = 0;
        let len = samples.len() / channels_usize * channels_usize;
        while offset < len {
            if route.partial.is_none() {
                let Ok(mut block) = route.free.try_recv() else {
                    route.dropped.fetch_add(1, Ordering::Relaxed);
                    route.next_continuous = false;
                    return;
                };
                block.len = 0;
                block.rate = rate;
                block.channels = channels;
                block.at = at + offset as i64 * 1_000_000 / (rate as i64 * channels as i64);
                block.continuous = route.next_continuous;
                route.partial = Some(block);
            }
            let block = route.partial.as_mut().unwrap();
            let count = (chunk_len - block.len).min(len - offset);
            block.samples[block.len..block.len + count]
                .copy_from_slice(&samples[offset..offset + count]);
            block.len += count;
            offset += count;
            if block.len == chunk_len {
                let block = route.partial.take().unwrap();
                route.next_continuous = true;
                if route.ready.try_send(block).is_err() {
                    route.dropped.fetch_add(1, Ordering::Relaxed);
                    route.next_continuous = false;
                    return;
                }
            }
        }
    }
    fn disable(&self) {
        self.enabled.store(false, Ordering::Release);
        *self.route.lock().unwrap() = None;
    }
}
#[derive(Default, Clone)]
pub struct Status {
    pub message: String,
    pub seconds: f64,
    pub bytes: usize,
    pub overhead: usize,
    pub available: Option<usize>,
    pub surface: Option<(u32, u32)>,
    pub codec: String,
    pub video_dropped: u64,
    pub audio_dropped: u64,
    pub gpu_pending: usize,
    pub cpu_pending: usize,
    pub queue_slots: usize,
    pub queue_bytes: usize,
    pub backlog_ms: i64,
    pub saving: bool,
}
pub struct Shared {
    pub generation: AtomicU64,
    pub stop: Arc<AtomicBool>,
    pub audio_dropped: Arc<AtomicU64>,
    pub video_dropped: AtomicU64,
    pub gpu_pending: AtomicUsize,
    pub queue_slots: AtomicUsize,
    pub queue_bytes: AtomicUsize,
    pub staging_bytes: AtomicUsize,
    pub captured_at: AtomicI64,
    pub processed_at: AtomicI64,
    pub status: Mutex<Status>,
    pub saving_bytes: std::sync::atomic::AtomicUsize,
}
impl Shared {
    fn message(&self, text: impl Into<String>) {
        if let Ok(mut status) = self.status.try_lock() {
            status.message = text.into();
        }
    }
}
pub struct Save {
    seconds: u32,
    at: i64,
}
pub struct Runtime {
    pub queue: Arc<queue::WorkQueue>,
    pub config: Arc<ReplayConfig>,
    pub shared: Arc<Shared>,
    save: Sender<Save>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        // Worker and encoder teardown never block the GUI/capture threads.
        if let Some(thread) = self.thread.take() {
            let _ = std::thread::Builder::new()
                .name("replay-cleanup".into())
                .spawn(move || {
                    let _ = thread.join();
                });
        }
    }
}
#[derive(Default)]
pub struct Replay {
    pub config: ReplayConfig,
    pub audio: Arc<AudioTap>,
    pub gpu: Arc<Mutex<gpu::Readback>>,
    pub runtime: Option<Runtime>,
    pub last_status: Status,
    pub available_before: Option<usize>,
    retired: Option<Arc<Shared>>,
    memory_checked: Option<std::time::Instant>,
    pub available_now: Option<usize>,
    last_notice: String,
}
impl Replay {
    pub fn enable(&mut self) -> anyhow::Result<()> {
        self.config.validate()?;
        anyhow::ensure!(self.runtime.is_none(), "Replay is already enabled");
        if let Some(old) = &self.retired {
            anyhow::ensure!(
                Arc::strong_count(old) == 1,
                "Replay is still releasing resources or finishing a save"
            );
        }
        self.retired = None;
        let available = config::available_memory()
            .ok_or_else(|| anyhow::anyhow!("Cannot read available RAM"))?;
        anyhow::ensure!(
            self.config.budget().saturating_add(config::SAFETY_RESERVE) < available,
            "Replay budget exceeds available RAM after the 512 MiB safety reserve"
        );
        let shared = Arc::new(Shared {
            generation: AtomicU64::new(0),
            stop: Arc::new(AtomicBool::new(false)),
            audio_dropped: Arc::new(AtomicU64::new(0)),
            video_dropped: Default::default(),
            gpu_pending: Default::default(),
            queue_slots: Default::default(),
            queue_bytes: Default::default(),
            staging_bytes: AtomicUsize::new(64 * 1024 * 1024),
            captured_at: Default::default(),
            processed_at: Default::default(),
            status: Mutex::new(Status {
                message: "Waiting for rendered video and audio…".into(),
                available: Some(available),
                ..Default::default()
            }),
            saving_bytes: Default::default(),
        });
        let queue = Arc::new(queue::WorkQueue::new());
        let worker_queue = queue.clone();
        let (atx, arx) = bounded(AUDIO_BLOCKS);
        let (free_tx, free_rx) = bounded(AUDIO_BLOCKS);
        for _ in 0..AUDIO_BLOCKS {
            free_tx.send(AudioBlock {
                samples: Box::new([0.; AUDIO_SAMPLES]),
                len: 0,
                rate: 48000,
                channels: 2,
                at: 0,
                continuous: false,
            })?;
        }
        let (save, saves) = bounded(1);
        let config = self.config.clone();
        let worker_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("replay-encode".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker::run(config, &worker_queue, arx, free_tx, saves, &worker_shared)
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Recording worker panicked")));
                if let Err(e) = result {
                    worker_shared.message(format!("Replay stopped: {e:#}"));
                }
                {
                    let mut status = worker_shared.status.lock().unwrap();
                    status.seconds = 0.;
                    status.bytes = worker_shared.saving_bytes.load(Ordering::Acquire);
                    status.overhead = 0;
                    status.surface = None;
                }
                worker_shared.stop.store(true, Ordering::Release);
                worker_queue.clear();
            })?;
        *self.audio.route.lock().unwrap() = Some(AudioRoute {
            ready: atx,
            free: free_rx,
            stop: shared.stop.clone(),
            dropped: shared.audio_dropped.clone(),
            partial: None,
            next_continuous: false,
            format: None,
        });
        self.audio.enabled.store(true, Ordering::Release);
        self.available_before = Some(available);
        self.runtime = Some(Runtime {
            queue,
            config: Arc::new(self.config.clone()),
            shared,
            save,
            thread: Some(thread),
        });
        Ok(())
    }
    pub fn disable(&mut self) {
        self.audio.disable();
        if let Some(runtime) = self.runtime.take() {
            if !runtime.shared.stop.load(Ordering::Acquire) {
                runtime.shared.message(
                    "Replay disabled; unsaved history released (pending requests cancelled)",
                );
            }
            self.last_status = runtime.shared.status.lock().unwrap().clone();
            self.retired = Some(runtime.shared.clone());
            drop(runtime);
        }
    }
    pub fn save(&mut self, seconds: u32) -> anyhow::Result<()> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Enable replay first"))?;
        anyhow::ensure!(
            !runtime.shared.stop.load(Ordering::Acquire),
            "Replay has stopped; see its status"
        );
        anyhow::ensure!(
            !runtime.shared.status.lock().unwrap().saving,
            "A replay is already being saved"
        );
        runtime
            .save
            .try_send(Save {
                seconds: seconds.min(self.config.history_seconds),
                at: now_us(),
            })
            .map_err(|_| anyhow::anyhow!("A replay save is already pending"))
    }
    pub fn notification(&mut self) -> Option<String> {
        let message = self.status().message;
        if message != self.last_notice
            && (message.starts_with("Saved ")
                || message.starts_with("Replay stopped:")
                || message.starts_with("Replay save failed:")
                || message.starts_with("Not enough"))
        {
            self.last_notice = message.clone();
            Some(message)
        } else {
            None
        }
    }
    pub fn status(&self) -> Status {
        self.runtime
            .as_ref()
            .map(|r| {
                let mut status = r.shared.status.lock().unwrap().clone();
                status.video_dropped = r.shared.video_dropped.load(Ordering::Relaxed);
                status.audio_dropped = r.shared.audio_dropped.load(Ordering::Relaxed);
                status.gpu_pending = r.shared.gpu_pending.load(Ordering::Relaxed);
                status.cpu_pending = r.queue.len();
                status.queue_slots = r.shared.queue_slots.load(Ordering::Relaxed);
                status.queue_bytes = r.shared.queue_bytes.load(Ordering::Relaxed);
                if !r.shared.stop.load(Ordering::Acquire) {
                    status.overhead = r.shared.staging_bytes.load(Ordering::Acquire);
                }
                let processed = r.shared.processed_at.load(Ordering::Relaxed);
                status.backlog_ms = if processed > 0 {
                    (r.shared.captured_at.load(Ordering::Relaxed) - processed).max(0) / 1000
                } else {
                    0
                };
                status
            })
            .or_else(|| {
                self.retired
                    .as_ref()
                    .map(|s| s.status.lock().unwrap().clone())
            })
            .unwrap_or_else(|| self.last_status.clone())
    }
    pub fn update(&mut self) {
        if self
            .memory_checked
            .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(1))
        {
            self.available_now = config::available_memory();
            self.memory_checked = Some(std::time::Instant::now());
        }
        if self
            .runtime
            .as_ref()
            .is_some_and(|r| r.shared.stop.load(Ordering::Acquire))
        {
            self.disable();
        }
    }
    pub fn shortcuts(&mut self, ctx: &eframe::egui::Context) {
        if self.runtime.is_none() || ctx.wants_keyboard_input() {
            return;
        }
        for (key, seconds) in self.config.keys.into_iter().zip(self.config.durations()) {
            if let Some(key) = ui::key(key) {
                let pressed = ctx.input_mut(|i| i.consume_key(eframe::egui::Modifiers::NONE, key));
                if pressed {
                    if let Err(e) = self.save(seconds) {
                        self.last_status.message = e.to_string();
                        if let Some(r) = &self.runtime {
                            r.shared.message(e.to_string());
                        }
                    }
                }
            }
        }
    }
}
impl Drop for Replay {
    fn drop(&mut self) {
        self.disable();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn test_tap(
        capacity: usize,
    ) -> (
        AudioTap,
        Receiver<AudioBlock>,
        Sender<AudioBlock>,
        Arc<AtomicU64>,
    ) {
        let tap = AudioTap::default();
        let (tx, rx) = bounded(capacity);
        let (free, available) = bounded(capacity);
        for _ in 0..capacity {
            free.send(AudioBlock {
                samples: Box::new([0.; AUDIO_SAMPLES]),
                len: 0,
                rate: 48000,
                channels: 2,
                at: 0,
                continuous: false,
            })
            .unwrap();
        }
        let dropped = Arc::new(AtomicU64::new(0));
        *tap.route.lock().unwrap() = Some(AudioRoute {
            ready: tx,
            free: available,
            stop: Arc::new(AtomicBool::new(false)),
            dropped: dropped.clone(),
            partial: None,
            next_continuous: false,
            format: None,
        });
        tap.enabled.store(true, Ordering::Relaxed);
        (tap, rx, free, dropped)
    }
    #[test]
    fn small_capture_reads_survive_a_busy_video_encoder() {
        let (tap, rx, free, dropped) = test_tap(32);
        let mut received = 0;
        for read in 0..1000 {
            // ALSA can return short reads. A 100ms video encode must not lose
            // audio merely because each 1ms read occupies a separate pool slot.
            tap.submit(&[0.5; 96], 48000, 2, read * 1000);
            if read % 100 == 99 {
                while let Ok(block) = rx.try_recv() {
                    received += block.len;
                    assert!(block.samples[..block.len].iter().all(|v| *v == 0.5));
                    free.send(block).unwrap();
                }
            }
        }
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        assert_eq!(received, 96000);
    }
    #[test]
    fn real_audio_loss_and_capture_restart_break_continuity() {
        let (tap, rx, free, dropped) = test_tap(1);
        tap.submit(&[0.5; 3840], 48000, 2, 0);
        tap.submit(&[0.5; 96], 48000, 2, 40_000); // Pool full: actual 1ms loss.
        let first = rx.try_recv().unwrap();
        assert!(!first.continuous);
        free.send(first).unwrap();
        tap.submit(&[0.5; 3840], 48000, 2, 41_000);
        let recovered = rx.try_recv().unwrap();
        assert!(!recovered.continuous);
        assert_eq!(recovered.at, 41_000);
        free.send(recovered).unwrap();
        tap.submit(&[0.5; 3840], 48000, 2, 81_000);
        let continuous = rx.try_recv().unwrap();
        assert!(continuous.continuous);
        free.send(continuous).unwrap();
        tap.mark_discontinuity();
        tap.submit(&[0.5; 3840], 48000, 2, 1_000_000);
        assert!(!rx.try_recv().unwrap().continuous);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn capture_restart_flushes_partial_audio_before_reanchoring() {
        let (tap, rx, _, dropped) = test_tap(2);
        tap.submit(&[0.25; 96], 48000, 2, 0);
        assert!(rx.is_empty());
        tap.mark_discontinuity();
        tap.submit(&[0.75; 3840], 48000, 2, 100_000);
        let before = rx.try_recv().unwrap();
        let after = rx.try_recv().unwrap();
        assert_eq!(before.len, 96);
        assert_eq!(before.samples[0], 0.25);
        assert_eq!(after.at, 100_000);
        assert_eq!(after.samples[0], 0.75);
        assert!(!after.continuous);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }
    #[test]
    fn disabled_tap_and_contended_tap_never_wait() {
        let tap = AudioTap::default();
        let held = tap.route.lock().unwrap();
        tap.submit(&[0.; 10], 48000, 2, 0);
        tap.enabled.store(true, Ordering::Relaxed);
        tap.submit(&[0.; 10], 48000, 2, 0);
        drop(held);
    }
    #[test]
    fn full_audio_pool_drops_recording_without_affecting_playback() {
        let (tap, rx, _, dropped) = test_tap(1);
        tap.submit(&[0.5; 3840], 48000, 2, 1000);
        tap.submit(&[0.6; 3840], 48000, 2, 41000);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        let block = rx.try_recv().unwrap();
        assert_eq!(block.at, 1000);
        assert_eq!(block.samples[0], 0.5);
        tap.disable();
        assert!(!tap.is_enabled());
    }
    #[test]
    fn replay_settings_roundtrip_and_old_config_defaults() {
        let old: crate::config::MichadameConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(old.replay.history_seconds, 300);
        assert_eq!(old.replay.work_queue_mib, 512);
        let mut updated = old;
        updated.replay.history_seconds = 600;
        updated.replay.keys = [1, 2, 3, 4, 5, 6];
        let json = serde_json::to_string(&updated).unwrap();
        let loaded: crate::config::MichadameConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.replay.history_seconds, 600);
        assert_eq!(loaded.replay.keys, [1, 2, 3, 4, 5, 6]);
        assert!(Replay::default().runtime.is_none());
    }
}
