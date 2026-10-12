pub mod clipboard;
pub mod config;
mod encoder;
pub mod gpu;
mod queue;
mod ring;
pub mod ui;
mod worker;

pub(crate) use worker::{AudioClock, MixSource, TimelineMixer};

use config::{Rate, ReplayConfig};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::{
    atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering},
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
    #[allow(dead_code)]
    pub rate: Rate,
    pub generation: u64,
}
pub(crate) const AUDIO_SAMPLES: usize = 8192;
pub(crate) const AUDIO_BLOCKS: usize = 256;
pub(crate) struct AudioBlock {
    pub samples: Box<[f32; AUDIO_SAMPLES]>,
    pub len: usize,
    pub rate: u32,
    pub channels: u16,
    pub at: i64,
    /// All samples since the preceding block were captured and queued.
    pub continuous: bool,
}
pub(crate) struct AudioRoute {
    pub ready: Sender<AudioBlock>,
    pub free: Receiver<AudioBlock>,
    pub stop: Arc<AtomicBool>,
    pub dropped: Arc<AtomicU64>,
    pub partial: Option<AudioBlock>,
    pub next_continuous: bool,
    pub format: Option<(u32, u16)>,
}
fn submit_to_route(
    route_mutex: &Mutex<Option<AudioRoute>>,
    discontinuity: &AtomicBool,
    samples: &[f32],
    rate: u32,
    channels: u16,
    at: i64,
) {
    let Ok(mut route) = route_mutex.try_lock() else {
        discontinuity.store(true, Ordering::Relaxed);
        return;
    };
    let Some(route) = route.as_mut() else {
        return;
    };
    if route.stop.load(Ordering::Relaxed) || channels == 0 || rate == 0 {
        return;
    }
    let channels_usize = channels as usize;
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
    if discontinuity.swap(false, Ordering::Relaxed) || changed {
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

struct ActiveAlertAudio {
    samples: Vec<f32>,
    playhead_frames: f64,
}

/// ALSA capture and microphone capture threads submit audio through this tap.
pub struct AudioTap {
    enabled: AtomicBool,
    discontinuity: AtomicBool,
    mic_discontinuity: AtomicBool,
    route: Mutex<Option<AudioRoute>>,
    mic_route: Mutex<Option<AudioRoute>>,
    broadcast_discontinuity: AtomicBool,
    broadcast_mic_discontinuity: AtomicBool,
    broadcast_route: Mutex<Option<AudioRoute>>,
    broadcast_mic_route: Mutex<Option<AudioRoute>>,
    broadcast_preroll_active: AtomicBool,
    pub filter: std::sync::RwLock<crate::replay::config::AudioFilterConfig>,
    game_volume: Arc<AtomicU32>,
    active_alert: Mutex<Option<ActiveAlertAudio>>,
    last_game_submit: AtomicI64,
}

impl Default for AudioTap {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            discontinuity: AtomicBool::new(false),
            mic_discontinuity: AtomicBool::new(false),
            route: Mutex::new(None),
            mic_route: Mutex::new(None),
            broadcast_discontinuity: AtomicBool::new(false),
            broadcast_mic_discontinuity: AtomicBool::new(false),
            broadcast_route: Mutex::new(None),
            broadcast_mic_route: Mutex::new(None),
            broadcast_preroll_active: AtomicBool::new(false),
            filter: std::sync::RwLock::new(crate::replay::config::AudioFilterConfig::default()),
            game_volume: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            active_alert: Mutex::new(None),
            last_game_submit: AtomicI64::new(0),
        }
    }
}

impl AudioTap {
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    pub fn set_filter_config(&self, config: crate::replay::config::AudioFilterConfig) {
        if let Ok(mut w) = self.filter.write() {
            *w = config;
        }
    }
    pub fn set_game_volume(&self, volume: f32) {
        self.game_volume.store(volume.to_bits(), Ordering::Relaxed);
    }
    #[allow(dead_code)]
    pub fn game_volume(&self) -> f32 {
        f32::from_bits(self.game_volume.load(Ordering::Relaxed))
    }
    /// Called on ALSA restart/recovery, independently of whether replay is enabled.
    pub fn mark_discontinuity(&self) {
        self.discontinuity.store(true, Ordering::Relaxed);
        self.broadcast_discontinuity.store(true, Ordering::Relaxed);
    }
    /// Called on microphone restart/recovery.
    pub fn mark_mic_discontinuity(&self) {
        self.mic_discontinuity.store(true, Ordering::Relaxed);
        self.broadcast_mic_discontinuity.store(true, Ordering::Relaxed);
    }
    /// Plays an alert chime into the stream broadcast and replay routes.
    ///
    /// If ALSA game audio is actively flowing, the chime samples are queued and resampled
    /// on-the-fly to match the ALSA capture rate (e.g. 44.1 kHz, 48 kHz) and channel layout.
    /// If ALSA audio is idle/stopped, the chime is directly submitted as 48 kHz stereo audio blocks.
    pub fn play_alert(&self, samples: &[f32]) {
        if !self.is_enabled() {
            return;
        }
        let now = now_us();
        let last_submit = self.last_game_submit.load(Ordering::Relaxed);

        if last_submit > 0 && (now - last_submit) <= 150_000 {
            // ALSA capture is actively running; queue samples to be resampled and mixed in submit()
            let mut guard = self.active_alert.lock().unwrap();
            *guard = Some(ActiveAlertAudio {
                samples: samples.to_vec(),
                playhead_frames: 0.0,
            });
        } else {
            // ALSA capture is idle or not running; submit directly to broadcast and replay routes
            let chunk_size = 1920 * 2; // 40ms of 48 kHz stereo
            let mut offset = 0;
            let mut at = now;
            while offset < samples.len() {
                let end = (offset + chunk_size).min(samples.len());
                let chunk = &samples[offset..end];
                submit_to_route(&self.route, &self.discontinuity, chunk, 48000, 2, at);
                submit_to_route(
                    &self.broadcast_route,
                    &self.broadcast_discontinuity,
                    chunk,
                    48000,
                    2,
                    at,
                );
                at += (chunk.len() as i64 / 2) * 1_000_000 / 48000;
                offset = end;
            }
        }
    }
    pub fn submit(&self, samples: &[f32], rate: u32, channels: u16, at: i64) {
        self.last_game_submit.store(now_us(), Ordering::Relaxed);
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }

        let mixed_buf;
        let effective_samples = {
            let mut alert_guard = self.active_alert.lock().unwrap();
            if let Some(active) = alert_guard.as_mut() {
                let src_frames = active.samples.len() / 2;
                if src_frames == 0 || rate == 0 || channels == 0 {
                    *alert_guard = None;
                    samples
                } else {
                    let mut buf = samples.to_vec();
                    let frames = buf.len() / channels as usize;
                    let step = 48000.0 / rate as f64;
                    let mut finished = false;

                    for k in 0..frames {
                        if active.playhead_frames >= src_frames as f64 {
                            finished = true;
                            break;
                        }
                        let pos = active.playhead_frames;
                        let idx = pos.floor() as usize;
                        let frac = (pos - idx as f64) as f32;

                        let idx0 = idx.saturating_sub(1);
                        let idx1 = idx.min(src_frames - 1);
                        let idx2 = (idx + 1).min(src_frames - 1);
                        let idx3 = (idx + 2).min(src_frames - 1);

                        // High-fidelity Catmull-Rom cubic interpolation per channel
                        let mut ch_samples = [0.0f32; 2];
                        for c in 0..2 {
                            let p0 = active.samples[idx0 * 2 + c];
                            let p1 = active.samples[idx1 * 2 + c];
                            let p2 = active.samples[idx2 * 2 + c];
                            let p3 = active.samples[idx3 * 2 + c];

                            let c0 = p1;
                            let c1 = 0.5 * (p2 - p0);
                            let c2 = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
                            let c3 = 0.5 * (p3 - p0) + 1.5 * (p1 - p2);

                            ch_samples[c] = ((c3 * frac + c2) * frac + c1) * frac + c0;
                        }

                        if channels == 1 {
                            let mono_mix = (ch_samples[0] + ch_samples[1]) * 0.5;
                            buf[k] = (buf[k] + mono_mix).clamp(-1.0, 1.0);
                        } else {
                            let base = k * channels as usize;
                            buf[base] = (buf[base] + ch_samples[0]).clamp(-1.0, 1.0);
                            buf[base + 1] = (buf[base + 1] + ch_samples[1]).clamp(-1.0, 1.0);
                        }

                        active.playhead_frames += step;
                    }

                    if finished || active.playhead_frames >= src_frames as f64 {
                        *alert_guard = None;
                    }
                    mixed_buf = buf;
                    &mixed_buf[..]
                }
            } else {
                samples
            }
        };

        submit_to_route(&self.route, &self.discontinuity, effective_samples, rate, channels, at);
        if !self.broadcast_preroll_active.load(Ordering::Relaxed) {
            submit_to_route(&self.broadcast_route, &self.broadcast_discontinuity, effective_samples, rate, channels, at);
        }
    }
    pub fn submit_mic(&self, samples: &[f32], rate: u32, channels: u16, at: i64) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        submit_to_route(&self.mic_route, &self.mic_discontinuity, samples, rate, channels, at);
        if !self.broadcast_preroll_active.load(Ordering::Relaxed) {
            submit_to_route(&self.broadcast_mic_route, &self.broadcast_mic_discontinuity, samples, rate, channels, at);
        }
    }
    /// Submits decoded pre-roll audio directly into the live broadcast route.
    pub fn submit_preroll_audio(&self, samples: &[f32], rate: u32, channels: u16, at: i64) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        submit_to_route(
            &self.broadcast_route,
            &self.broadcast_discontinuity,
            samples,
            rate,
            channels,
            at,
        );
    }
    pub fn set_broadcast_preroll_active(&self, active: bool) {
        self.broadcast_preroll_active.store(active, Ordering::SeqCst);
        if !active {
            self.broadcast_discontinuity.store(true, Ordering::Relaxed);
            self.broadcast_mic_discontinuity.store(true, Ordering::Relaxed);
        }
    }
    #[allow(dead_code)]
    pub fn is_broadcast_preroll_active(&self) -> bool {
        self.broadcast_preroll_active.load(Ordering::Relaxed)
    }
    fn disable(&self) {
        *self.route.lock().unwrap() = None;
        *self.mic_route.lock().unwrap() = None;
        self.refresh_enabled();
    }
    pub fn register_broadcast_routes(&self, game: AudioRoute, mic: AudioRoute) {
        *self.broadcast_route.lock().unwrap() = Some(game);
        *self.broadcast_mic_route.lock().unwrap() = Some(mic);
        self.refresh_enabled();
    }
    pub fn unregister_broadcast_routes(&self) {
        *self.broadcast_route.lock().unwrap() = None;
        *self.broadcast_mic_route.lock().unwrap() = None;
        self.refresh_enabled();
    }
    fn refresh_enabled(&self) {
        let replay_on = self.route.lock().unwrap().is_some();
        let broadcast_on = self.broadcast_route.lock().unwrap().is_some();
        self.enabled.store(replay_on || broadcast_on, Ordering::Release);
    }
}
#[derive(Default, Clone)]
pub struct Status {
    pub message: String,
    pub resets: u64,
    pub last_reset: String,
    pub history_exhaustions: u64,
    pub last_exhaustion: String,
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
    pub conversion_ms: f64,
    pub hardware_ms: f64,
    pub frame_interval_ms: f64,
    pub conversion_threads: usize,
    pub recent_video_drops: u64,
    pub saving: bool,
    pub clipboard_bytes: usize,
}
pub struct Shared {
    clipboard: Arc<clipboard::Clipboard>,
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
    pub game_volume: Arc<AtomicU32>,
}
impl Shared {
    /// Worker only: keep the reason visible after ordinary progress messages.
    fn record_reset(&self, reason: String) {
        tracing::warn!(%reason, "Replay history reset");
        let mut status = self.status.lock().unwrap();
        status.resets += 1;
        status.last_reset = reason;
    }
    fn message(&self, text: impl Into<String>) {
        if let Ok(mut status) = self.status.try_lock() {
            status.message = text.into();
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Destination {
    File,
    Clipboard,
    /// Audio only, transcoded to MP3.
    AudioClipboard,
}
pub struct Save {
    destination: Destination,
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
    clipboard: Arc<clipboard::Clipboard>,
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
    pub fn clipboard(&self) -> Arc<clipboard::Clipboard> {
        self.clipboard.clone()
    }

    pub fn sync_audio_filter(&self) {
        self.audio.set_filter_config(self.config.audio_filter.clone());
    }

    pub fn set_game_volume(&self, volume: f32) {
        self.audio.set_game_volume(volume);
    }

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
            clipboard: self.clipboard.clone(),
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
            game_volume: self.audio.game_volume.clone(),
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
        let (mic_atx, mic_arx) = bounded(AUDIO_BLOCKS);
        let (mic_free_tx, mic_free_rx) = bounded(AUDIO_BLOCKS);
        for _ in 0..AUDIO_BLOCKS {
            mic_free_tx.send(AudioBlock {
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
                    worker::run(
                        config,
                        &worker_queue,
                        arx,
                        free_tx,
                        mic_arx,
                        mic_free_tx,
                        saves,
                        &worker_shared,
                    )
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
        *self.audio.mic_route.lock().unwrap() = Some(AudioRoute {
            ready: mic_atx,
            free: mic_free_rx,
            stop: shared.stop.clone(),
            dropped: shared.audio_dropped.clone(),
            partial: None,
            next_continuous: false,
            format: None,
        });
        self.audio.refresh_enabled();
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
        self.request_export(seconds, Destination::File)
    }
    fn request_export(&mut self, seconds: u32, destination: Destination) -> anyhow::Result<()> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Enable replay first"))?;
        anyhow::ensure!(
            !runtime.shared.stop.load(Ordering::Acquire),
            "Replay has stopped; see its status"
        );
        let mut status = runtime.shared.status.lock().unwrap();
        anyhow::ensure!(!status.saving, "A replay is already being saved");
        runtime
            .save
            .try_send(Save {
                destination,
                seconds: seconds.min(self.config.history_seconds),
                at: now_us(),
            })
            .map_err(|_| anyhow::anyhow!("A replay save is already pending"))?;
        status.message = "Preparing replay export…".into();
        // Identical-size clipboard copies still need a fresh completion notice.
        self.last_notice = status.message.clone();
        Ok(())
    }
    pub fn notification(&mut self) -> Option<String> {
        let message = self.status().message;
        if message != self.last_notice
            && (message.starts_with("Copied ")
                || message.starts_with("Replay copy failed:")
                || message.starts_with("Audio copy failed:")
                || message.starts_with("Saved ")
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
        let mut status = self
            .runtime
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
            .unwrap_or_else(|| self.last_status.clone());
        status.clipboard_bytes = self.clipboard.bytes();
        status
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
        if let Some(destination) = clipboard_shortcut(ctx) {
            let (seconds, prefix) = match destination {
                Destination::AudioClipboard => (AUDIO_CLIP_SECONDS, "Audio copy failed"),
                _ => (VIDEO_CLIP_SECONDS, "Replay copy failed"),
            };
            if let Err(error) = self.request_export(seconds, destination) {
                let message = format!("{prefix}: {error}");
                self.last_status.message = message.clone();
                if let Some(runtime) = &self.runtime {
                    runtime.shared.message(message);
                }
            }
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
        self.clipboard.close();
    }
}

/// Length of the Ctrl+C video clip.
const VIDEO_CLIP_SECONDS: u32 = 15;
/// Length of the Ctrl+Shift+C audio-only clip.
const AUDIO_CLIP_SECONDS: u32 = 7;

// egui-winit translates both Ctrl+C and Ctrl+Shift+C to Event::Copy rather than
// a C key event; the held Shift modifier selects the audio-only export.
fn clipboard_shortcut(ctx: &eframe::egui::Context) -> Option<Destination> {
    use eframe::egui::{Event, Key, ViewportId};
    if ctx.viewport_id() != ViewportId::ROOT || ctx.wants_keyboard_input() {
        return None;
    }
    ctx.input_mut(|input| {
        if !input.focused {
            return None;
        }
        let held_shift = input.modifiers.shift;
        let mut found = None;
        input.events.retain(|event| {
            let destination = match event {
                Event::Copy => Some(held_shift),
                Event::Key {
                    key: Key::C,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if modifiers.ctrl && !modifiers.alt && !modifiers.mac_cmd => {
                    Some(modifiers.shift)
                }
                _ => None,
            }
            .map(|audio| {
                if audio {
                    Destination::AudioClipboard
                } else {
                    Destination::Clipboard
                }
            });
            if destination.is_some() {
                found = found.or(destination);
            }
            destination.is_none()
        });
        found
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_copy_is_only_consumed_in_focused_video_without_text_editing() {
        use eframe::egui::{self, Event, Key, ViewportId};
        for (viewport, focused, editing, expected) in [
            (ViewportId::ROOT, true, false, true),
            (ViewportId::ROOT, false, false, false),
            (ViewportId::ROOT, true, true, false),
            (
                ViewportId::from_hash_of("control_window"),
                true,
                false,
                false,
            ),
        ] {
            for shift in [false, true] {
                let modifiers = if shift {
                    egui::Modifiers::CTRL | egui::Modifiers::SHIFT
                } else {
                    egui::Modifiers::CTRL
                };
                let wanted = expected.then_some(if shift {
                    Destination::AudioClipboard
                } else {
                    Destination::Clipboard
                });
                for event in [
                    Event::Copy,
                    Event::Key {
                        key: Key::C,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ] {
                    let ctx = egui::Context::default();
                    let mut input = egui::RawInput {
                        viewport_id: viewport,
                        focused,
                        modifiers,
                        events: vec![event],
                        ..Default::default()
                    };
                    input.viewports.entry(viewport).or_default();
                    let _ = ctx.run(input, |ctx| {
                        if editing {
                            ctx.memory_mut(|m| m.request_focus(egui::Id::new("text")));
                        }
                        assert_eq!(clipboard_shortcut(ctx), wanted);
                        assert!(
                            clipboard_shortcut(ctx).is_none(),
                            "Consumed copy must not be exported twice"
                        );
                        assert_eq!(ctx.input(|i| i.events.len()), usize::from(!expected));
                    });
                }
            }
        }
        // Plain C still belongs to the CRT filter, not the clipboard.
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![Event::Key {
                key: Key::C,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| assert!(clipboard_shortcut(ctx).is_none()));
    }
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
        let legacy: ReplayConfig = serde_json::from_str(
            r#"{"codec":"Av1","quality":20,"rate_control":"Bitrate","max_bitrate_mbps":40,"memory_mib":6000}"#,
        )
        .unwrap();
        assert_eq!(legacy.memory_mib, 6000);
        assert_eq!(legacy.history_seconds, 300);
        let mut updated = old;
        updated.replay.history_seconds = 600;
        updated.replay.keys = [1, 2, 3, 4, 5, 6];
        let json = serde_json::to_string(&updated).unwrap();
        let loaded: crate::config::MichadameConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.replay.history_seconds, 600);
        assert_eq!(loaded.replay.keys, [1, 2, 3, 4, 5, 6]);
        assert!(Replay::default().runtime.is_none());
    }

    #[test]
    fn test_audio_tap_play_alert_idle_submits_blocks() {
        let (tap, rx, free, _) = test_tap(4);
        let chime = vec![0.3f32; 3840]; // 1 full 40ms block of stereo
        tap.play_alert(&chime);

        let block = rx.try_recv().expect("Direct alert submission should send block when idle");
        assert_eq!(block.len, 3840);
        assert!((block.samples[0] - 0.3).abs() < 1e-4);
        free.send(block).unwrap();
    }

    #[test]
    fn test_audio_tap_play_alert_active_mixes_samples() {
        let (tap, rx, free, _) = test_tap(4);
        // Simulate active ALSA submission
        tap.submit(&[0.2f32; 3840], 48000, 2, now_us());
        let _ = rx.try_recv();

        // Queue alert while actively flowing
        let chime = vec![0.3f32; 3840];
        tap.play_alert(&chime);

        // Next submit should mix incoming game audio (0.2) + chime (0.3) = 0.5
        tap.submit(&[0.2f32; 3840], 48000, 2, now_us());
        let mixed = rx.try_recv().expect("Should receive mixed block");
        assert_eq!(mixed.len, 3840);
        assert!((mixed.samples[0] - 0.5).abs() < 1e-4);
        free.send(mixed).unwrap();
    }

    #[test]
    fn test_audio_tap_play_alert_active_mixes_44100_preserving_pitch_and_timing() {
        let (tap, rx, free, _) = test_tap(4);
        // Simulate active 44.1 kHz ALSA capture
        tap.submit(&[0.1f32; 3528], 44100, 2, now_us());
        let _ = rx.try_recv();

        // 48 kHz stereo constant chime (0.4)
        let chime = vec![0.4f32; 4800];
        tap.play_alert(&chime);

        // Submit next 44.1 kHz block: should resample and mix 0.1 + 0.4 = 0.5
        tap.submit(&[0.1f32; 3528], 44100, 2, now_us());
        let mixed = rx.try_recv().expect("Should receive mixed block at 44100");
        assert_eq!(mixed.rate, 44100);
        assert_eq!(mixed.channels, 2);
        assert!((mixed.samples[0] - 0.5).abs() < 1e-3);
        free.send(mixed).unwrap();
    }

    #[test]
    fn test_audio_tap_play_alert_mono_downmix() {
        let (tap, rx, free, _) = test_tap(4);
        // Simulate active mono capture
        tap.submit(&[0.1f32; 1920], 48000, 1, now_us());
        let _ = rx.try_recv();

        // Stereo chime: L=0.6, R=0.2 -> Mono average = 0.4
        let mut chime = Vec::with_capacity(3840);
        for _ in 0..1920 {
            chime.push(0.6f32);
            chime.push(0.2f32);
        }
        tap.play_alert(&chime);

        tap.submit(&[0.1f32; 1920], 48000, 1, now_us());
        let mixed = rx.try_recv().expect("Should receive mixed mono block");
        assert_eq!(mixed.rate, 48000);
        assert_eq!(mixed.channels, 1);
        // 0.1 game + 0.4 average chime = 0.5
        assert!((mixed.samples[0] - 0.5).abs() < 1e-3);
        free.send(mixed).unwrap();
    }
}
