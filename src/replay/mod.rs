pub mod clipboard;
pub mod config;
mod encoder;
pub mod gpu;
mod queue;
mod ring;
pub mod ui;
mod worker;

pub(crate) use worker::{AudioClock, MixSource, TimelineMixer};

use config::ReplayConfig;
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

enum FrameRecycler {
    Replay {
        free_tx: Sender<Vec<u8>>,
        pool_generation: Arc<AtomicU64>,
        generation: u64,
    },
    Broadcast {
        free_tx: Sender<Vec<u8>>,
    },
}

struct SharedFrameInner {
    data: Vec<u8>,
    recycler: Option<FrameRecycler>,
}

impl Drop for SharedFrameInner {
    fn drop(&mut self) {
        let data = std::mem::take(&mut self.data);
        if data.capacity() == 0 {
            return;
        }
        match &self.recycler {
            Some(FrameRecycler::Replay {
                free_tx,
                pool_generation,
                generation,
            }) => {
                if pool_generation.load(Ordering::Acquire) == *generation {
                    let _ = free_tx.try_send(data);
                }
            }
            Some(FrameRecycler::Broadcast { free_tx }) => {
                let _ = free_tx.try_send(data);
            }
            None => {}
        }
    }
}

/// Reference-counted RGBA capture buffer shared zero-copy across Replay and Broadcast
/// consumers, returning its backing `Vec<u8>` to the originating pool on the final drop.
#[derive(Clone)]
pub struct SharedFrameBytes(Arc<SharedFrameInner>);

impl SharedFrameBytes {
    #[allow(dead_code)]
    pub fn from_vec(data: Vec<u8>) -> Self {
        Self(Arc::new(SharedFrameInner {
            data,
            recycler: None,
        }))
    }

    pub(crate) fn from_replay_pool(
        data: Vec<u8>,
        free_tx: Sender<Vec<u8>>,
        pool_generation: Arc<AtomicU64>,
        generation: u64,
    ) -> Self {
        Self(Arc::new(SharedFrameInner {
            data,
            recycler: Some(FrameRecycler::Replay {
                free_tx,
                pool_generation,
                generation,
            }),
        }))
    }

    pub(crate) fn from_broadcast_pool(data: Vec<u8>, free_tx: Sender<Vec<u8>>) -> Self {
        Self(Arc::new(SharedFrameInner {
            data,
            recycler: Some(FrameRecycler::Broadcast { free_tx }),
        }))
    }

    #[allow(dead_code)]
    pub fn strong_count(&self) -> usize {
        Arc::strong_count(&self.0)
    }
}

impl std::ops::Deref for SharedFrameBytes {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0.data
    }
}

impl AsRef<[u8]> for SharedFrameBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0.data
    }
}

impl std::fmt::Debug for SharedFrameBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedFrameBytes")
            .field("len", &self.0.data.len())
            .field("strong_count", &Arc::strong_count(&self.0))
            .finish()
    }
}

impl PartialEq<[u8]> for SharedFrameBytes {
    fn eq(&self, other: &[u8]) -> bool {
        self.0.data.as_slice() == other
    }
}

impl PartialEq<Vec<u8>> for SharedFrameBytes {
    fn eq(&self, other: &Vec<u8>) -> bool {
        self.0.data == *other
    }
}

impl PartialEq for SharedFrameBytes {
    fn eq(&self, other: &Self) -> bool {
        self.0.data == other.0.data
    }
}

impl Eq for SharedFrameBytes {}

pub struct VideoFrame {
    pub rgba: SharedFrameBytes,
    pub width: u32,
    pub height: u32,
    pub at: i64,
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
fn mix_alert_in_place(
    dst: &mut [f32],
    alert_samples: &[f32],
    start_playhead: f64,
    frame_offset: usize,
    rate: u32,
    channels: u16,
) {
    let src_frames = alert_samples.len() / 2;
    let channels_usize = channels as usize;
    if src_frames == 0 || rate == 0 || channels_usize == 0 {
        return;
    }
    let frames = dst.len() / channels_usize;
    let step = 48000.0 / rate as f64;

    for k in 0..frames {
        let pos = start_playhead + ((frame_offset + k) as f64) * step;
        if pos >= src_frames as f64 {
            break;
        }
        let idx = pos.floor() as usize;
        let frac = (pos - idx as f64) as f32;

        let idx0 = idx.saturating_sub(1);
        let idx1 = idx.min(src_frames - 1);
        let idx2 = (idx + 1).min(src_frames - 1);
        let idx3 = (idx + 2).min(src_frames - 1);

        // High-fidelity Catmull-Rom cubic interpolation per channel
        let mut ch_samples = [0.0f32; 2];
        for c in 0..2 {
            let p0 = alert_samples[idx0 * 2 + c];
            let p1 = alert_samples[idx1 * 2 + c];
            let p2 = alert_samples[idx2 * 2 + c];
            let p3 = alert_samples[idx3 * 2 + c];

            let c0 = p1;
            let c1 = 0.5 * (p2 - p0);
            let c2 = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
            let c3 = 0.5 * (p3 - p0) + 1.5 * (p1 - p2);

            ch_samples[c] = ((c3 * frac + c2) * frac + c1) * frac + c0;
        }

        if channels == 1 {
            let mono_mix = (ch_samples[0] + ch_samples[1]) * 0.5;
            dst[k] = (dst[k] + mono_mix).clamp(-1.0, 1.0);
        } else {
            let base = k * channels_usize;
            dst[base] = (dst[base] + ch_samples[0]).clamp(-1.0, 1.0);
            dst[base + 1] = (dst[base + 1] + ch_samples[1]).clamp(-1.0, 1.0);
        }
    }
}

fn submit_to_route(
    route_mutex: &Mutex<Option<AudioRoute>>,
    discontinuity: &AtomicBool,
    samples: &[f32],
    rate: u32,
    channels: u16,
    at: i64,
    alert_mix: Option<(&[f32], f64)>,
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
        let dst_slice = &mut block.samples[block.len..block.len + count];
        dst_slice.copy_from_slice(&samples[offset..offset + count]);
        if let Some((alert_samples, start_playhead)) = alert_mix {
            mix_alert_in_place(
                dst_slice,
                alert_samples,
                start_playhead,
                offset / channels_usize,
                rate,
                channels,
            );
        }
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
    samples: Arc<[f32]>,
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
    has_active_alert: AtomicBool,
    active_alert: Mutex<Option<ActiveAlertAudio>>,
    last_game_submit: AtomicI64,
    reactive_listening: AtomicBool,
    reactive_lp180_bits: AtomicU32,
    reactive_lp2400_bits: AtomicU32,
    reactive_low_bits: AtomicU32,
    reactive_mid_bits: AtomicU32,
    reactive_high_bits: AtomicU32,
    reactive_left_bits: AtomicU32,
    reactive_right_bits: AtomicU32,
    reactive_last_us: AtomicI64,
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
            has_active_alert: AtomicBool::new(false),
            active_alert: Mutex::new(None),
            last_game_submit: AtomicI64::new(0),
            reactive_listening: AtomicBool::new(false),
            reactive_lp180_bits: AtomicU32::new(0.0f32.to_bits()),
            reactive_lp2400_bits: AtomicU32::new(0.0f32.to_bits()),
            reactive_low_bits: AtomicU32::new(0.0f32.to_bits()),
            reactive_mid_bits: AtomicU32::new(0.0f32.to_bits()),
            reactive_high_bits: AtomicU32::new(0.0f32.to_bits()),
            reactive_left_bits: AtomicU32::new(0.0f32.to_bits()),
            reactive_right_bits: AtomicU32::new(0.0f32.to_bits()),
            reactive_last_us: AtomicI64::new(0),
        }
    }
}

impl AudioTap {
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    pub fn set_reactive_listening(&self, active: bool) {
        self.reactive_listening.store(active, Ordering::Relaxed);
    }
    pub fn is_reactive_listening(&self) -> bool {
        self.reactive_listening.load(Ordering::Relaxed)
    }
    /// Lock-free 3-band + stereo energy observer for Audio-Reactive CRT Interference.
    pub fn observe_reactive(&self, samples: &[f32], rate: u32, channels: u16) {
        if !self.reactive_listening.load(Ordering::Relaxed)
            || samples.is_empty()
            || rate == 0
            || channels == 0
        {
            return;
        }
        let ch = channels as usize;
        let frames = samples.len() / ch;
        if frames == 0 {
            return;
        }

        let dt = 1.0 / (rate.max(8000) as f32);
        let a180 = dt / (1.0 / (std::f32::consts::TAU * 180.0) + dt);
        let a2400 = dt / (1.0 / (std::f32::consts::TAU * 2400.0) + dt);

        let mut lp180 = f32::from_bits(self.reactive_lp180_bits.load(Ordering::Relaxed));
        let mut lp2400 = f32::from_bits(self.reactive_lp2400_bits.load(Ordering::Relaxed));
        if !lp180.is_finite() {
            lp180 = 0.0;
        }
        if !lp2400.is_finite() {
            lp2400 = 0.0;
        }

        let mut sum_low_sq = 0.0f32;
        let mut sum_mid_sq = 0.0f32;
        let mut sum_high_sq = 0.0f32;
        let mut sum_l_sq = 0.0f32;
        let mut sum_r_sq = 0.0f32;

        let mut peak_low = 0.0f32;
        let mut peak_mid = 0.0f32;
        let mut peak_high = 0.0f32;

        for frame in samples.chunks_exact(ch) {
            let l = frame[0].clamp(-2.0, 2.0);
            let r = if ch >= 2 {
                frame[1].clamp(-2.0, 2.0)
            } else {
                l
            };
            let mono = (l + r) * 0.5;

            lp180 += a180 * (mono - lp180);
            lp2400 += a2400 * (mono - lp2400);

            let low = lp180;
            let mid = lp2400 - lp180;
            let high = mono - lp2400;

            sum_low_sq += low * low;
            sum_mid_sq += mid * mid;
            sum_high_sq += high * high;
            sum_l_sq += l * l;
            sum_r_sq += r * r;

            peak_low = peak_low.max(low.abs());
            peak_mid = peak_mid.max(mid.abs());
            peak_high = peak_high.max(high.abs());
        }

        self.reactive_lp180_bits
            .store(lp180.to_bits(), Ordering::Relaxed);
        self.reactive_lp2400_bits
            .store(lp2400.to_bits(), Ordering::Relaxed);

        let inv_n = 1.0 / (frames as f32);
        // Modern game audio typically sits around -24 dBFS to -12 dBFS (linear RMS 0.06..0.25).
        // Combine RMS with peak response and a gentle perceptual curve (x^0.62) so normal
        // gameplay audio drives the CRT reactive bands cleanly without requiring clipping levels.
        let perceptual = |v: f32| -> f32 {
            let clamped = v.max(0.0);
            if clamped <= 0.0001 {
                0.0
            } else {
                clamped.powf(0.62)
            }
        };
        let low_raw = ((sum_low_sq * inv_n).sqrt() * 2.40).max(peak_low * 1.15);
        let mid_raw = ((sum_mid_sq * inv_n).sqrt() * 2.85).max(peak_mid * 1.25);
        let high_raw = ((sum_high_sq * inv_n).sqrt() * 3.60).max(peak_high * 1.45);
        let low_val = perceptual(low_raw) * 1.25;
        let mid_val = perceptual(mid_raw) * 1.25;
        let high_val = perceptual(high_raw) * 1.30;
        let left_val = perceptual((sum_l_sq * inv_n).sqrt() * 2.2);
        let right_val = perceptual((sum_r_sq * inv_n).sqrt() * 2.2);

        let blend_in = |atom: &AtomicU32, new_v: f32| {
            let prev = f32::from_bits(atom.load(Ordering::Relaxed));
            let prev_clean = if prev.is_finite() { prev } else { 0.0 };
            let merged = new_v.max(prev_clean * 0.90).clamp(0.0, 4.0);
            atom.store(merged.to_bits(), Ordering::Relaxed);
        };

        blend_in(&self.reactive_low_bits, low_val);
        blend_in(&self.reactive_mid_bits, mid_val);
        blend_in(&self.reactive_high_bits, high_val);
        blend_in(&self.reactive_left_bits, left_val);
        blend_in(&self.reactive_right_bits, right_val);
        self.reactive_last_us.store(now_us(), Ordering::Relaxed);
    }

    /// Reads the latest 3-band + stereo energy snapshot for the UI/shader loop.
    pub fn take_reactive_raw(&self) -> (f32, f32, f32, f32, f32) {
        let last_us = self.reactive_last_us.load(Ordering::Relaxed);
        if last_us == 0 || (now_us() - last_us) > 120_000 {
            return (0.0, 0.0, 0.0, 0.0, 0.0);
        }
        let read_and_decay = |atom: &AtomicU32| -> f32 {
            let v = f32::from_bits(atom.load(Ordering::Relaxed));
            let clean = if v.is_finite() {
                v.clamp(0.0, 4.0)
            } else {
                0.0
            };
            atom.store((clean * 0.88).to_bits(), Ordering::Relaxed);
            clean
        };
        (
            read_and_decay(&self.reactive_low_bits),
            read_and_decay(&self.reactive_mid_bits),
            read_and_decay(&self.reactive_high_bits),
            read_and_decay(&self.reactive_left_bits),
            read_and_decay(&self.reactive_right_bits),
        )
    }
    pub fn set_filter_config(&self, config: crate::replay::config::AudioFilterConfig) {
        if let Ok(mut w) = self.filter.write() {
            *w = config;
        }
    }
    pub fn set_game_volume(&self, volume: f32) {
        self.game_volume.store(volume.to_bits(), Ordering::Relaxed);
    }
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
        self.broadcast_mic_discontinuity
            .store(true, Ordering::Relaxed);
    }
    /// Plays an alert chime into the stream broadcast and replay routes.
    ///
    /// If ALSA game audio is actively flowing, the chime samples are queued and resampled
    /// on-the-fly to match the ALSA capture rate (e.g. 44.1 kHz, 48 kHz) and channel layout.
    /// If ALSA audio is idle/stopped, the chime is directly submitted as 48 kHz stereo audio blocks.
    pub fn play_alert(&self, samples: &[f32]) {
        if !self.is_enabled() || samples.len() < 2 {
            return;
        }
        self.play_alert_arc(Arc::from(samples));
    }

    /// Zero-copy variant of [`AudioTap::play_alert`] accepting shared `Arc<[f32]>` alert samples.
    pub fn play_alert_arc(&self, samples: Arc<[f32]>) {
        if !self.is_enabled() || samples.len() < 2 {
            return;
        }
        let now = now_us();
        let last_submit = self.last_game_submit.load(Ordering::Relaxed);

        if last_submit > 0 && (now - last_submit) <= 150_000 {
            // ALSA capture is actively running; queue samples to be resampled and mixed in submit()
            let mut guard = self.active_alert.lock().unwrap();
            *guard = Some(ActiveAlertAudio {
                samples,
                playhead_frames: 0.0,
            });
            self.has_active_alert.store(true, Ordering::Release);
        } else {
            {
                let mut guard = self.active_alert.lock().unwrap();
                *guard = None;
                self.has_active_alert.store(false, Ordering::Release);
            }
            // ALSA capture is idle or not running; submit directly to broadcast and replay routes
            let chunk_size = 1920 * 2; // 40ms of 48 kHz stereo
            let mut offset = 0;
            let mut at = now;
            while offset < samples.len() {
                let end = (offset + chunk_size).min(samples.len());
                let chunk = &samples[offset..end];
                submit_to_route(&self.route, &self.discontinuity, chunk, 48000, 2, at, None);
                if !self.broadcast_preroll_active.load(Ordering::Relaxed) {
                    submit_to_route(
                        &self.broadcast_route,
                        &self.broadcast_discontinuity,
                        chunk,
                        48000,
                        2,
                        at,
                        None,
                    );
                }
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

        let alert_mix: Option<(Arc<[f32]>, f64)> = if self.has_active_alert.load(Ordering::Acquire)
        {
            let mut alert_guard = self.active_alert.lock().unwrap();
            if let Some(active) = alert_guard.as_mut() {
                let src_frames = active.samples.len() / 2;
                if src_frames == 0 || rate == 0 || channels == 0 {
                    *alert_guard = None;
                    self.has_active_alert.store(false, Ordering::Release);
                    None
                } else {
                    let start_playhead = active.playhead_frames;
                    let frames = samples.len() / channels as usize;
                    let step = 48000.0 / rate as f64;
                    let next_playhead = start_playhead + (frames as f64) * step;
                    let arc_samples = Arc::clone(&active.samples);
                    if next_playhead >= src_frames as f64 {
                        *alert_guard = None;
                        self.has_active_alert.store(false, Ordering::Release);
                    } else {
                        active.playhead_frames = next_playhead;
                    }
                    Some((arc_samples, start_playhead))
                }
            } else {
                self.has_active_alert.store(false, Ordering::Release);
                None
            }
        } else {
            None
        };
        let alert_slice = alert_mix.as_ref().map(|(s, p)| (s.as_ref(), *p));

        submit_to_route(
            &self.route,
            &self.discontinuity,
            samples,
            rate,
            channels,
            at,
            alert_slice,
        );
        if !self.broadcast_preroll_active.load(Ordering::Relaxed) {
            submit_to_route(
                &self.broadcast_route,
                &self.broadcast_discontinuity,
                samples,
                rate,
                channels,
                at,
                alert_slice,
            );
        }
    }
    pub fn submit_mic(&self, samples: &[f32], rate: u32, channels: u16, at: i64) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        submit_to_route(
            &self.mic_route,
            &self.mic_discontinuity,
            samples,
            rate,
            channels,
            at,
            None,
        );
        if !self.broadcast_preroll_active.load(Ordering::Relaxed) {
            submit_to_route(
                &self.broadcast_mic_route,
                &self.broadcast_mic_discontinuity,
                samples,
                rate,
                channels,
                at,
                None,
            );
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
            None,
        );
    }
    pub fn set_broadcast_preroll_active(&self, active: bool) {
        self.broadcast_preroll_active
            .store(active, Ordering::SeqCst);
        self.broadcast_discontinuity.store(true, Ordering::Relaxed);
        self.broadcast_mic_discontinuity
            .store(true, Ordering::Relaxed);
    }
    pub fn is_broadcast_preroll_active(&self) -> bool {
        self.broadcast_preroll_active.load(Ordering::Relaxed)
    }
    fn disable(&self) {
        *self.route.lock().unwrap() = None;
        *self.mic_route.lock().unwrap() = None;
        self.refresh_enabled();
    }
    pub(crate) fn register_broadcast_routes(&self, game: AudioRoute, mic: AudioRoute) {
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
        let any_on = replay_on || broadcast_on;
        self.enabled.store(any_on, Ordering::Release);
        if !any_on {
            let mut guard = self.active_alert.lock().unwrap();
            *guard = None;
            self.has_active_alert.store(false, Ordering::Release);
        }
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
pub struct ScrubSnapshot {
    pub packets: Vec<Arc<ring::Encoded>>,
    pub video: ffmpeg_next::codec::Parameters,
    pub audio: ffmpeg_next::codec::Parameters,
    pub directory: String,
    pub preview_file: Arc<tempfile::NamedTempFile>,
    pub duration_secs: f64,
    pub width: u32,
    pub height: u32,
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
    pub scrub_snapshot: Mutex<Option<Result<ScrubSnapshot, String>>>,
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
    /// Snapshot for the Replay Buffer Scrub Player window (F5).
    ScrubPreview,
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
    pub scrub_open_requested: bool,
}
impl Replay {
    pub fn clipboard(&self) -> Arc<clipboard::Clipboard> {
        self.clipboard.clone()
    }

    pub fn sync_audio_filter(&self) {
        self.audio
            .set_filter_config(self.config.audio_filter.clone());
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
            scrub_snapshot: Mutex::new(None),
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
        self.available_now = Some(available);
        self.memory_checked = Some(std::time::Instant::now());
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
    pub fn request_scrub_preview(&mut self) -> bool {
        if self.status().seconds < MIN_SCRUB_DURATION_SECS {
            return false;
        }
        let Some(runtime) = &self.runtime else {
            return false;
        };
        if runtime.shared.stop.load(Ordering::Acquire) {
            return false;
        }
        if runtime.shared.status.lock().unwrap().saving {
            return false;
        }
        *runtime.shared.scrub_snapshot.lock().unwrap() = None;
        runtime
            .save
            .try_send(Save {
                destination: Destination::ScrubPreview,
                seconds: self.config.history_seconds,
                at: now_us(),
            })
            .is_ok()
    }
    pub fn take_scrub_snapshot(&mut self) -> Option<Result<ScrubSnapshot, String>> {
        self.runtime
            .as_ref()
            .and_then(|r| r.shared.scrub_snapshot.lock().unwrap().take())
    }
    pub fn save_trimmed(
        &mut self,
        snapshot: ScrubSnapshot,
        start_secs: f64,
        end_secs: f64,
    ) -> anyhow::Result<()> {
        let trimmed =
            ring::trim_packets(&snapshot.packets, start_secs, end_secs).ok_or_else(|| {
                anyhow::anyhow!("Not enough decodable video and audio in selected range")
            })?;
        let duration =
            (trimmed.iter().map(|p| p.end).max().unwrap() - trimmed[0].start) as f64 / 1e6;
        let directory = if self.config.directory.trim().is_empty() {
            snapshot.directory
        } else {
            self.config.directory.clone()
        };
        let video = snapshot.video;
        let audio = snapshot.audio;
        let export_shared = self.runtime.as_ref().map(|r| r.shared.clone());
        if let Some(ref shared) = export_shared {
            let mut status = shared.status.lock().unwrap();
            status.saving = true;
            status.message = "Saving trimmed replay clip…".into();
            self.last_notice = status.message.clone();
        }
        std::thread::Builder::new()
            .name("replay-trimmed-save".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker::save_file(&directory, video, audio, trimmed)
                        .map(|path| format!("Saved {duration:.1}s: {}", path.display()))
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Replay writer panicked")));
                if let Some(shared) = export_shared {
                    let mut status = shared.status.lock().unwrap();
                    status.saving = false;
                    status.message = match result {
                        Ok(message) => message,
                        Err(e) => format!("Replay save failed: {e:#}"),
                    };
                }
            })?;
        Ok(())
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
        if (self.runtime.is_some() || self.audio.is_enabled())
            && self
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
        if ctx.input_mut(|i| i.consume_key(eframe::egui::Modifiers::NONE, eframe::egui::Key::F5)) {
            if self.status().seconds >= MIN_SCRUB_DURATION_SECS {
                self.scrub_open_requested = true;
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

/// Minimum buffered replay duration (in seconds) required to open the scrub player (F5).
pub const MIN_SCRUB_DURATION_SECS: f64 = 5.0;
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
        let json = serde_json::to_string(&updated).unwrap();
        let loaded: crate::config::MichadameConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.replay.history_seconds, 600);
        assert!(Replay::default().runtime.is_none());
    }

    #[test]
    fn test_audio_tap_play_alert_idle_submits_blocks() {
        let (tap, rx, free, _) = test_tap(4);
        let chime = vec![0.3f32; 3840]; // 1 full 40ms block of stereo
        tap.play_alert(&chime);

        let block = rx
            .try_recv()
            .expect("Direct alert submission should send block when idle");
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

    #[test]
    fn test_audio_tap_play_alert_arc_and_has_active_alert_clears() {
        let (tap, rx, free, _) = test_tap(4);
        assert!(!tap.has_active_alert.load(Ordering::Acquire));

        // Simulate active 48 kHz stereo game audio
        tap.submit(&[0.15f32; 3840], 48000, 2, now_us());
        free.send(rx.try_recv().unwrap()).unwrap();

        // Install a 2-block (7680 samples = 3840 frames) shared Arc<[f32]> alert
        let chime: Arc<[f32]> = Arc::from(vec![0.25f32; 7680]);
        tap.play_alert_arc(Arc::clone(&chime));
        assert!(tap.has_active_alert.load(Ordering::Acquire));

        // First block consumes half the alert; has_active_alert remains true
        tap.submit(&[0.15f32; 3840], 48000, 2, now_us());
        assert!(tap.has_active_alert.load(Ordering::Acquire));
        let first = rx.try_recv().unwrap();
        assert!((first.samples[0] - 0.40).abs() < 1e-4);
        free.send(first).unwrap();

        // Second block finishes the alert and clears has_active_alert
        tap.submit(&[0.15f32; 3840], 48000, 2, now_us());
        assert!(!tap.has_active_alert.load(Ordering::Acquire));
        assert!(tap.active_alert.lock().unwrap().is_none());
        let second = rx.try_recv().unwrap();
        assert!((second.samples[0] - 0.40).abs() < 1e-4);
        free.send(second).unwrap();

        // Subsequent submits take the lock-free fast-path with pure game audio
        tap.submit(&[0.15f32; 3840], 48000, 2, now_us());
        let third = rx.try_recv().unwrap();
        assert!((third.samples[0] - 0.15).abs() < 1e-4);
        free.send(third).unwrap();
    }
}
