//! PS5 Remote Play streaming session worker.
//!
//! Bridges `libchiaki` (via `chiaki_bridge.c`) into Michadame's core video/audio/replay
//! pipeline:
//! - Video: Annex-B H.264 / HEVC packets -> FFmpeg `VideoDecoderSession` (VAAPI/NVDEC/DRM
//!   hardware decode with automatic CPU fallback) -> `Arc<RawFrame>` sent to the shared
//!   rendering pipeline so all CRT / Anime4K / FX shaders, OCR, Screenshots, Replay Buffer,
//!   and Twitch streaming work identically to Capture Card / Video / PC-98 / PC-88.
//! - Audio: Opus-decoded stereo 48kHz PCM -> Michadame's shared `rodio` playback stream
//!   (respecting `master_volume`, updating `peak_amplitude` and `audio_latency_ms`) +
//!   `replay_audio` tap (`observe_reactive` + `submit`).
//! - Haptics & DualSense feedback: forwarded to `DualSenseBridgeHandle` (single SDL2 thread).

#[cfg(feature = "ps5-remote-play")]
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicU8},
    Arc,
};
#[cfg(feature = "ps5-remote-play")]
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};

#[cfg(feature = "ps5-remote-play")]
use std::ffi::{c_char, c_int, c_void, CString};
#[cfg(feature = "ps5-remote-play")]
use std::sync::Mutex;

#[cfg(feature = "ps5-remote-play")]
use rtrb::{Producer, RingBuffer};

use super::controller::DualSenseBridgeHandle;
use super::ffi::RegisteredConsoleData;
use super::psn::StoredPsnToken;
use super::{Ps5Codec, Ps5ConnectionMode};
use crate::video::types::RawFrame;

#[cfg(feature = "ps5-remote-play")]
use super::controller::ControllerCommand;
#[cfg(feature = "ps5-remote-play")]
use super::ffi;
#[cfg(feature = "ps5-remote-play")]
use crate::video::decoder::normalize_decoded_video_frame;
#[cfg(feature = "ps5-remote-play")]
use crate::video::hwdec::VideoDecoderSession;
#[cfg(feature = "ps5-remote-play")]
use crate::video::types::ColorRange;

#[derive(Clone, Debug)]
pub enum Ps5Command {
    Stop,
    SetVolume(f32),
    SetLoginPin(String),
    KeyboardSetText(String),
    KeyboardAccept,
    KeyboardReject,
    GoHome,
    GotoBed,
    RequestIdr,
}

#[derive(Clone, Debug)]
pub enum Ps5SessionEvent {
    Status(String),
    Connected,
    NicknameReceived(String),
    ResolutionChanged(u32, u32),
    PsnTokenRefreshed(StoredPsnToken),
    AutoRegistSucceeded(RegisteredConsoleData),
    LoginPinRequested { incorrect: bool },
    KeyboardOpen(String),
    KeyboardTextChange(String),
    KeyboardRemoteClose,
    CantDisplay(bool),
    Failed(String),
    Stopped(String),
}

pub struct Ps5SessionParams {
    pub console_name: String,
    pub host: String,
    pub regist_key: Option<[u8; 16]>,
    pub morning: Option<[u8; 16]>,
    pub psn_duid_hex: String,
    pub psn_token: Option<StoredPsnToken>,
    pub psn_account_id: Option<[u8; 8]>,
    pub console_login_pin: Option<String>,
    pub mode: Ps5ConnectionMode,
    pub auto_regist_only: bool,
    pub width: u32,
    pub height: u32,
    pub max_fps: u32,
    pub bitrate_kbps: u32,
    pub codec: Ps5Codec,
    pub enable_keyboard: bool,
    pub enable_dualsense_haptics: bool,
    pub allow_hwdec: bool,
    pub scaler_filter: Arc<AtomicU8>,
    pub color_range: Arc<AtomicU8>,
    pub master_volume: f32,
    pub peak_amplitude: Arc<AtomicU64>,
    pub audio_latency_ms: Arc<AtomicU64>,
    pub queue_drops: Arc<AtomicU64>,
    pub replay_audio: Arc<crate::replay::AudioTap>,
    pub frame_tx: Sender<Arc<RawFrame>>,
    pub command_rx: Receiver<Ps5Command>,
    pub event_tx: Sender<Ps5SessionEvent>,
    pub stop_requested: Arc<AtomicBool>,
    pub request_repaint: Arc<dyn Fn() + Send + Sync>,
    pub controller_handle: Option<DualSenseBridgeHandle>,
}

/// Effective input jitter period (30 ms @ 48 kHz) passed to `LiveSource`.
/// PS5 sends 10 ms (480-frame) Opus packets over UDP while Linux CPAL pulls ~25 ms bursts;
/// sizing `LiveSource` with a 30 ms jitter period yields a ~70 ms prebuffer and ~42.5 ms
/// steady-state target, keeping the ring buffer strictly non-empty across CPAL bursts and
/// UDP jitter while Catmull-Rom PI adaptive resampling tracks PS5-vs-PC clock skew seamlessly.
#[cfg_attr(not(feature = "ps5-remote-play"), allow(dead_code))]
const PS5_AUDIO_JITTER_PERIOD_FRAMES: u32 = 1_440;

#[cfg(feature = "ps5-remote-play")]
const PS5_VIDEO_DECODE_QUEUE_CAPACITY: usize = 8;

/// Converts interleaved signed 16-bit PCM from `libchiaki`'s Opus decoder into normalized `f32`
/// samples ready for stereo/mono playback and replay capture.
///
/// - 1-channel mono and 2-channel stereo pass through 1:1.
/// - Multi-channel streams (5.1 / 7.1) are downmixed to stereo with ITU center/surround weights
///   so center dialogue is preserved and stereo outputs never truncate channels.
///
/// Returns `(output_channels, max_abs_sample)`.
#[cfg_attr(not(feature = "ps5-remote-play"), allow(dead_code))]
fn convert_ps5_pcm_i16_to_f32(
    raw: &[i16],
    input_channels: usize,
    out: &mut Vec<f32>,
) -> (u16, f32) {
    out.clear();
    let in_ch = input_channels.max(1);
    let frames = raw.len() / in_ch;
    if frames == 0 {
        return (if in_ch == 1 { 1 } else { 2 }, 0.0);
    }

    let mut max_abs = 0.0f32;
    if in_ch <= 2 {
        let aligned = frames * in_ch;
        out.reserve(aligned);
        for &s in &raw[..aligned] {
            let f = (s as f32) / 32768.0;
            let a = f.abs();
            if a > max_abs {
                max_abs = a;
            }
            out.push(f);
        }
        return (in_ch as u16, max_abs);
    }

    const INV_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;
    out.reserve(frames * 2);
    for frame in 0..frames {
        let base = frame * in_ch;
        let (l, r) = match in_ch {
            6 => {
                // 5.1: FL, FR, FC, LFE, BL, BR
                let fl = raw[base] as f32 / 32768.0;
                let fr = raw[base + 1] as f32 / 32768.0;
                let fc = raw[base + 2] as f32 / 32768.0;
                let bl = raw[base + 4] as f32 / 32768.0;
                let br = raw[base + 5] as f32 / 32768.0;
                (
                    (fl + INV_SQRT2 * fc + INV_SQRT2 * bl).clamp(-1.0, 1.0),
                    (fr + INV_SQRT2 * fc + INV_SQRT2 * br).clamp(-1.0, 1.0),
                )
            }
            8 => {
                // 7.1: FL, FR, FC, LFE, BL, BR, SL, SR
                let fl = raw[base] as f32 / 32768.0;
                let fr = raw[base + 1] as f32 / 32768.0;
                let fc = raw[base + 2] as f32 / 32768.0;
                let bl = raw[base + 4] as f32 / 32768.0;
                let br = raw[base + 5] as f32 / 32768.0;
                let sl = raw[base + 6] as f32 / 32768.0;
                let sr = raw[base + 7] as f32 / 32768.0;
                (
                    (fl + INV_SQRT2 * fc + INV_SQRT2 * bl + INV_SQRT2 * sl).clamp(-1.0, 1.0),
                    (fr + INV_SQRT2 * fc + INV_SQRT2 * br + INV_SQRT2 * sr).clamp(-1.0, 1.0),
                )
            }
            _ => {
                let mut sum_l = 0.0f32;
                let mut sum_r = 0.0f32;
                let mut cnt_l = 0usize;
                let mut cnt_r = 0usize;
                for c in 0..in_ch {
                    let s = raw[base + c] as f32 / 32768.0;
                    if c % 2 == 0 {
                        sum_l += s;
                        cnt_l += 1;
                    } else {
                        sum_r += s;
                        cnt_r += 1;
                    }
                }
                (
                    (sum_l / (cnt_l.max(1) as f32)).clamp(-1.0, 1.0),
                    (sum_r / (cnt_r.max(1) as f32)).clamp(-1.0, 1.0),
                )
            }
        };
        max_abs = max_abs.max(l.abs()).max(r.abs());
        out.push(l);
        out.push(r);
    }
    (2, max_abs)
}

#[cfg(feature = "ps5-remote-play")]
struct Ps5AudioPlayback {
    _guard: crate::devices::audio::SharedPlaybackGuard,
    _handle: rodio::OutputStreamHandle,
    statistics: Arc<crate::devices::audio::PlaybackStatistics>,
    stop: Arc<AtomicBool>,
}

#[cfg(feature = "ps5-remote-play")]
impl Drop for Ps5AudioPlayback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let silence = self.statistics.silence_frames.load(Ordering::Relaxed);
        let dropped = self.statistics.drift_dropped_frames.load(Ordering::Relaxed);
        if silence > 0 || dropped > 0 {
            tracing::debug!(
                silence_frames = silence,
                drift_dropped_frames = dropped,
                "PS5 Remote Play audio playback stopped"
            );
        }
    }
}

#[cfg(feature = "ps5-remote-play")]
struct VideoDecodeContext {
    codec_id: ffmpeg_next::codec::Id,
    allow_hwdec: bool,
    max_fps: u32,
    decoder: Option<VideoDecoderSession>,
    decoded: ffmpeg_next::frame::Video,
    normalized: ffmpeg_next::frame::Video,
    scaler: Option<ffmpeg_next::software::scaling::context::Context>,
    active_scaler_filter: u8,
    last_resolution: (u32, u32),
    scaler_filter: Arc<AtomicU8>,
    color_range: Arc<AtomicU8>,
    queue_drops: Arc<AtomicU64>,
    frame_tx: Sender<Arc<RawFrame>>,
    event_tx: Sender<Ps5SessionEvent>,
    request_repaint: Arc<dyn Fn() + Send + Sync>,
    ffmpeg_inited: bool,
    init_failed: bool,
}

#[cfg(feature = "ps5-remote-play")]
impl VideoDecodeContext {
    fn decode_sample(&mut self, data: &[u8]) -> bool {
        if data.is_empty() || self.init_failed {
            return true;
        }

        if !self.ffmpeg_inited {
            if let Err(err) = ffmpeg_next::init() {
                tracing::error!("PS5 video decoder FFmpeg init failed: {err}");
                self.init_failed = true;
                return true;
            }
            self.ffmpeg_inited = true;
        }

        if self.decoder.is_none() {
            match VideoDecoderSession::open_raw_codec_low_latency(self.codec_id, self.allow_hwdec) {
                Ok(session) => {
                    tracing::info!(
                        codec = ?self.codec_id,
                        hw = session.is_hardware(),
                        backend = ?session.hw_backend_name(),
                        "PS5 Remote Play FFmpeg decoder initialized"
                    );
                    self.decoder = Some(session);
                }
                Err(err) => {
                    tracing::error!(
                        codec = ?self.codec_id,
                        error = %err,
                        "Failed to open FFmpeg video decoder for PS5 stream"
                    );
                    self.init_failed = true;
                    return true;
                }
            }
        }

        let Some(decoder) = self.decoder.as_mut() else {
            return true;
        };

        let pkt = ffmpeg_next::Packet::copy(data);
        if let Err(err) = decoder.send_packet(&pkt) {
            tracing::debug!(error = %err, bytes = data.len(), "PS5 video decoder rejected packet; requesting IDR");
            return false;
        }

        let replay_rate = crate::replay::config::Rate::new(self.max_fps.max(1), 1);

        loop {
            match decoder.receive_decoded_frame(&mut self.decoded) {
                Ok(true) => {}
                Ok(false) => break,
                Err(err) => {
                    tracing::debug!(error = %err, "PS5 video receive_decoded_frame error");
                    break;
                }
            }

            let desired_scaler = self.scaler_filter.load(Ordering::Relaxed);
            let normalized = match normalize_decoded_video_frame(
                &self.decoded,
                &mut self.normalized,
                &mut self.scaler,
                &mut self.active_scaler_filter,
                desired_scaler,
            ) {
                Ok(Some(res)) => res,
                Ok(None) => continue,
                Err(err) => {
                    tracing::warn!(error = %err, "PS5 frame normalization failed");
                    continue;
                }
            };

            let (width, height, format, frame_data) = normalized;
            if (width, height) != self.last_resolution {
                self.last_resolution = (width, height);
                let _ = self
                    .event_tx
                    .send(Ps5SessionEvent::ResolutionChanged(width, height));
            }

            let range_val = self.color_range.load(Ordering::Relaxed);
            let raw_frame = Arc::new(RawFrame {
                captured_at: crate::replay::now_us(),
                rate: replay_rate,
                width,
                height,
                data: frame_data,
                format,
                color_range: ColorRange::from_u8(range_val),
            });

            match self.frame_tx.try_send(raw_frame) {
                Ok(()) => (self.request_repaint)(),
                Err(crossbeam_channel::TrySendError::Full(_)) => {
                    self.queue_drops.fetch_add(1, Ordering::Relaxed);
                }
                Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                    return true;
                }
            }
        }

        true
    }
}

#[cfg(feature = "ps5-remote-play")]
struct AudioOutputContext {
    input_channels: u16,
    output_channels: u16,
    sample_rate: u32,
    volume_bits: Arc<AtomicU32>,
    peak_amplitude: Arc<AtomicU64>,
    audio_latency_ms: Arc<AtomicU64>,
    replay_audio: Arc<crate::replay::AudioTap>,
    playback: Option<Ps5AudioPlayback>,
    playback_init_attempted: bool,
    producer: Option<Producer<f32>>,
    scratch_f32: Vec<f32>,
}

#[cfg(feature = "ps5-remote-play")]
impl AudioOutputContext {
    fn new(
        volume_bits: Arc<AtomicU32>,
        peak_amplitude: Arc<AtomicU64>,
        audio_latency_ms: Arc<AtomicU64>,
        replay_audio: Arc<crate::replay::AudioTap>,
    ) -> Self {
        Self {
            input_channels: 2,
            output_channels: 2,
            sample_rate: 48_000,
            volume_bits,
            peak_amplitude,
            audio_latency_ms,
            replay_audio,
            playback: None,
            playback_init_attempted: false,
            producer: None,
            scratch_f32: Vec::with_capacity(2048),
        }
    }

    fn configure(&mut self, channels: u32, sample_rate: u32) {
        let in_ch = channels.clamp(1, 8) as u16;
        let out_ch = if in_ch == 1 { 1 } else { 2 };
        let rate = sample_rate.clamp(8_000, 192_000);
        if self.playback.is_some()
            && self.input_channels == in_ch
            && self.output_channels == out_ch
            && self.sample_rate == rate
        {
            return;
        }
        self.input_channels = in_ch;
        self.output_channels = out_ch;
        self.sample_rate = rate;
        self.playback = None;
        self.playback_init_attempted = true;
        self.producer = None;

        let stop = Arc::new(AtomicBool::new(false));
        let ring_frames =
            (rate.max(48_000) as usize * 2).max(PS5_AUDIO_JITTER_PERIOD_FRAMES as usize * 12);
        let (producer, consumer) = RingBuffer::<f32>::new(ring_frames * out_ch as usize);

        match crate::devices::audio::initialize_rodio_playback() {
            Ok((stream_guard, stream_handle)) => {
                let statistics = Arc::new(crate::devices::audio::PlaybackStatistics::default());
                let capture_delay_ms = Arc::new(AtomicU64::new(10));
                let source = crate::devices::audio::LiveSource::new(
                    Arc::clone(&statistics),
                    consumer,
                    out_ch,
                    rate,
                    PS5_AUDIO_JITTER_PERIOD_FRAMES,
                    Arc::clone(&self.audio_latency_ms),
                    capture_delay_ms,
                    Arc::clone(&stop),
                    Arc::clone(&self.volume_bits),
                );
                match stream_handle.play_raw(source) {
                    Ok(()) => {
                        self.playback = Some(Ps5AudioPlayback {
                            _guard: stream_guard,
                            _handle: stream_handle,
                            statistics,
                            stop,
                        });
                        self.producer = Some(producer);
                        tracing::info!(
                            input_channels = in_ch,
                            output_channels = out_ch,
                            sample_rate = rate,
                            "PS5 Remote Play audio output started (adaptive LiveSource)"
                        );
                    }
                    Err(err) => {
                        tracing::warn!("Failed to start PS5 raw rodio playback: {err}");
                    }
                }
            }
            Err(err) => {
                tracing::warn!("Failed to open audio output for PS5 Remote Play: {err}");
            }
        }
    }

    fn push_pcm_i16(&mut self, samples_per_channel: usize, raw: &[i16]) {
        if samples_per_channel == 0 || raw.is_empty() {
            return;
        }
        if self.playback.is_none() && !self.playback_init_attempted {
            self.configure(self.input_channels as u32, self.sample_rate);
        }

        let in_ch = self.input_channels as usize;
        let total_in_samples = samples_per_channel.saturating_mul(in_ch).min(raw.len());
        let aligned_in = (total_in_samples / in_ch) * in_ch;
        if aligned_in == 0 {
            return;
        }

        let (out_ch, max_abs) =
            convert_ps5_pcm_i16_to_f32(&raw[..aligned_in], in_ch, &mut self.scratch_f32);
        let slice_f32 = self.scratch_f32.as_slice();
        let out_ch_usize = out_ch as usize;

        if let Some(ref mut prod) = self.producer {
            let free = prod.slots();
            let writable = ((free / out_ch_usize) * out_ch_usize).min(slice_f32.len());
            if writable >= out_ch_usize {
                if let Ok(mut wchunk) = prod.write_chunk(writable) {
                    let (first, second) = wchunk.as_mut_slices();
                    let f_len = first.len();
                    let s_len = second.len();
                    first.copy_from_slice(&slice_f32[..f_len]);
                    if s_len > 0 {
                        second.copy_from_slice(&slice_f32[f_len..f_len + s_len]);
                    }
                    wchunk.commit_all();
                }
            }
        }

        let peak_bits = (max_abs * 1000.0) as u64;
        self.peak_amplitude.fetch_max(peak_bits, Ordering::Relaxed);

        self.replay_audio
            .observe_reactive(slice_f32, self.sample_rate, out_ch);
        if self.replay_audio.is_enabled() {
            self.replay_audio
                .submit(slice_f32, self.sample_rate, out_ch, crate::replay::now_us());
        }
    }
}

#[cfg(feature = "ps5-remote-play")]
struct QuitState {
    reason_code: i32,
    is_error: bool,
    message: String,
}

#[cfg(feature = "ps5-remote-play")]
struct SessionCallbackState {
    video_pkt_tx: Option<Sender<Vec<u8>>>,
    video_idr_needed: Arc<AtomicBool>,
    queue_drops: Arc<AtomicU64>,
    audio: Mutex<AudioOutputContext>,
    event_tx: Sender<Ps5SessionEvent>,
    request_repaint: Arc<dyn Fn() + Send + Sync>,
    controller_handle: Option<DualSenseBridgeHandle>,
    enable_dualsense_haptics: bool,
    connected_flag: AtomicBool,
    quit_flag: AtomicBool,
    quit_info: Mutex<Option<QuitState>>,
}

#[cfg(feature = "ps5-remote-play")]
unsafe extern "C" fn bridge_video_cb(
    buf: *const u8,
    buf_size: usize,
    _frames_lost: i32,
    _frame_recovered: bool,
    user: *mut c_void,
) -> bool {
    if buf.is_null() || buf_size == 0 || user.is_null() {
        return true;
    }
    let state = &*(user as *const SessionCallbackState);
    let idr_needed = state.video_idr_needed.swap(false, Ordering::AcqRel);
    let Some(ref tx) = state.video_pkt_tx else {
        return true;
    };
    let data = std::slice::from_raw_parts(buf, buf_size);
    match tx.try_send(data.to_vec()) {
        Ok(()) => !idr_needed,
        Err(crossbeam_channel::TrySendError::Full(_)) => {
            state.queue_drops.fetch_add(1, Ordering::Relaxed);
            false
        }
        Err(crossbeam_channel::TrySendError::Disconnected(_)) => true,
    }
}

#[cfg(feature = "ps5-remote-play")]
unsafe extern "C" fn bridge_audio_settings_cb(channels: u32, rate: u32, user: *mut c_void) {
    if user.is_null() {
        return;
    }
    let state = &*(user as *const SessionCallbackState);
    if let Ok(mut ctx) = state.audio.lock() {
        ctx.configure(channels, rate);
    }
}

#[cfg(feature = "ps5-remote-play")]
unsafe extern "C" fn bridge_audio_frame_cb(
    buf: *const i16,
    samples_per_channel: usize,
    user: *mut c_void,
) {
    if buf.is_null() || samples_per_channel == 0 || user.is_null() {
        return;
    }
    let state = &*(user as *const SessionCallbackState);
    if let Ok(mut ctx) = state.audio.lock() {
        let total = samples_per_channel.saturating_mul(ctx.input_channels as usize);
        let slice = std::slice::from_raw_parts(buf, total);
        ctx.push_pcm_i16(samples_per_channel, slice);
    }
}

#[cfg(feature = "ps5-remote-play")]
unsafe extern "C" fn bridge_haptics_frame_cb(buf: *const u8, buf_size: usize, user: *mut c_void) {
    if buf.is_null() || buf_size == 0 || user.is_null() {
        return;
    }
    let state = &*(user as *const SessionCallbackState);
    if !state.enable_dualsense_haptics {
        return;
    }
    if let Some(ref handle) = state.controller_handle {
        let slice = std::slice::from_raw_parts(buf, buf_size);
        handle.send_effect(ControllerCommand::PushHaptics(slice.to_vec()));
    }
}

#[cfg(feature = "ps5-remote-play")]
unsafe extern "C" fn bridge_event_cb(ev: *const ffi::RawPs5Event, user: *mut c_void) {
    if ev.is_null() || user.is_null() {
        return;
    }
    let ev = &*ev;
    let state = &*(user as *const SessionCallbackState);

    match ev.r#type {
        ffi::EV_CONNECTED => {
            state.connected_flag.store(true, Ordering::Release);
            let _ = state.event_tx.send(Ps5SessionEvent::Connected);
            (state.request_repaint)();
        }
        ffi::EV_QUIT => {
            let msg = ffi::c_ptr_to_string(ev.text);
            if let Ok(mut guard) = state.quit_info.lock() {
                *guard = Some(QuitState {
                    reason_code: ev.quit_reason,
                    is_error: ev.quit_is_error,
                    message: msg,
                });
            }
            state.quit_flag.store(true, Ordering::Release);
            (state.request_repaint)();
        }
        ffi::EV_LOGIN_PIN_REQUEST => {
            let _ = state
                .event_tx
                .send(Ps5SessionEvent::LoginPinRequested { incorrect: ev.flag });
            (state.request_repaint)();
        }
        ffi::EV_REGIST => {
            let data = RegisteredConsoleData::from(&ev.regist);
            let _ = state
                .event_tx
                .send(Ps5SessionEvent::AutoRegistSucceeded(data));
            (state.request_repaint)();
        }
        ffi::EV_NICKNAME => {
            let nick = ffi::c_ptr_to_string(ev.text);
            if !nick.is_empty() {
                let _ = state.event_tx.send(Ps5SessionEvent::NicknameReceived(nick));
                (state.request_repaint)();
            }
        }
        ffi::EV_KEYBOARD_OPEN => {
            let initial = ffi::c_ptr_to_string(ev.text);
            let _ = state.event_tx.send(Ps5SessionEvent::KeyboardOpen(initial));
            (state.request_repaint)();
        }
        ffi::EV_KEYBOARD_TEXT_CHANGE => {
            let updated = ffi::c_ptr_to_string(ev.text);
            let _ = state
                .event_tx
                .send(Ps5SessionEvent::KeyboardTextChange(updated));
            (state.request_repaint)();
        }
        ffi::EV_KEYBOARD_REMOTE_CLOSE => {
            let _ = state.event_tx.send(Ps5SessionEvent::KeyboardRemoteClose);
            (state.request_repaint)();
        }
        ffi::EV_CANT_DISPLAY => {
            let _ = state.event_tx.send(Ps5SessionEvent::CantDisplay(ev.flag));
            (state.request_repaint)();
        }
        ffi::EV_RUMBLE => {
            if state.enable_dualsense_haptics {
                if let Some(ref handle) = state.controller_handle {
                    handle.send_effect(ControllerCommand::SetRumble {
                        left: ev.rumble_left,
                        right: ev.rumble_right,
                    });
                }
            }
        }
        ffi::EV_TRIGGER_EFFECTS => {
            if state.enable_dualsense_haptics {
                if let Some(ref handle) = state.controller_handle {
                    handle.send_effect(ControllerCommand::SetTriggerEffects {
                        type_left: ev.trigger_type_left,
                        left: ev.trigger_left,
                        type_right: ev.trigger_type_right,
                        right: ev.trigger_right,
                    });
                }
            }
        }
        ffi::EV_LED_COLOR => {
            if let Some(ref handle) = state.controller_handle {
                handle.send_effect(ControllerCommand::SetLed {
                    r: ev.led_color[0],
                    g: ev.led_color[1],
                    b: ev.led_color[2],
                });
            }
        }
        ffi::EV_PLAYER_INDEX => {
            if let Some(ref handle) = state.controller_handle {
                handle.send_effect(ControllerCommand::SetPlayerIndex(ev.player_index));
            }
        }
        ffi::EV_HAPTIC_INTENSITY => {
            if let Some(ref handle) = state.controller_handle {
                handle.send_effect(ControllerCommand::SetHapticIntensity(ev.intensity));
            }
        }
        ffi::EV_TRIGGER_INTENSITY => {
            if let Some(ref handle) = state.controller_handle {
                handle.send_effect(ControllerCommand::SetTriggerIntensity(ev.intensity));
            }
        }
        ffi::EV_MOTION_RESET => {
            if let Some(ref handle) = state.controller_handle {
                handle.send_effect(ControllerCommand::ResetMotion);
            }
        }
        _ => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChiakiLogSeverity {
    Error,
    Warn,
    Info,
    Debug,
}

/// Maps `libchiaki`'s `ChiakiLogLevel` bitmask (`lib/include/chiaki/log.h`):
/// `ERROR = 1 << 0 (1)`, `WARNING = 1 << 1 (2)`, `INFO = 1 << 2 (4)`,
/// `VERBOSE = 1 << 3 (8)`, `DEBUG = 1 << 4 (16)`.
#[cfg_attr(not(feature = "ps5-remote-play"), allow(dead_code))]
fn classify_chiaki_log_level(level: i32) -> ChiakiLogSeverity {
    const CHIAKI_LOG_ERROR: i32 = 1 << 0;
    const CHIAKI_LOG_WARNING: i32 = 1 << 1;
    const CHIAKI_LOG_INFO: i32 = 1 << 2;

    if (level & CHIAKI_LOG_ERROR) != 0 {
        ChiakiLogSeverity::Error
    } else if (level & CHIAKI_LOG_WARNING) != 0 {
        ChiakiLogSeverity::Warn
    } else if (level & CHIAKI_LOG_INFO) != 0 {
        ChiakiLogSeverity::Info
    } else {
        ChiakiLogSeverity::Debug
    }
}

#[cfg(feature = "ps5-remote-play")]
unsafe extern "C" fn bridge_log_cb(level: c_int, msg: *const c_char, _user: *mut c_void) {
    let text = ffi::c_ptr_to_string(msg);
    if text.is_empty() {
        return;
    }
    match classify_chiaki_log_level(level as i32) {
        ChiakiLogSeverity::Error => tracing::error!(target: "libchiaki", "{text}"),
        ChiakiLogSeverity::Warn => tracing::warn!(target: "libchiaki", "{text}"),
        ChiakiLogSeverity::Info => tracing::info!(target: "libchiaki", "{text}"),
        ChiakiLogSeverity::Debug => tracing::debug!(target: "libchiaki", "{text}"),
    }
}

#[cfg(feature = "ps5-remote-play")]
enum AttemptOutcome {
    Stopped(String),
    FallbackToPsn(String),
    Failed(String),
}

#[cfg(feature = "ps5-remote-play")]
fn run_single_attempt(
    params: &Ps5SessionParams,
    use_psn: bool,
    psn_access_token: Option<&str>,
    volume_bits: &Arc<AtomicU32>,
    can_fallback_to_psn: bool,
) -> AttemptOutcome {
    let codec_id = match params.codec {
        Ps5Codec::H264 => ffmpeg_next::codec::Id::H264,
        Ps5Codec::H265 => ffmpeg_next::codec::Id::HEVC,
    };

    let video_idr_needed = Arc::new(AtomicBool::new(false));
    let (video_pkt_tx, video_decode_thread) = if params.auto_regist_only {
        (None, None)
    } else {
        let (tx, rx) = crossbeam_channel::bounded::<Vec<u8>>(PS5_VIDEO_DECODE_QUEUE_CAPACITY);
        let idr_flag = Arc::clone(&video_idr_needed);
        let allow_hwdec = params.allow_hwdec;
        let max_fps = params.max_fps;
        let scaler_filter = Arc::clone(&params.scaler_filter);
        let color_range = Arc::clone(&params.color_range);
        let queue_drops = Arc::clone(&params.queue_drops);
        let frame_tx = params.frame_tx.clone();
        let event_tx = params.event_tx.clone();
        let request_repaint = Arc::clone(&params.request_repaint);
        let handle = std::thread::Builder::new()
            .name("ps5-video-decode".to_string())
            .spawn(move || {
                let mut decode_ctx = VideoDecodeContext {
                    codec_id,
                    allow_hwdec,
                    max_fps,
                    decoder: None,
                    decoded: ffmpeg_next::frame::Video::empty(),
                    normalized: ffmpeg_next::frame::Video::empty(),
                    scaler: None,
                    active_scaler_filter: 0,
                    last_resolution: (0, 0),
                    scaler_filter,
                    color_range,
                    queue_drops,
                    frame_tx,
                    event_tx,
                    request_repaint,
                    ffmpeg_inited: false,
                    init_failed: false,
                };
                while let Ok(pkt) = rx.recv() {
                    if !decode_ctx.decode_sample(&pkt) {
                        idr_flag.store(true, Ordering::Release);
                    }
                }
            })
            .ok();
        (Some(tx), handle)
    };

    let cb_state = Box::new(SessionCallbackState {
        video_pkt_tx,
        video_idr_needed,
        queue_drops: Arc::clone(&params.queue_drops),
        audio: Mutex::new(AudioOutputContext::new(
            Arc::clone(volume_bits),
            Arc::clone(&params.peak_amplitude),
            Arc::clone(&params.audio_latency_ms),
            Arc::clone(&params.replay_audio),
        )),
        event_tx: params.event_tx.clone(),
        request_repaint: Arc::clone(&params.request_repaint),
        controller_handle: if params.auto_regist_only {
            None
        } else {
            params.controller_handle.clone()
        },
        enable_dualsense_haptics: params.enable_dualsense_haptics,
        connected_flag: AtomicBool::new(false),
        quit_flag: AtomicBool::new(false),
        quit_info: Mutex::new(None),
    });
    let cb_state_ptr = Box::into_raw(cb_state);

    let c_host = CString::new(params.host.trim()).unwrap_or_else(|_| CString::new("").unwrap());
    let c_psn_tok = psn_access_token.and_then(|s| CString::new(s).ok());
    let c_psn_duid = if use_psn && !params.psn_duid_hex.trim().is_empty() {
        CString::new(params.psn_duid_hex.trim()).ok()
    } else {
        None
    };

    let raw_cfg = ffi::RawSessionConfig {
        host: if use_psn {
            std::ptr::null()
        } else {
            c_host.as_ptr()
        },
        regist_key: params.regist_key.unwrap_or([0u8; 16]),
        morning: params.morning.unwrap_or([0u8; 16]),
        psn_oauth_token: if use_psn {
            c_psn_tok.as_ref().map_or(std::ptr::null(), |s| s.as_ptr())
        } else {
            std::ptr::null()
        },
        psn_duid_hex: if use_psn {
            c_psn_duid.as_ref().map_or(std::ptr::null(), |s| s.as_ptr())
        } else {
            std::ptr::null()
        },
        psn_account_id: params.psn_account_id.unwrap_or([0u8; 8]),
        auto_regist: params.auto_regist_only,
        width: params.width,
        height: params.height,
        max_fps: params.max_fps,
        bitrate_kbps: params.bitrate_kbps,
        codec: match params.codec {
            Ps5Codec::H264 => 0,
            Ps5Codec::H265 => 1,
        },
        enable_keyboard: params.enable_keyboard,
        enable_dualsense: !params.auto_regist_only,
        video_cb: if params.auto_regist_only {
            None
        } else {
            Some(bridge_video_cb)
        },
        audio_settings_cb: if params.auto_regist_only {
            None
        } else {
            Some(bridge_audio_settings_cb)
        },
        audio_frame_cb: if params.auto_regist_only {
            None
        } else {
            Some(bridge_audio_frame_cb)
        },
        haptics_frame_cb: if params.auto_regist_only {
            None
        } else {
            Some(bridge_haptics_frame_cb)
        },
        event_cb: Some(bridge_event_cb),
        log_cb: Some(bridge_log_cb),
        user: cb_state_ptr.cast::<c_void>(),
    };

    let mut err_buf = [0 as c_char; 256];
    let session =
        unsafe { ffi::michadame_ps5_session_create(&raw_cfg, err_buf.as_mut_ptr(), err_buf.len()) };
    if session.is_null() {
        let msg = ffi::c_char_buf_to_string(&err_buf);
        let msg = if msg.is_empty() {
            "Failed to initialize PS5 Remote Play session".to_string()
        } else {
            msg
        };
        unsafe { drop(Box::from_raw(cb_state_ptr)) };
        if let Some(h) = video_decode_thread {
            let _ = h.join();
        }
        if can_fallback_to_psn && !use_psn && !params.stop_requested.load(Ordering::Acquire) {
            return AttemptOutcome::FallbackToPsn(msg);
        }
        return AttemptOutcome::Failed(msg);
    }

    if !params.auto_regist_only {
        if let Some(ref handle) = params.controller_handle {
            handle.attach_session(session as usize, params.enable_dualsense_haptics);
        }
    }

    let start_rc = if use_psn {
        let _ = params.event_tx.send(Ps5SessionEvent::Status(format!(
            "Connecting to {} via PSN Remote Play...",
            params.console_name
        )));
        (params.request_repaint)();
        let duid_ptr = c_psn_duid.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
        unsafe {
            ffi::michadame_ps5_session_connect_psn_and_start(
                session,
                duid_ptr,
                err_buf.as_mut_ptr(),
                err_buf.len(),
            )
        }
    } else {
        let _ = params.event_tx.send(Ps5SessionEvent::Status(format!(
            "Connecting to {} ({}) on local network...",
            params.console_name, params.host
        )));
        (params.request_repaint)();
        unsafe {
            ffi::michadame_ps5_session_start_local(session, err_buf.as_mut_ptr(), err_buf.len())
        }
    };

    if start_rc != 0 {
        let msg = ffi::c_char_buf_to_string(&err_buf);
        let msg = if msg.is_empty() {
            format!("Failed to start PS5 session (code {start_rc})")
        } else {
            msg
        };
        if !params.auto_regist_only {
            if let Some(ref handle) = params.controller_handle {
                handle.detach_session();
            }
        }
        unsafe {
            ffi::michadame_ps5_session_stop(session);
            ffi::michadame_ps5_session_destroy(session);
            drop(Box::from_raw(cb_state_ptr));
        }
        if let Some(h) = video_decode_thread {
            let _ = h.join();
        }
        if can_fallback_to_psn && !use_psn && !params.stop_requested.load(Ordering::Acquire) {
            return AttemptOutcome::FallbackToPsn(msg);
        }
        return AttemptOutcome::Failed(msg);
    }

    // Command & lifecycle loop until stop requested or libchiaki signals EV_QUIT
    let mut user_stopped = false;
    let mut auto_pin_submitted = false;
    while !params.stop_requested.load(Ordering::Acquire) {
        let quit_now = unsafe { (&*cb_state_ptr).quit_flag.load(Ordering::Acquire) };
        if quit_now {
            break;
        }

        // If user has a saved 4-digit console login PIN and hasn't tried it yet, wait for
        // the UI event or submit directly when prompted via Ps5Command::SetLoginPin.
        let _ = &mut auto_pin_submitted;

        match params.command_rx.recv_timeout(Duration::from_millis(25)) {
            Ok(cmd) => match cmd {
                Ps5Command::Stop => {
                    user_stopped = true;
                    break;
                }
                Ps5Command::SetVolume(vol) => {
                    volume_bits.store(vol.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
                }
                Ps5Command::SetLoginPin(pin) => {
                    if let Ok(c_pin) = CString::new(pin.trim()) {
                        unsafe {
                            ffi::michadame_ps5_session_set_login_pin(session, c_pin.as_ptr());
                        }
                    }
                }
                Ps5Command::KeyboardSetText(text) => {
                    if let Ok(c_text) = CString::new(text) {
                        unsafe {
                            ffi::michadame_ps5_session_keyboard_set_text(session, c_text.as_ptr());
                        }
                    }
                }
                Ps5Command::KeyboardAccept => unsafe {
                    ffi::michadame_ps5_session_keyboard_accept(session);
                },
                Ps5Command::KeyboardReject => unsafe {
                    ffi::michadame_ps5_session_keyboard_reject(session);
                },
                Ps5Command::GoHome => unsafe {
                    ffi::michadame_ps5_session_go_home(session);
                },
                Ps5Command::GotoBed => unsafe {
                    ffi::michadame_ps5_session_goto_bed(session);
                },
                Ps5Command::RequestIdr => unsafe {
                    ffi::michadame_ps5_session_request_idr(session);
                },
            },
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                user_stopped = true;
                break;
            }
        }
    }

    if params.stop_requested.load(Ordering::Acquire) {
        user_stopped = true;
    }

    // Strict teardown ordering:
    // 1. Detach DualSense monitor synchronously so it never touches `session` again.
    // 2. Stop & destroy libchiaki session (joins all libchiaki worker threads).
    // 3. Reclaim and inspect boxed callback state safely.
    if !params.auto_regist_only {
        if let Some(ref handle) = params.controller_handle {
            handle.detach_session();
        }
    }

    unsafe {
        ffi::michadame_ps5_session_stop(session);
        ffi::michadame_ps5_session_destroy(session);
    }

    let cb_state = unsafe { Box::from_raw(cb_state_ptr) };
    let was_connected = cb_state.connected_flag.load(Ordering::Acquire);
    let quit_info = cb_state.quit_info.lock().ok().and_then(|mut g| g.take());
    drop(cb_state);
    if let Some(h) = video_decode_thread {
        let _ = h.join();
    }

    if user_stopped {
        return AttemptOutcome::Stopped("PS5 Remote Play disconnected.".to_string());
    }

    if let Some(q) = quit_info {
        let _ = q.reason_code;
        let treat_as_error = q.is_error || (!was_connected && !params.auto_regist_only);
        if treat_as_error {
            if can_fallback_to_psn && !use_psn && !was_connected {
                return AttemptOutcome::FallbackToPsn(q.message);
            }
            return AttemptOutcome::Failed(if q.message.is_empty() {
                "PS5 Remote Play connection terminated with an error.".to_string()
            } else {
                q.message
            });
        }
        return AttemptOutcome::Stopped(if q.message.is_empty() {
            "PS5 Remote Play ended.".to_string()
        } else {
            q.message
        });
    }

    AttemptOutcome::Stopped("PS5 Remote Play disconnected.".to_string())
}

/// Runs the PS5 Remote Play session worker on a background thread.
#[cfg(feature = "ps5-remote-play")]
pub fn run_ps5_session_worker(params: Ps5SessionParams) {
    if let Err(err) = ffi::init_lib() {
        let _ = params.event_tx.send(Ps5SessionEvent::Failed(format!(
            "PS5 Remote Play backend init failed: {err}"
        )));
        (params.request_repaint)();
        return;
    }

    let volume_bits = Arc::new(AtomicU32::new(
        params.master_volume.clamp(0.0, 1.0).to_bits(),
    ));

    let has_local_creds =
        !params.host.trim().is_empty() && params.regist_key.is_some() && params.morning.is_some();
    let has_psn_target = !params.psn_duid_hex.trim().is_empty()
        && params.psn_token.is_some()
        && params.psn_account_id.is_some();

    let (try_local_first, allow_psn_fallback, require_psn) = if params.auto_regist_only {
        (false, false, true)
    } else {
        match params.mode {
            Ps5ConnectionMode::LocalLan => (true, false, false),
            Ps5ConnectionMode::PsnRemote => (false, false, true),
            Ps5ConnectionMode::Auto => {
                if has_local_creds {
                    (true, has_psn_target, false)
                } else if has_psn_target {
                    (false, false, true)
                } else {
                    (true, false, false)
                }
            }
        }
    };

    if try_local_first {
        if !has_local_creds {
            let _ = params.event_tx.send(Ps5SessionEvent::Failed(
                "Selected PS5 console is not registered for local LAN streaming yet. Register it via PIN or sign in to PSN first."
                    .to_string(),
            ));
            (params.request_repaint)();
            return;
        }

        match run_single_attempt(&params, false, None, &volume_bits, allow_psn_fallback) {
            AttemptOutcome::Stopped(msg) => {
                let _ = params.event_tx.send(Ps5SessionEvent::Stopped(msg));
                (params.request_repaint)();
                return;
            }
            AttemptOutcome::Failed(err) => {
                let _ = params.event_tx.send(Ps5SessionEvent::Failed(err));
                (params.request_repaint)();
                return;
            }
            AttemptOutcome::FallbackToPsn(local_err) => {
                tracing::info!(
                    error = %local_err,
                    "Local LAN PS5 connection failed; falling back to PSN Remote Play"
                );
                let _ = params.event_tx.send(Ps5SessionEvent::Status(
                    "Local LAN unreachable — trying PSN Remote Play...".to_string(),
                ));
                (params.request_repaint)();
            }
        }
    } else if !require_psn {
        let _ = params.event_tx.send(Ps5SessionEvent::Failed(
            "No PS5 connection target configured.".to_string(),
        ));
        (params.request_repaint)();
        return;
    }

    // PSN Remote Play / Auto-Regist path
    if params.stop_requested.load(Ordering::Acquire) {
        let _ = params.event_tx.send(Ps5SessionEvent::Stopped(
            "PS5 Remote Play cancelled.".to_string(),
        ));
        (params.request_repaint)();
        return;
    }

    let Some(ref stored_token) = params.psn_token else {
        let _ = params.event_tx.send(Ps5SessionEvent::Failed(
            "PSN sign-in is required for remote connection or PSN auto-registration.".to_string(),
        ));
        (params.request_repaint)();
        return;
    };
    if params.psn_duid_hex.trim().is_empty() {
        let _ = params.event_tx.send(Ps5SessionEvent::Failed(
            "Selected PS5 does not have a PSN Device ID. Refresh PSN consoles in the PS5 tab."
                .to_string(),
        ));
        (params.request_repaint)();
        return;
    }
    if params.psn_account_id.is_none() {
        let _ = params.event_tx.send(Ps5SessionEvent::Failed(
            "PSN Account-ID is missing. Sign in to PSN in the PS5 tab.".to_string(),
        ));
        (params.request_repaint)();
        return;
    }

    let _ = params.event_tx.send(Ps5SessionEvent::Status(
        "Verifying PSN authentication token...".to_string(),
    ));
    (params.request_repaint)();

    let fresh_token = match super::psn::ensure_fresh_psn_token(stored_token) {
        Ok(tok) => {
            if tok.access_token != stored_token.access_token {
                let _ = params
                    .event_tx
                    .send(Ps5SessionEvent::PsnTokenRefreshed(tok.clone()));
            }
            tok
        }
        Err(err) => {
            let _ = params.event_tx.send(Ps5SessionEvent::Failed(format!(
                "PSN authentication refresh failed: {err}"
            )));
            (params.request_repaint)();
            return;
        }
    };

    match run_single_attempt(
        &params,
        true,
        Some(&fresh_token.access_token),
        &volume_bits,
        false,
    ) {
        AttemptOutcome::Stopped(msg) => {
            let _ = params.event_tx.send(Ps5SessionEvent::Stopped(msg));
        }
        AttemptOutcome::Failed(err) | AttemptOutcome::FallbackToPsn(err) => {
            let _ = params.event_tx.send(Ps5SessionEvent::Failed(err));
        }
    }
    (params.request_repaint)();
}

#[cfg(not(feature = "ps5-remote-play"))]
pub fn run_ps5_session_worker(params: Ps5SessionParams) {
    let _ = params.event_tx.send(Ps5SessionEvent::Failed(
        "PS5 Remote Play was disabled at compile time (built with --no-default-features)."
            .to_string(),
    ));
    (params.request_repaint)();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chiaki_log_level_bitmask_maps_error_warning_info_verbose_debug() {
        assert_eq!(classify_chiaki_log_level(1 << 0), ChiakiLogSeverity::Error);
        assert_eq!(classify_chiaki_log_level(1 << 1), ChiakiLogSeverity::Warn);
        assert_eq!(classify_chiaki_log_level(1 << 2), ChiakiLogSeverity::Info);
        assert_eq!(classify_chiaki_log_level(1 << 3), ChiakiLogSeverity::Debug);
        assert_eq!(classify_chiaki_log_level(1 << 4), ChiakiLogSeverity::Debug);
    }

    #[test]
    fn ps5_pcm_conversion_passes_stereo_and_downmixes_5_1_cleanly() {
        let mut out = Vec::new();
        let stereo = [16384i16, -16384, 0, 32767];
        let (ch, peak) = convert_ps5_pcm_i16_to_f32(&stereo, 2, &mut out);
        assert_eq!(ch, 2);
        assert_eq!(out.len(), 4);
        assert!((out[0] - 0.5).abs() < 1e-4);
        assert!((out[1] + 0.5).abs() < 1e-4);
        assert!(peak > 0.99);

        // 5.1 frame with center-channel-only dialogue should appear equally on L and R
        let surround_5_1 = [0i16, 0, 16384, 0, 0, 0];
        let (ch51, peak51) = convert_ps5_pcm_i16_to_f32(&surround_5_1, 6, &mut out);
        assert_eq!(ch51, 2);
        assert_eq!(out.len(), 2);
        let expected = 0.5 * std::f32::consts::FRAC_1_SQRT_2;
        assert!((out[0] - expected).abs() < 1e-4);
        assert!((out[1] - expected).abs() < 1e-4);
        assert!((peak51 - expected).abs() < 1e-4);
    }

    #[test]
    fn ps5_audio_live_source_streams_10ms_packets_through_25ms_cpal_bursts_without_underruns() {
        use rtrb::RingBuffer;
        use std::sync::atomic::{AtomicU32, Ordering};

        let sample_rate = 48_000u32;
        let channels = 2u16;
        let ring_frames =
            (sample_rate as usize * 2).max(PS5_AUDIO_JITTER_PERIOD_FRAMES as usize * 12);
        let (mut prod, cons) = RingBuffer::<f32>::new(ring_frames * channels as usize);
        let stats = Arc::new(crate::devices::audio::PlaybackStatistics::default());
        let latency_ms = Arc::new(AtomicU64::new(0));
        let capture_delay = Arc::new(AtomicU64::new(10));
        let stop = Arc::new(AtomicBool::new(false));
        let volume = Arc::new(AtomicU32::new(1.0f32.to_bits()));

        let mut source = crate::devices::audio::LiveSource::new(
            Arc::clone(&stats),
            cons,
            channels,
            sample_rate,
            PS5_AUDIO_JITTER_PERIOD_FRAMES,
            Arc::clone(&latency_ms),
            capture_delay,
            stop,
            volume,
        );

        // Simulate 2 seconds of 440 Hz sine wave arriving in 10 ms (480-frame) PS5 Opus packets
        // with +/- 5 ms UDP jitter while Linux CPAL pulls 25 ms (1200-frame) bursts.
        let mut prod_frame: u64 = 0;
        let mut started = false;
        let mut prev_l: Option<f32> = None;
        let mut max_step = 0.0f32;
        let freq = 440.0f64;

        // Prime initial 80 ms (8 packets of 10 ms)
        for _ in 0..8 {
            for _ in 0..480 {
                let s = ((2.0 * std::f64::consts::PI * freq * prod_frame as f64)
                    / sample_rate as f64)
                    .sin() as f32
                    * 0.5;
                prod.push(s).unwrap();
                prod.push(s).unwrap();
                prod_frame += 1;
            }
        }

        // Run 80 CPAL 25 ms bursts (2.0 seconds total), interleaving 2 or 3 10 ms packets
        // (average 2.5 packets = 25 ms per burst, simulating real UDP arrival jitter).
        for burst in 0..80 {
            let mut burst_samples = Vec::with_capacity(1200 * 2);
            for _ in 0..(1200 * 2) {
                burst_samples.push(source.next().unwrap());
            }
            for frame in 0..1200 {
                let l = burst_samples[frame * 2];
                if !started && l.abs() > 1e-4 {
                    started = true;
                }
                if started {
                    if let Some(p) = prev_l {
                        max_step = max_step.max((l - p).abs());
                    }
                    prev_l = Some(l);
                }
            }

            let packets_this_burst = if burst % 2 == 0 { 3 } else { 2 };
            for _ in 0..packets_this_burst {
                for _ in 0..480 {
                    let s = ((2.0 * std::f64::consts::PI * freq * prod_frame as f64)
                        / sample_rate as f64)
                        .sin() as f32
                        * 0.5;
                    prod.push(s).unwrap();
                    prod.push(s).unwrap();
                    prod_frame += 1;
                }
            }
        }

        assert!(started, "Expected continuous audio output after prebuffer");
        assert_eq!(
            stats.silence_frames.load(Ordering::Relaxed),
            0,
            "Expected zero audio underruns across 2 seconds of 10ms->25ms streaming"
        );
        assert_eq!(
            stats.drift_dropped_frames.load(Ordering::Relaxed),
            0,
            "Expected zero hard sample drops"
        );
        // For 440 Hz @ 48 kHz with amplitude 0.5, max true derivative per sample is ~0.029.
        // Any click/discontinuity would jump >> 0.1.
        assert!(
            max_step < 0.06,
            "Detected waveform discontinuity / crackle: max_step={max_step:.4}"
        );
    }
}
