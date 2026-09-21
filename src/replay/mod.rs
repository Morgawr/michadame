pub mod config;
mod encoder;
pub mod gpu;
mod ring;
pub mod ui;
mod worker;

use config::{Rate, ReplayConfig};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
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
struct AudioBlock {
    samples: Box<[f32; AUDIO_SAMPLES]>,
    len: usize,
    rate: u32,
    channels: u16,
    at: i64,
}
struct AudioRoute {
    ready: Sender<AudioBlock>,
    free: Receiver<AudioBlock>,
    stop: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
}
/// Only the ALSA capture thread touches this tap; the playback callback is unchanged.
#[derive(Default)]
pub struct AudioTap {
    enabled: AtomicBool,
    route: Mutex<Option<AudioRoute>>,
}
impl AudioTap {
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    pub fn submit(&self, samples: &[f32], rate: u32, channels: u16, at: i64) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let Ok(route) = self.route.try_lock() else {
            return;
        };
        let Some(route) = route.as_ref() else {
            return;
        };
        if route.stop.load(Ordering::Relaxed) || channels == 0 || rate == 0 {
            return;
        }
        let chunk_len = AUDIO_SAMPLES / channels as usize * channels as usize;
        if chunk_len == 0 {
            return;
        }
        for (i, samples) in samples.chunks(chunk_len).enumerate() {
            let Ok(mut block) = route.free.try_recv() else {
                route.dropped.fetch_add(1, Ordering::Relaxed);
                return;
            };
            block.samples[..samples.len()].copy_from_slice(samples);
            block.len = samples.len();
            block.rate = rate;
            block.channels = channels;
            block.at = at + (i * chunk_len) as i64 * 1_000_000 / (rate as i64 * channels as i64);
            if route.ready.try_send(block).is_err() {
                route.dropped.fetch_add(1, Ordering::Relaxed);
                return;
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
    pub dropped: u64,
    pub saving: bool,
}
pub struct Shared {
    pub generation: AtomicU64,
    pub stop: Arc<AtomicBool>,
    pub dropped: Arc<AtomicU64>,
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
    pub video: Sender<VideoFrame>,
    pub recycled: Receiver<Vec<u8>>,
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
            dropped: Arc::new(AtomicU64::new(0)),
            status: Mutex::new(Status {
                message: "Waiting for rendered video and audio…".into(),
                available: Some(available),
                ..Default::default()
            }),
            saving_bytes: Default::default(),
        });
        let (vtx, vrx) = bounded(2);
        let (recycle_tx, recycled) = bounded(3);
        let (atx, arx) = bounded(32);
        let (free_tx, free_rx) = bounded(32);
        for _ in 0..32 {
            free_tx.send(AudioBlock {
                samples: Box::new([0.; AUDIO_SAMPLES]),
                len: 0,
                rate: 48000,
                channels: 2,
                at: 0,
            })?;
        }
        let (save, saves) = bounded(1);
        let config = self.config.clone();
        let worker_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("replay-encode".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker::run(config, vrx, recycle_tx, arx, free_tx, saves, &worker_shared)
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
            })?;
        *self.audio.route.lock().unwrap() = Some(AudioRoute {
            ready: atx,
            free: free_rx,
            stop: shared.stop.clone(),
            dropped: shared.dropped.clone(),
        });
        self.audio.enabled.store(true, Ordering::Release);
        self.available_before = Some(available);
        self.runtime = Some(Runtime {
            video: vtx,
            recycled,
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
            .map(|r| r.shared.status.lock().unwrap().clone())
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
        let tap = AudioTap::default();
        let (tx, rx) = bounded(1);
        let (free, available) = bounded(1);
        free.send(AudioBlock {
            samples: Box::new([0.; AUDIO_SAMPLES]),
            len: 0,
            rate: 0,
            channels: 0,
            at: 0,
        })
        .unwrap();
        let dropped = Arc::new(AtomicU64::new(0));
        *tap.route.lock().unwrap() = Some(AudioRoute {
            ready: tx,
            free: available,
            stop: Arc::new(AtomicBool::new(false)),
            dropped: dropped.clone(),
        });
        tap.enabled.store(true, Ordering::Relaxed);
        tap.submit(&[0.5; 10], 48000, 2, 1000);
        tap.submit(&[0.6; 10], 48000, 2, 2000);
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
