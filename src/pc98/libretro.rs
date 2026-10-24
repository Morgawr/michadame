use super::{
    core_sync::{ensure_np2kai_core, ensure_system_dir, prepare_boot_cmd, saves_dir},
    input::{
        RETROK_F12, RETRO_DEVICE_ID_MOUSE_LEFT, RETRO_DEVICE_ID_MOUSE_MIDDLE,
        RETRO_DEVICE_ID_MOUSE_RIGHT, RETRO_DEVICE_ID_MOUSE_X, RETRO_DEVICE_ID_MOUSE_Y,
        RETRO_DEVICE_KEYBOARD, RETRO_DEVICE_MOUSE,
    },
    Pc98Command, Pc98Config, Pc98Event, Pc98InputState,
};
use crate::video::types::{ColorRange, RawFrame};
use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender};
use rodio::{Sink, Source};
use std::collections::{BTreeMap, VecDeque};
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

const RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY: c_uint = 9;
const RETRO_ENVIRONMENT_SET_PIXEL_FORMAT: c_uint = 10;
const RETRO_ENVIRONMENT_SET_DISK_CONTROL_INTERFACE: c_uint = 13;
const RETRO_ENVIRONMENT_GET_VARIABLE: c_uint = 15;
const RETRO_ENVIRONMENT_SET_VARIABLES: c_uint = 16;
const RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE: c_uint = 17;
const RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME: c_uint = 18;
const RETRO_ENVIRONMENT_GET_LOG_INTERFACE: c_uint = 27;
const RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY: c_uint = 31;
const RETRO_ENVIRONMENT_SET_SERIALIZATION_QUIRKS: c_uint = 44;
const RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION: c_uint = 52;
const RETRO_ENVIRONMENT_SET_CORE_OPTIONS: c_uint = 53;
const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_INTL: c_uint = 54;
const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_DISPLAY: c_uint = 55;

const PC98_AUDIO_SAMPLE_RATE: u32 = 44100;
const PC98_REPLAY_SAMPLE_RATE: u32 = 48000;
const PC98_AUDIO_CHANNELS: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetroPixelFormat {
    Rgb1555 = 0,
    Xrgb8888 = 1,
    Rgb565 = 2,
}

#[repr(C)]
struct RetroVariable {
    key: *const c_char,
    value: *const c_char,
}

#[repr(C)]
struct RetroGameInfo {
    path: *const c_char,
    data: *const c_void,
    size: usize,
    meta: *const c_char,
}

#[repr(C)]
struct RetroGameGeometry {
    base_width: c_uint,
    base_height: c_uint,
    max_width: c_uint,
    max_height: c_uint,
    aspect_ratio: f32,
}

#[repr(C)]
struct RetroSystemTiming {
    fps: f64,
    sample_rate: f64,
}

#[repr(C)]
struct RetroSystemAvInfo {
    geometry: RetroGameGeometry,
    timing: RetroSystemTiming,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RetroDiskControlCallback {
    set_eject_state: Option<unsafe extern "C" fn(ejected: bool) -> bool>,
    get_eject_state: Option<unsafe extern "C" fn() -> bool>,
    get_image_index: Option<unsafe extern "C" fn() -> c_uint>,
    set_image_index: Option<unsafe extern "C" fn(index: c_uint) -> bool>,
    get_num_images: Option<unsafe extern "C" fn() -> c_uint>,
    replace_image_index:
        Option<unsafe extern "C" fn(index: c_uint, info: *const RetroGameInfo) -> bool>,
    add_image_index: Option<unsafe extern "C" fn() -> bool>,
}

type RetroEnvironmentFn = unsafe extern "C" fn(cmd: c_uint, data: *mut c_void) -> bool;
type RetroVideoRefreshFn =
    unsafe extern "C" fn(data: *const c_void, width: c_uint, height: c_uint, pitch: usize);
type RetroAudioSampleFn = unsafe extern "C" fn(left: i16, right: i16);
type RetroAudioSampleBatchFn = unsafe extern "C" fn(data: *const i16, frames: usize) -> usize;
type RetroInputPollFn = unsafe extern "C" fn();
type RetroInputStateFn =
    unsafe extern "C" fn(port: c_uint, device: c_uint, index: c_uint, id: c_uint) -> i16;

struct Pc98AudioQueue {
    samples: VecDeque<f32>,
}

struct Pc98AudioSource {
    queue: Arc<Mutex<Pc98AudioQueue>>,
    read_buf: VecDeque<f32>,
}

impl Iterator for Pc98AudioSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.read_buf.is_empty() {
            if let Ok(mut q) = self.queue.try_lock() {
                let take_count = q.samples.len().min(64);
                if take_count > 0 {
                    self.read_buf.extend(q.samples.drain(..take_count));
                }
            }
            if self.read_buf.is_empty() {
                self.read_buf.push_back(0.0);
                self.read_buf.push_back(0.0);
            }
        }
        Some(self.read_buf.pop_front().unwrap_or(0.0))
    }
}

impl Source for Pc98AudioSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        PC98_AUDIO_CHANNELS
    }

    fn sample_rate(&self) -> u32 {
        PC98_AUDIO_SAMPLE_RATE
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

pub fn parse_wav_mono_f32(wav_bytes: &[u8]) -> Option<Vec<f32>> {
    if wav_bytes.len() < 44 || &wav_bytes[0..4] != b"RIFF" || &wav_bytes[8..12] != b"WAVE" {
        return None;
    }

    let mut pos = 12usize;
    let mut audio_format = 1u16;
    let mut channels = 1u16;
    let mut bits_per_sample = 16u16;
    let mut data_slice: Option<&[u8]> = None;

    while pos + 8 <= wav_bytes.len() {
        let chunk_id = &wav_bytes[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            wav_bytes[pos + 4],
            wav_bytes[pos + 5],
            wav_bytes[pos + 6],
            wav_bytes[pos + 7],
        ]) as usize;
        pos += 8;
        let end = pos.saturating_add(chunk_size).min(wav_bytes.len());

        if chunk_id == b"fmt " && end - pos >= 16 {
            let fmt = &wav_bytes[pos..end];
            audio_format = u16::from_le_bytes([fmt[0], fmt[1]]);
            channels = u16::from_le_bytes([fmt[2], fmt[3]]).max(1);
            bits_per_sample = u16::from_le_bytes([fmt[14], fmt[15]]);
        } else if chunk_id == b"data" {
            data_slice = Some(&wav_bytes[pos..end]);
            break;
        }

        // RIFF chunks are word-aligned
        pos = end + (chunk_size & 1);
    }

    if audio_format != 1 {
        return None;
    }
    let raw = data_slice?;
    let ch = channels as usize;

    let mut out = Vec::new();
    match bits_per_sample {
        16 => {
            let frame_bytes = 2 * ch;
            if frame_bytes == 0 {
                return None;
            }
            for frame in raw.chunks_exact(frame_bytes) {
                let mut sum = 0.0f32;
                for c in 0..ch {
                    let s = i16::from_le_bytes([frame[c * 2], frame[c * 2 + 1]]);
                    sum += (s as f32) / 32768.0;
                }
                out.push(sum / (ch as f32));
            }
        }
        8 => {
            if ch == 0 {
                return None;
            }
            for frame in raw.chunks_exact(ch) {
                let mut sum = 0.0f32;
                for &b in frame {
                    sum += ((b as f32) - 128.0) / 128.0;
                }
                out.push(sum / (ch as f32));
            }
        }
        _ => return None,
    }

    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

pub struct SeekPcmChannel {
    pub samples: Arc<[f32]>,
    pub pos: usize,
    pub playing: bool,
    pub looping: bool,
    pub volume: f32,
}

/// Mixes authentic NEC PC-98 / PC-88 mechanical floppy drive stepper motor seek sounds
/// (`fddseek.wav` for multi-track seeks and `fddseek1.wav` for single-track head steps)
/// directly into the 44.1 kHz stereo Libretro audio stream.
pub struct FloppySeekMixer {
    pub enabled: bool,
    pub volume: f32,
    pub channels: [SeekPcmChannel; 2],
}

impl FloppySeekMixer {
    pub fn new(enabled: bool, volume_pct: u32) -> Self {
        let np2_dir = super::core_sync::np2kai_system_subdir();

        let mut seek_multi = std::fs::read(np2_dir.join("fddseek.wav"))
            .ok()
            .and_then(|b| parse_wav_mono_f32(&b))
            .or_else(|| parse_wav_mono_f32(super::core_sync::EMBEDDED_FDDSEEK_WAV))
            .unwrap_or_default();

        // If `fddseek.wav` is the single-period 773-sample X11 loop (~17.5ms), expand it 17x
        // (13,141 samples = ~298ms, matching win9x/resources/fddseek.wav) so non-looping calls
        // from NP2kai's `fdc.c` (`soundmng_pcmplay(SOUND_PCMSEEK, FALSE)`) play the full
        // multi-track stepper burst while looping calls from `fdd_mtr.c` loop seamlessly.
        if !seek_multi.is_empty() && seek_multi.len() < 4000 {
            let period = seek_multi.clone();
            seek_multi.reserve(period.len() * 16);
            for _ in 0..16 {
                seek_multi.extend_from_slice(&period);
            }
        }

        let seek_step = std::fs::read(np2_dir.join("fddseek1.wav"))
            .ok()
            .and_then(|b| parse_wav_mono_f32(&b))
            .or_else(|| parse_wav_mono_f32(super::core_sync::EMBEDDED_FDDSEEK1_WAV))
            .unwrap_or_default();

        let vol = (volume_pct.clamp(0, 100) as f32) / 100.0;

        Self {
            enabled,
            volume: vol,
            channels: [
                SeekPcmChannel {
                    samples: Arc::from(seek_multi),
                    pos: 0,
                    playing: false,
                    looping: false,
                    volume: 1.0,
                },
                SeekPcmChannel {
                    samples: Arc::from(seek_step),
                    pos: 0,
                    playing: false,
                    looping: false,
                    volume: 1.0,
                },
            ],
        }
    }

    pub fn set_config(&mut self, enabled: bool, volume_pct: u32) {
        self.enabled = enabled;
        self.volume = (volume_pct.clamp(0, 100) as f32) / 100.0;
        if !enabled {
            for ch in &mut self.channels {
                ch.playing = false;
                ch.pos = 0;
            }
        }
    }

    pub fn play(&mut self, ch_idx: usize, looping: bool) {
        if !self.enabled || ch_idx >= self.channels.len() {
            return;
        }
        let ch = &mut self.channels[ch_idx];
        if ch.samples.is_empty() {
            return;
        }
        ch.looping = looping;
        // Matches DirectSound `CSoundDeviceDSound3::PlayPCM` in NP21W:
        // only reset `pos` to 0 when the channel is not already playing (or after `stop()`).
        if !ch.playing {
            ch.pos = 0;
            ch.playing = true;
        }
    }

    pub fn stop(&mut self, ch_idx: usize) {
        if ch_idx < self.channels.len() {
            let ch = &mut self.channels[ch_idx];
            ch.playing = false;
            ch.pos = 0;
        }
    }

    pub fn set_channel_volume(&mut self, ch_idx: usize, vol_pct: i32) {
        if ch_idx < self.channels.len() {
            self.channels[ch_idx].volume = (vol_pct.clamp(0, 100) as f32) / 100.0;
        }
    }

    pub fn mix_into_stereo(&mut self, stereo_samples: &mut [f32]) {
        if !self.enabled || self.volume <= 0.0 {
            for ch in &mut self.channels {
                ch.playing = false;
                ch.pos = 0;
            }
            return;
        }
        if !self.channels[0].playing && !self.channels[1].playing {
            return;
        }

        let master_vol = self.volume;
        for frame in stereo_samples.chunks_exact_mut(2) {
            let mut seek_mono = 0.0f32;
            for ch in &mut self.channels {
                if ch.playing {
                    if let Some(&s) = ch.samples.get(ch.pos) {
                        seek_mono += s * ch.volume * master_vol;
                    }
                    ch.pos += 1;
                    if ch.pos >= ch.samples.len() {
                        if ch.looping && !ch.samples.is_empty() {
                            ch.pos = 0;
                        } else {
                            ch.playing = false;
                            ch.pos = 0;
                        }
                    }
                }
            }
            if seek_mono != 0.0 {
                frame[0] = (frame[0] + seek_mono).clamp(-1.0, 1.0);
                frame[1] = (frame[1] + seek_mono).clamp(-1.0, 1.0);
            }
        }
    }
}

/// Matches `pcm_channel_t` in NP2kai's `x11/soundmng.c`.
#[repr(C)]
struct Np2PcmChannel {
    cookie: *mut c_void,
    path: *mut c_char,
    volume: c_int,
}

static mut NP2_PCM_CHANNELS: [Np2PcmChannel; 3] = [
    Np2PcmChannel {
        cookie: 1usize as *mut c_void,
        path: std::ptr::null_mut(),
        volume: 80,
    },
    Np2PcmChannel {
        cookie: 2usize as *mut c_void,
        path: std::ptr::null_mut(),
        volume: 80,
    },
    Np2PcmChannel {
        cookie: 3usize as *mut c_void,
        path: std::ptr::null_mut(),
        volume: 80,
    },
];

unsafe extern "C" fn np2_snddrv_pcmplay(
    _cookie: *mut c_void,
    num: c_uint,
    loop_flag: c_int,
) {
    if let Ok(mut guard) = ACTIVE_BRIDGE.lock() {
        if let Some(ctx) = guard.as_mut() {
            ctx.seek_mixer.play(num as usize, loop_flag != 0);
        }
    }
}

unsafe extern "C" fn np2_snddrv_pcmstop(_cookie: *mut c_void, num: c_uint) {
    if let Ok(mut guard) = ACTIVE_BRIDGE.lock() {
        if let Some(ctx) = guard.as_mut() {
            ctx.seek_mixer.stop(num as usize);
        }
    }
}

unsafe extern "C" fn np2_snddrv_pcmvolume(
    _cookie: *mut c_void,
    num: c_uint,
    volume: c_int,
) {
    if let Ok(mut guard) = ACTIVE_BRIDGE.lock() {
        if let Some(ctx) = guard.as_mut() {
            ctx.seek_mixer.set_channel_volume(num as usize, volume);
        }
    }
}

struct BridgeContext {
    sys_dir_cstr: CString,
    save_dir_cstr: CString,
    variables: BTreeMap<String, CString>,
    variables_dirty: bool,
    disk_control: Option<RetroDiskControlCallback>,
    pixel_format: RetroPixelFormat,
    frame_tx: Sender<Arc<RawFrame>>,
    event_tx: Sender<Pc98Event>,
    request_repaint: Arc<dyn Fn() + Send + Sync>,
    last_resolution: (u32, u32),
    last_frame: Option<Arc<RawFrame>>,
    input_state: Arc<Mutex<Pc98InputState>>,
    latched_keys: [bool; 512],
    latched_mouse_dx: i16,
    latched_mouse_dy: i16,
    latched_mouse_left: bool,
    latched_mouse_right: bool,
    latched_mouse_middle: bool,
    audio_queue: Arc<Mutex<Pc98AudioQueue>>,
    peak_amplitude: Arc<AtomicU64>,
    audio_latency_ms: Arc<AtomicU64>,
    replay_audio: Arc<crate::replay::AudioTap>,
    seek_mixer: FloppySeekMixer,
}

static ACTIVE_BRIDGE: Mutex<Option<Box<BridgeContext>>> = Mutex::new(None);

pub fn build_core_variables(cfg: &Pc98Config) -> BTreeMap<String, CString> {
    let mut vars = BTreeMap::new();
    let mut insert = |k: &str, v: &str| {
        if let Ok(c) = CString::new(v) {
            vars.insert(k.to_string(), c);
        }
    };

    insert("np2kai_drive", "FDD2");
    insert("np2kai_keyboard", "Ja");
    insert("np2kai_keyrepeat", "ON");
    insert("np2kai_keyrepeat_delay", "500ms");
    insert("np2kai_keyrepeat_interval", "50ms");
    insert("np2kai_model", &cfg.model);
    insert("np2kai_clk_base", &cfg.base_clock);
    insert("np2kai_clk_mult", &cfg.clock_mult.to_string());
    insert("np2kai_cpu_feature", &cfg.cpu_feature);
    insert("np2kai_ExMemory", &cfg.ram_mb.to_string());
    insert("np2kai_uselasthddmount", "OFF");
    insert(
        "np2kai_FastMC",
        if cfg.fast_memcheck { "ON" } else { "OFF" },
    );
    insert("np2kai_gdc", &cfg.gdc_type);
    insert(
        "np2kai_skipline",
        if cfg.skipline_revisions {
            "Full 255 lines"
        } else {
            "OFF"
        },
    );
    insert("np2kai_realpal", "OFF");
    insert("np2kai_lcd", "OFF");
    insert("np2kai_SNDboard", &cfg.sound_board);
    insert("np2kai_118ROM", "OFF");
    insert("np2kai_jast_snd", "OFF");
    insert("np2kai_usefmgen", "fmgen");
    insert("np2kai_xroll", "ON");
    insert("np2kai_volume_M", "100");
    insert("np2kai_volume_F", "64");
    insert("np2kai_volume_S", "64");
    insert("np2kai_volume_A", "64");
    insert("np2kai_volume_P", "92");
    insert("np2kai_volume_R", "64");
    insert("np2kai_volume_C", "128");
    insert(
        "np2kai_Seek_Snd",
        if cfg.floppy_seek_sound { "ON" } else { "OFF" },
    );
    insert(
        "np2kai_Seek_Vol",
        &cfg.floppy_seek_volume.clamp(0, 100).to_string(),
    );
    insert(
        "np2kai_BEEP_vol",
        &cfg.beep_volume.clamp(0, 3).to_string(),
    );
    insert("np2kai_PEGC", "ON");
    insert("np2kai_inputmouse", "ON");
    insert("np2kai_joymode", "OFF");
    insert("np2kai_stick2mouse", "OFF");

    vars
}

pub fn convert_rgb565_to_rgba(
    src_bytes: &[u8],
    width: u32,
    height: u32,
    pitch: usize,
) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let row_start = y * pitch;
        if row_start + w * 2 > src_bytes.len() {
            break;
        }
        let row = &src_bytes[row_start..row_start + w * 2];
        let dst_row = &mut out[y * w * 4..(y + 1) * w * 4];
        for (x, chunk) in row.chunks_exact(2).enumerate() {
            let px = u16::from_le_bytes([chunk[0], chunk[1]]);
            let r5 = ((px >> 11) & 0x1F) as u8;
            let g6 = ((px >> 5) & 0x3F) as u8;
            let b5 = (px & 0x1F) as u8;
            let dst = &mut dst_row[x * 4..x * 4 + 4];
            dst[0] = (r5 << 3) | (r5 >> 2);
            dst[1] = (g6 << 2) | (g6 >> 4);
            dst[2] = (b5 << 3) | (b5 >> 2);
            dst[3] = 255;
        }
    }
    out
}

pub fn convert_xrgb8888_to_rgba(
    src_bytes: &[u8],
    width: u32,
    height: u32,
    pitch: usize,
) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let row_start = y * pitch;
        if row_start + w * 4 > src_bytes.len() {
            break;
        }
        let row = &src_bytes[row_start..row_start + w * 4];
        let dst_row = &mut out[y * w * 4..(y + 1) * w * 4];
        for (x, chunk) in row.chunks_exact(4).enumerate() {
            let px = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let dst = &mut dst_row[x * 4..x * 4 + 4];
            dst[0] = ((px >> 16) & 0xFF) as u8;
            dst[1] = ((px >> 8) & 0xFF) as u8;
            dst[2] = (px & 0xFF) as u8;
            dst[3] = 255;
        }
    }
    out
}

pub fn convert_rgb1555_to_rgba(
    src_bytes: &[u8],
    width: u32,
    height: u32,
    pitch: usize,
) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let row_start = y * pitch;
        if row_start + w * 2 > src_bytes.len() {
            break;
        }
        let row = &src_bytes[row_start..row_start + w * 2];
        let dst_row = &mut out[y * w * 4..(y + 1) * w * 4];
        for (x, chunk) in row.chunks_exact(2).enumerate() {
            let px = u16::from_le_bytes([chunk[0], chunk[1]]);
            let r5 = ((px >> 10) & 0x1F) as u8;
            let g5 = ((px >> 5) & 0x1F) as u8;
            let b5 = (px & 0x1F) as u8;
            let dst = &mut dst_row[x * 4..x * 4 + 4];
            dst[0] = (r5 << 3) | (r5 >> 2);
            dst[1] = (g5 << 3) | (g5 >> 2);
            dst[2] = (b5 << 3) | (b5 >> 2);
            dst[3] = 255;
        }
    }
    out
}

unsafe extern "C" fn retro_environment_cb(cmd: c_uint, data: *mut c_void) -> bool {
    let Ok(mut guard) = ACTIVE_BRIDGE.lock() else {
        return false;
    };
    let Some(ctx) = guard.as_mut() else {
        return false;
    };

    match cmd {
        RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY => {
            if !data.is_null() {
                let out = data as *mut *const c_char;
                *out = ctx.sys_dir_cstr.as_ptr();
                return true;
            }
            false
        }
        RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY => {
            if !data.is_null() {
                let out = data as *mut *const c_char;
                *out = ctx.save_dir_cstr.as_ptr();
                return true;
            }
            false
        }
        RETRO_ENVIRONMENT_SET_PIXEL_FORMAT => {
            if !data.is_null() {
                let fmt = *(data as *const c_uint);
                ctx.pixel_format = match fmt {
                    0 => RetroPixelFormat::Rgb1555,
                    1 => RetroPixelFormat::Xrgb8888,
                    _ => RetroPixelFormat::Rgb565,
                };
                return true;
            }
            false
        }
        RETRO_ENVIRONMENT_GET_VARIABLE => {
            if !data.is_null() {
                let var = &mut *(data as *mut RetroVariable);
                if !var.key.is_null() {
                    if let Ok(key_str) = CStr::from_ptr(var.key).to_str() {
                        if let Some(val_cstr) = ctx.variables.get(key_str) {
                            var.value = val_cstr.as_ptr();
                            return true;
                        }
                    }
                }
                var.value = std::ptr::null();
            }
            false
        }
        RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE => {
            if !data.is_null() {
                let out = data as *mut bool;
                *out = ctx.variables_dirty;
                ctx.variables_dirty = false;
                return true;
            }
            false
        }
        RETRO_ENVIRONMENT_SET_DISK_CONTROL_INTERFACE => {
            if !data.is_null() {
                ctx.disk_control = Some(*(data as *const RetroDiskControlCallback));
            }
            true
        }
        RETRO_ENVIRONMENT_SET_VARIABLES
        | RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME
        | RETRO_ENVIRONMENT_SET_SERIALIZATION_QUIRKS
        | RETRO_ENVIRONMENT_SET_CORE_OPTIONS
        | RETRO_ENVIRONMENT_SET_CORE_OPTIONS_INTL
        | RETRO_ENVIRONMENT_SET_CORE_OPTIONS_DISPLAY => true,
        RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION => {
            if !data.is_null() {
                let out = data as *mut c_uint;
                *out = 1;
                return true;
            }
            false
        }
        RETRO_ENVIRONMENT_GET_LOG_INTERFACE => false,
        _ => false,
    }
}

unsafe extern "C" fn retro_video_refresh_cb(
    data: *const c_void,
    width: c_uint,
    height: c_uint,
    pitch: usize,
) {
    if data.is_null() || width == 0 || height == 0 || pitch == 0 {
        return;
    }

    let (pixel_format, frame_tx, event_tx, request_repaint, res_changed) = {
        let Ok(mut guard) = ACTIVE_BRIDGE.lock() else {
            return;
        };
        let Some(ctx) = guard.as_mut() else {
            return;
        };
        let new_res = (width, height);
        let changed = if ctx.last_resolution != new_res {
            ctx.last_resolution = new_res;
            true
        } else {
            false
        };
        (
            ctx.pixel_format,
            ctx.frame_tx.clone(),
            ctx.event_tx.clone(),
            ctx.request_repaint.clone(),
            changed,
        )
    };

    if res_changed {
        let _ = event_tx.send(Pc98Event::ResolutionChanged { width, height });
    }

    let total_src_bytes = (height as usize).saturating_mul(pitch);
    let src_slice = std::slice::from_raw_parts(data as *const u8, total_src_bytes);

    let rgba_data = match pixel_format {
        RetroPixelFormat::Rgb565 => convert_rgb565_to_rgba(src_slice, width, height, pitch),
        RetroPixelFormat::Xrgb8888 => convert_xrgb8888_to_rgba(src_slice, width, height, pitch),
        RetroPixelFormat::Rgb1555 => convert_rgb1555_to_rgba(src_slice, width, height, pitch),
    };

    let raw_frame = Arc::new(RawFrame {
        data: rgba_data,
        width,
        height,
        format: ffmpeg_next::format::Pixel::RGBA,
        color_range: ColorRange::Full,
        captured_at: crate::replay::now_us(),
        rate: crate::replay::config::Rate { num: 5642, den: 100 },
    });

    if let Ok(mut guard) = ACTIVE_BRIDGE.lock() {
        if let Some(ctx) = guard.as_mut() {
            ctx.last_frame = Some(raw_frame.clone());
        }
    }

    if frame_tx.try_send(raw_frame).is_ok() {
        (request_repaint)();
    }
}

unsafe extern "C" fn retro_audio_sample_cb(_left: i16, _right: i16) {}

unsafe extern "C" fn retro_audio_sample_batch_cb(data: *const i16, frames: usize) -> usize {
    if data.is_null() || frames == 0 {
        return 0;
    }

    let sample_count = frames.saturating_mul(2);
    let raw_samples = std::slice::from_raw_parts(data, sample_count);

    let mut f32_samples = Vec::with_capacity(sample_count);
    for &s in raw_samples {
        f32_samples.push((s as f32) / 32768.0);
    }

    let (audio_queue, peak_amplitude, audio_latency_ms, replay_audio) = {
        let Ok(mut guard) = ACTIVE_BRIDGE.lock() else {
            return frames;
        };
        let Some(ctx) = guard.as_mut() else {
            return frames;
        };
        ctx.seek_mixer.mix_into_stereo(&mut f32_samples);
        (
            ctx.audio_queue.clone(),
            ctx.peak_amplitude.clone(),
            ctx.audio_latency_ms.clone(),
            ctx.replay_audio.clone(),
        )
    };

    let mut max_abs = 0.0f32;
    for &val in &f32_samples {
        let abs = val.abs();
        if abs > max_abs {
            max_abs = abs;
        }
    }

    let peak_bits = (max_abs * 1000.0) as u64;
    peak_amplitude.fetch_max(peak_bits, Ordering::Relaxed);
    audio_latency_ms.store(18, Ordering::Relaxed);

    if let Ok(mut q) = audio_queue.lock() {
        // Cap queue to ~70ms at 44.1kHz stereo so audio never lags behind video
        const MAX_QUEUED_SAMPLES: usize = 44100 * 2 / 14;
        if q.samples.len() + f32_samples.len() > MAX_QUEUED_SAMPLES {
            let drop_n = (q.samples.len() + f32_samples.len()) - MAX_QUEUED_SAMPLES;
            let drop_even = ((drop_n / 2) * 2).min(q.samples.len());
            q.samples.drain(..drop_even);
        }
        q.samples.extend(&f32_samples);
    }

    replay_audio.observe_reactive(&f32_samples, PC98_AUDIO_SAMPLE_RATE, PC98_AUDIO_CHANNELS);

    if replay_audio.is_enabled() {
        let resampled = resample_stereo_44100_to_48000(&f32_samples);
        replay_audio.submit(
            &resampled,
            PC98_REPLAY_SAMPLE_RATE,
            PC98_AUDIO_CHANNELS,
            crate::replay::now_us(),
        );
    }

    frames
}

fn resample_stereo_44100_to_48000(input: &[f32]) -> Vec<f32> {
    let in_frames = input.len() / 2;
    if in_frames == 0 {
        return Vec::new();
    }
    let out_frames =
        (in_frames * PC98_REPLAY_SAMPLE_RATE as usize) / PC98_AUDIO_SAMPLE_RATE as usize;
    let mut out = Vec::with_capacity(out_frames * 2);
    let ratio = (in_frames as f32) / (out_frames.max(1) as f32);
    for i in 0..out_frames {
        let src_pos = (i as f32) * ratio;
        let idx0 = (src_pos.floor() as usize).min(in_frames - 1);
        let idx1 = (idx0 + 1).min(in_frames - 1);
        let frac = src_pos - (idx0 as f32);
        let l = input[idx0 * 2] * (1.0 - frac) + input[idx1 * 2] * frac;
        let r = input[idx0 * 2 + 1] * (1.0 - frac) + input[idx1 * 2 + 1] * frac;
        out.push(l);
        out.push(r);
    }
    out
}

unsafe extern "C" fn retro_input_poll_cb() {
    let Ok(mut guard) = ACTIVE_BRIDGE.lock() else {
        return;
    };
    let Some(ctx) = guard.as_mut() else {
        return;
    };

    let mut input_guard = ctx.input_state.lock().ok();
    if let Some(ref mut input) = input_guard {
        ctx.latched_keys = input.keys;
        for (key_code, ref mut rem) in input.pulse_keys.iter_mut() {
            if *rem > 0 && (*key_code as usize) < ctx.latched_keys.len() {
                ctx.latched_keys[*key_code as usize] = true;
                *rem -= 1;
            }
        }
        input.pulse_keys.retain(|(_, rem)| *rem > 0);

        ctx.latched_mouse_dx = input.mouse_dx.clamp(-127, 127) as i16;
        ctx.latched_mouse_dy = input.mouse_dy.clamp(-127, 127) as i16;
        input.mouse_dx -= ctx.latched_mouse_dx as i32;
        input.mouse_dy -= ctx.latched_mouse_dy as i32;

        ctx.latched_mouse_left = input.mouse_left;
        ctx.latched_mouse_right = input.mouse_right;
        ctx.latched_mouse_middle = input.mouse_middle;
    }
    drop(input_guard);
}

unsafe extern "C" fn retro_input_state_cb(
    _port: c_uint,
    device: c_uint,
    _index: c_uint,
    id: c_uint,
) -> i16 {
    let Ok(guard) = ACTIVE_BRIDGE.lock() else {
        return 0;
    };
    let Some(ctx) = guard.as_ref() else {
        return 0;
    };

    match device {
        RETRO_DEVICE_KEYBOARD => {
            if (id as usize) < ctx.latched_keys.len() && ctx.latched_keys[id as usize] {
                1
            } else {
                0
            }
        }
        RETRO_DEVICE_MOUSE => match id {
            RETRO_DEVICE_ID_MOUSE_X => ctx.latched_mouse_dx,
            RETRO_DEVICE_ID_MOUSE_Y => ctx.latched_mouse_dy,
            RETRO_DEVICE_ID_MOUSE_LEFT => i16::from(ctx.latched_mouse_left),
            RETRO_DEVICE_ID_MOUSE_RIGHT => i16::from(ctx.latched_mouse_right),
            RETRO_DEVICE_ID_MOUSE_MIDDLE => i16::from(ctx.latched_mouse_middle),
            _ => 0,
        },
        _ => 0,
    }
}

pub struct Pc98WorkerParams {
    pub config: Pc98Config,
    pub master_volume: f32,
    pub input_state: Arc<Mutex<Pc98InputState>>,
    pub frame_tx: Sender<Arc<RawFrame>>,
    pub event_tx: Sender<Pc98Event>,
    pub command_rx: Receiver<Pc98Command>,
    pub stop_requested: Arc<AtomicBool>,
    pub request_repaint: Arc<dyn Fn() + Send + Sync>,
    pub peak_amplitude: Arc<AtomicU64>,
    pub audio_latency_ms: Arc<AtomicU64>,
    pub replay_audio: Arc<crate::replay::AudioTap>,
}

pub fn run_pc98_worker(params: Pc98WorkerParams) -> Result<()> {
    let sys_dir = ensure_system_dir()?;
    let save_dir = saves_dir();
    let core_so_path = ensure_np2kai_core(Some(&params.event_tx))?;

    let _ = params.event_tx.send(Pc98Event::SyncProgress(
        "Booting NEC PC-9801VX hardware...".to_string(),
    ));

    // Copy the core .so to a unique temporary file so each session maps a fresh ELF image
    // with clean static globals (`firstcall = 1`, `PARAMCOUNT = 0`, `ARGUC = 0`).
    let temp_so = tempfile::Builder::new()
        .prefix("michadame-np2kai-")
        .suffix(".so")
        .tempfile()
        .context("Failed to create temporary NP2kai core copy")?;
    std::fs::copy(&core_so_path, temp_so.path())
        .with_context(|| format!("Failed to copy {:?} to temp path", core_so_path))?;

    let audio_queue = Arc::new(Mutex::new(Pc98AudioQueue {
        samples: VecDeque::with_capacity(8192),
    }));

    let audio_stream_opt = match crate::devices::audio::initialize_rodio_playback() {
        Ok((stream_guard, stream_handle)) => match Sink::try_new(&stream_handle) {
            Ok(sink) => {
                sink.set_volume(params.master_volume.clamp(0.0, 1.0));
                sink.append(Pc98AudioSource {
                    queue: audio_queue.clone(),
                    read_buf: VecDeque::with_capacity(128),
                });
                sink.play();
                Some((stream_guard, sink))
            }
            Err(e) => {
                tracing::warn!("Failed to create PC-98 audio sink: {e}");
                None
            }
        },
        Err(e) => {
            tracing::warn!("Failed to open audio output for PC-98: {e}");
            None
        }
    };

    let sys_dir_cstr = CString::new(sys_dir.to_string_lossy().as_bytes())
        .context("Invalid system dir path")?;
    let save_dir_cstr = CString::new(save_dir.to_string_lossy().as_bytes())
        .context("Invalid save dir path")?;

    {
        let mut guard = ACTIVE_BRIDGE.lock().unwrap();
        *guard = Some(Box::new(BridgeContext {
            sys_dir_cstr,
            save_dir_cstr,
            variables: build_core_variables(&params.config),
            variables_dirty: false,
            disk_control: None,
            pixel_format: RetroPixelFormat::Rgb565,
            frame_tx: params.frame_tx.clone(),
            event_tx: params.event_tx.clone(),
            request_repaint: params.request_repaint.clone(),
            last_resolution: (640, 400),
            last_frame: None,
            input_state: params.input_state.clone(),
            latched_keys: [false; 512],
            latched_mouse_dx: 0,
            latched_mouse_dy: 0,
            latched_mouse_left: false,
            latched_mouse_right: false,
            latched_mouse_middle: false,
            audio_queue,
            peak_amplitude: params.peak_amplitude,
            audio_latency_ms: params.audio_latency_ms,
            replay_audio: params.replay_audio,
            seek_mixer: FloppySeekMixer::new(
                params.config.floppy_seek_sound,
                params.config.floppy_seek_volume,
            ),
        }));
    }

    let result = unsafe {
        run_loaded_core(
            temp_so.path(),
            &params.config,
            &params.command_rx,
            &params.event_tx,
            &params.stop_requested,
            &params.input_state,
            audio_stream_opt.as_ref().map(|(_, sink)| sink),
        )
    };

    if let Ok(mut guard) = ACTIVE_BRIDGE.lock() {
        *guard = None;
    }

    result
}

unsafe fn mount_fdd_via_disk_control(drive: u8, path: Option<&Path>) -> bool {
    let dc = {
        let Ok(mut guard) = ACTIVE_BRIDGE.lock() else {
            return false;
        };
        let Some(ctx) = guard.as_mut() else {
            return false;
        };
        let drive_str = if drive == 0 { "FDD1" } else { "FDD2" };
        if let Ok(c_drv) = CString::new(drive_str) {
            ctx.variables.insert("np2kai_drive".to_string(), c_drv);
        }
        ctx.disk_control
    };
    let Some(dc) = dc else {
        return false;
    };
    let (Some(get_num), Some(add_img), Some(replace_img), Some(set_idx)) = (
        dc.get_num_images,
        dc.add_image_index,
        dc.replace_image_index,
        dc.set_image_index,
    ) else {
        return false;
    };

    if get_num() == 0 {
        add_img();
    }

    let path_str = path
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let Ok(c_path) = CString::new(path_str.as_bytes()) else {
        return false;
    };
    let info = RetroGameInfo {
        path: c_path.as_ptr(),
        data: std::ptr::null(),
        size: 0,
        meta: std::ptr::null(),
    };
    replace_img(0, &info);
    set_idx(0)
}

fn enqueue_dos_command(queue: &mut VecDeque<(u32, bool, u8, u8)>, cmd: &str) {
    for ch in cmd.chars() {
        let mapped: Option<(u32, bool, u8, u8)> = match ch {
            'a'..='z' | 'A'..='Z' => Some((ch.to_ascii_lowercase() as u32, false, 2, 4)),
            '0'..='9' => Some((ch as u32, false, 2, 4)),
            ':' => Some((super::input::RETROK_QUOTE, false, 2, 4)),
            ' ' => Some((super::input::RETROK_SPACE, false, 2, 4)),
            '/' => Some((super::input::RETROK_SLASH, false, 2, 4)),
            '-' => Some((super::input::RETROK_MINUS, false, 2, 4)),
            '.' => Some((super::input::RETROK_PERIOD, false, 2, 4)),
            '\\' => Some((super::input::RETROK_BACKSLASH, false, 2, 4)),
            '_' => Some((2, true, 2, 4)),
            '\n' => Some((super::input::RETROK_RETURN, false, 2, 12)),
            _ => None,
        };
        if let Some(entry) = mapped {
            queue.push_back(entry);
        }
    }
}

unsafe fn run_loaded_core(
    so_path: &Path,
    initial_config: &Pc98Config,
    command_rx: &Receiver<Pc98Command>,
    event_tx: &Sender<Pc98Event>,
    stop_requested: &AtomicBool,
    input_state: &Arc<Mutex<Pc98InputState>>,
    audio_sink: Option<&Sink>,
) -> Result<()> {
    let lib = libloading::Library::new(so_path)
        .with_context(|| format!("Failed to dlopen NP2kai core at {:?}", so_path))?;

    let retro_set_environment: libloading::Symbol<unsafe extern "C" fn(RetroEnvironmentFn)> =
        lib.get(b"retro_set_environment\0")?;
    let retro_set_video_refresh: libloading::Symbol<unsafe extern "C" fn(RetroVideoRefreshFn)> =
        lib.get(b"retro_set_video_refresh\0")?;
    let retro_set_audio_sample: libloading::Symbol<unsafe extern "C" fn(RetroAudioSampleFn)> =
        lib.get(b"retro_set_audio_sample\0")?;
    let retro_set_audio_sample_batch: libloading::Symbol<
        unsafe extern "C" fn(RetroAudioSampleBatchFn),
    > = lib.get(b"retro_set_audio_sample_batch\0")?;
    let retro_set_input_poll: libloading::Symbol<unsafe extern "C" fn(RetroInputPollFn)> =
        lib.get(b"retro_set_input_poll\0")?;
    let retro_set_input_state: libloading::Symbol<unsafe extern "C" fn(RetroInputStateFn)> =
        lib.get(b"retro_set_input_state\0")?;
    let retro_init: libloading::Symbol<unsafe extern "C" fn()> = lib.get(b"retro_init\0")?;
    let retro_deinit: libloading::Symbol<unsafe extern "C" fn()> = lib.get(b"retro_deinit\0")?;
    let retro_load_game: libloading::Symbol<unsafe extern "C" fn(*const RetroGameInfo) -> bool> =
        lib.get(b"retro_load_game\0")?;
    let retro_unload_game: libloading::Symbol<unsafe extern "C" fn()> =
        lib.get(b"retro_unload_game\0")?;
    let retro_get_system_av_info: libloading::Symbol<
        unsafe extern "C" fn(*mut RetroSystemAvInfo),
    > = lib.get(b"retro_get_system_av_info\0")?;
    let retro_run: libloading::Symbol<unsafe extern "C" fn()> = lib.get(b"retro_run\0")?;
    let retro_reset: libloading::Symbol<unsafe extern "C" fn()> = lib.get(b"retro_reset\0")?;
    let retro_serialize_size: libloading::Symbol<unsafe extern "C" fn() -> usize> =
        lib.get(b"retro_serialize_size\0")?;
    let retro_serialize: libloading::Symbol<
        unsafe extern "C" fn(*mut c_void, usize) -> bool,
    > = lib.get(b"retro_serialize\0")?;
    let retro_unserialize: libloading::Symbol<
        unsafe extern "C" fn(*const c_void, usize) -> bool,
    > = lib.get(b"retro_unserialize\0")?;
    let retro_get_memory_data: Option<
        libloading::Symbol<unsafe extern "C" fn(c_uint) -> *mut c_void>,
    > = lib.get(b"retro_get_memory_data\0").ok();

    retro_set_environment(retro_environment_cb);
    retro_set_video_refresh(retro_video_refresh_cb);
    retro_set_audio_sample(retro_audio_sample_cb);
    retro_set_audio_sample_batch(retro_audio_sample_batch_cb);
    retro_set_input_poll(retro_input_poll_cb);
    retro_set_input_state(retro_input_state_cb);

    retro_init();

    let boot_cmd_path = prepare_boot_cmd(initial_config)?;
    let boot_cmd_cstr = boot_cmd_path
        .as_ref()
        .and_then(|p| CString::new(p.to_string_lossy().as_bytes()).ok());

    let loaded = if let Some(ref c_path) = boot_cmd_cstr {
        let info = RetroGameInfo {
            path: c_path.as_ptr(),
            data: std::ptr::null(),
            size: 0,
            meta: std::ptr::null(),
        };
        retro_load_game(&info)
    } else {
        retro_load_game(std::ptr::null())
    };

    if !loaded {
        retro_deinit();
        anyhow::bail!("NP2kai retro_load_game returned false");
    }

    // Call 1 to retro_run() initializes pre_main(RPATH), update_variables(), and pccore_reset().
    retro_run();

    // Locate NP2kai's 1MB base RAM buffer (`mem`, symbol offset 0x1599260 relative to
    // `retro_get_memory_data` at 0x16d7a0) and verify the x86 reset vector `JMP FAR` (0xEA) at 0xFFFF0.
    let np2_elf_base: Option<usize> = retro_get_memory_data.as_ref().and_then(|sym| {
        let fn_addr = (**sym) as usize;
        if fn_addr <= 0x16d7a0 {
            return None;
        }
        let elf_base = fn_addr - 0x16d7a0;
        let candidate = (elf_base + 0x1599260) as *const u8;
        if *candidate.add(0xFFFF0) == 0xEA {
            Some(elf_base)
        } else {
            None
        }
    });
    let pc98_mem_base: Option<*const u8> =
        np2_elf_base.map(|elf_base| (elf_base + 0x1599260) as *const u8);

    // Hook NP2kai's `x11/soundmng.c` PCM driver (`snddrv` at 0x4f12c0 and `pcm_channel` at 0x4f1280).
    // Upstream `np2kai_libretro.so` compiles `soundmng.c` with `nosound_setup()`, leaving
    // `snddrv.pcmplay` (0x4f1300 -> 0xc28d0) and `snddrv.pcmstop` (0x4f1308 -> 0xc28f0) as no-op
    // stubs and `pcm_channel[0..2]` as NULL, which previously silenced FDC/FDD seek sounds.
    let pcm_channel_table: Option<*mut *mut Np2PcmChannel> = np2_elf_base.and_then(|elf_base| {
        let pcmplay_slot = (elf_base + 0x4f1300) as *mut usize;
        let pcmstop_slot = (elf_base + 0x4f1308) as *mut usize;
        let pcmvol_slot = (elf_base + 0x4f1310) as *mut usize;
        if *pcmplay_slot == elf_base + 0xc28d0 && *pcmstop_slot == elf_base + 0xc28f0 {
            let chan_table = (elf_base + 0x4f1280) as *mut *mut Np2PcmChannel;
            let vol = initial_config.floppy_seek_volume.clamp(0, 100) as c_int;
            for i in 0..3 {
                let chan_ptr = std::ptr::addr_of_mut!(NP2_PCM_CHANNELS[i]);
                (*chan_ptr).volume = vol;
                *chan_table.add(i) = chan_ptr;
            }
            *pcmplay_slot = np2_snddrv_pcmplay as *const () as usize;
            *pcmstop_slot = np2_snddrv_pcmstop as *const () as usize;
            *pcmvol_slot = np2_snddrv_pcmvolume as *const () as usize;
            Some(chan_table)
        } else {
            None
        }
    });

    let defer_fdd = super::library::should_defer_fdd_boot(initial_config);
    let mut active_config = initial_config.clone();
    let mut current_fdd1: Option<PathBuf> = {
        let s = initial_config.fdd1_path.trim();
        if !s.is_empty() && Path::new(s).exists() {
            Some(PathBuf::from(s))
        } else {
            None
        }
    };
    let mut current_fdd2: Option<PathBuf> = {
        let s = initial_config.fdd2_path.trim();
        if !s.is_empty() && Path::new(s).exists() {
            Some(PathBuf::from(s))
        } else {
            None
        }
    };

    // When HDD1 is the boot drive, wait until the PC-98 BIOS finishes its RAM check and selects
    // SASI HDD #0 (`mem[0x00584] == 0x80` + 20 frames, or 520 frames fallback) before inserting
    // FDD1/FDD2 so a non-system floppy never hijacks the boot order.
    let mut deferred_fdd_pending =
        defer_fdd && (current_fdd1.is_some() || current_fdd2.is_some());
    let mut hdd_boot_detected_frames: u32 = 0;
    let mut frames_since_boot: u32 = 0;

    let startup_cmd = super::library::detect_startup_command(initial_config);
    let mut startup_cmd_sent = false;
    let mut dos_prompt_frames: u32 = 0;
    let mut auto_type_queue: VecDeque<(u32, bool, u8, u8)> = VecDeque::new();

    let mut av_info: RetroSystemAvInfo = std::mem::zeroed();
    retro_get_system_av_info(&mut av_info);
    let width = av_info.geometry.base_width.max(640);
    let height = av_info.geometry.base_height.max(400);
    let fps = if av_info.timing.fps > 10.0 && av_info.timing.fps < 120.0 {
        av_info.timing.fps
    } else {
        56.42
    };

    let _ = event_tx.send(Pc98Event::Started { width, height, fps });

    let frame_duration = Duration::from_secs_f64(1.0 / fps);
    let mut next_frame_instant = Instant::now();

    while !stop_requested.load(Ordering::Relaxed) {
        frames_since_boot = frames_since_boot.saturating_add(1);

        if deferred_fdd_pending {
            if let Some(base) = pc98_mem_base {
                if *base.add(0x00584) == 0x80 {
                    hdd_boot_detected_frames += 1;
                }
            }
            if hdd_boot_detected_frames >= 20 || frames_since_boot >= 520 {
                if let Some(ref p1) = current_fdd1 {
                    mount_fdd_via_disk_control(0, Some(p1));
                }
                if let Some(ref p2) = current_fdd2 {
                    mount_fdd_via_disk_control(1, Some(p2));
                }
                deferred_fdd_pending = false;
            }
        } else if !startup_cmd_sent {
            if let (Some(ref cmd_str), Some(base)) = (&startup_cmd, pc98_mem_base) {
                let tvram = std::slice::from_raw_parts(base.add(0xA0000), 0x1E00);
                let has_idle_prompt = tvram
                    .windows(14)
                    .any(|w| w == b"A\0:\0\\\0>\0 \0 \0 \0")
                    || tvram.windows(10).any(|w| w == b"A\0>\0 \0 \0 \0");
                if has_idle_prompt {
                    dos_prompt_frames += 1;
                } else {
                    dos_prompt_frames = 0;
                }
                if dos_prompt_frames >= 25 {
                    enqueue_dos_command(&mut auto_type_queue, cmd_str);
                    startup_cmd_sent = true;
                }
            }
        }

        if let Some(front) = auto_type_queue.front_mut() {
            if front.2 > 0 {
                if let Ok(mut inp) = input_state.lock() {
                    if front.1 {
                        inp.pulse_key(super::input::RETROK_LSHIFT, 1);
                    }
                    inp.pulse_key(front.0, 1);
                }
                front.2 -= 1;
            } else if front.3 > 0 {
                front.3 -= 1;
            } else {
                auto_type_queue.pop_front();
            }
        }

        while let Ok(cmd) = command_rx.try_recv() {
            match cmd {
                Pc98Command::Stop => {
                    stop_requested.store(true, Ordering::Relaxed);
                }
                Pc98Command::Reset => {
                    if defer_fdd && (current_fdd1.is_some() || current_fdd2.is_some()) {
                        mount_fdd_via_disk_control(0, None);
                        mount_fdd_via_disk_control(1, None);
                        deferred_fdd_pending = true;
                    }
                    hdd_boot_detected_frames = 0;
                    frames_since_boot = 0;
                    startup_cmd_sent = false;
                    dos_prompt_frames = 0;
                    auto_type_queue.clear();
                    retro_reset();
                    let _ = event_tx.send(Pc98Event::Info("PC-98 system reset.".to_string()));
                }
                Pc98Command::SetVolume(vol) => {
                    if let Some(sink) = audio_sink {
                        sink.set_volume(vol.clamp(0.0, 1.0));
                    }
                }
                Pc98Command::MountFdd { drive, path } => {
                    if drive == 0 {
                        current_fdd1 = path.clone();
                    } else if drive == 1 {
                        current_fdd2 = path.clone();
                    }
                    if let Some(p) = path {
                        if mount_fdd_via_disk_control(drive, Some(&p)) {
                            let name = p
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| p.display().to_string());
                            let _ = event_tx.send(Pc98Event::Info(format!(
                                "Mounted FDD{}: {}",
                                drive + 1,
                                name
                            )));
                        }
                    } else if mount_fdd_via_disk_control(drive, None) {
                        let _ =
                            event_tx.send(Pc98Event::Info(format!("Ejected FDD{}", drive + 1)));
                    }
                }
                Pc98Command::TriggerMenu => {
                    if let Ok(mut inp) = input_state.lock() {
                        inp.pulse_key(RETROK_F12, 3);
                    }
                }
                Pc98Command::UpdateOptions(new_cfg) => {
                    if let Ok(mut guard) = ACTIVE_BRIDGE.lock() {
                        if let Some(ctx) = guard.as_mut() {
                            let prev_drive = ctx.variables.get("np2kai_drive").cloned();
                            ctx.variables = build_core_variables(&new_cfg);
                            if let Some(drv) = prev_drive {
                                ctx.variables.insert("np2kai_drive".to_string(), drv);
                            }
                            ctx.variables_dirty = true;
                            ctx.seek_mixer.set_config(
                                new_cfg.floppy_seek_sound,
                                new_cfg.floppy_seek_volume,
                            );
                        }
                    }
                    active_config = new_cfg;
                }
                Pc98Command::SaveState { slot } => {
                    let slot = slot.clamp(1, super::savestate::SAVE_SLOT_COUNT as u8);
                    let state_size = retro_serialize_size();
                    if state_size == 0 {
                        let _ = event_tx.send(Pc98Event::Info(
                            "PC-98 save state failed (serialize size is 0).".to_string(),
                        ));
                    } else {
                        let mut state_buf = vec![0u8; state_size];
                        if retro_serialize(state_buf.as_mut_ptr() as *mut c_void, state_size) {
                            let (last_frame, last_res) = ACTIVE_BRIDGE
                                .lock()
                                .ok()
                                .and_then(|g| {
                                    g.as_ref().map(|ctx| {
                                        (ctx.last_frame.clone(), ctx.last_resolution)
                                    })
                                })
                                .unwrap_or((None, (width, height)));
                            let (game_key, game_title) =
                                super::savestate::derive_pc98_game_key(&active_config);
                            let meta = super::savestate::SaveStateMeta {
                                slot,
                                created_at: super::savestate::current_unix_timestamp(),
                                system: "PC-98".to_string(),
                                game_key: game_key.clone(),
                                game_title,
                                fdd1_path: current_fdd1
                                    .as_ref()
                                    .map(|p| p.to_string_lossy().to_string())
                                    .unwrap_or_default(),
                                fdd2_path: current_fdd2
                                    .as_ref()
                                    .map(|p| p.to_string_lossy().to_string())
                                    .unwrap_or_default(),
                                hdd1_path: active_config.hdd1_path.trim().to_string(),
                                tape_path: String::new(),
                                resolution: last_res,
                                size_bytes: state_buf.len(),
                            };
                            match super::savestate::write_slot_files(
                                &super::savestate::pc98_states_root(),
                                &game_key,
                                &meta,
                                &state_buf,
                                last_frame.as_deref(),
                            ) {
                                Ok(()) => {
                                    let _ = event_tx.send(Pc98Event::StateSaved { slot, meta });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(Pc98Event::Info(format!(
                                        "Failed to write PC-98 slot {}: {e}",
                                        slot
                                    )));
                                }
                            }
                        } else {
                            let _ = event_tx.send(Pc98Event::Info(format!(
                                "PC-98 core failed to serialize state into slot {}.",
                                slot
                            )));
                        }
                    }
                }
                Pc98Command::LoadState { slot } => {
                    let slot = slot.clamp(1, super::savestate::SAVE_SLOT_COUNT as u8);
                    let (game_key, _) = super::savestate::derive_pc98_game_key(&active_config);
                    match super::savestate::read_slot_state(
                        &super::savestate::pc98_states_root(),
                        &game_key,
                        slot,
                    ) {
                        Ok((meta, state_bytes)) => {
                            if !meta.fdd1_path.is_empty() && Path::new(&meta.fdd1_path).exists() {
                                let p1 = PathBuf::from(&meta.fdd1_path);
                                if current_fdd1.as_ref() != Some(&p1) || deferred_fdd_pending {
                                    mount_fdd_via_disk_control(0, Some(&p1));
                                    current_fdd1 = Some(p1);
                                }
                            }
                            if !meta.fdd2_path.is_empty() && Path::new(&meta.fdd2_path).exists() {
                                let p2 = PathBuf::from(&meta.fdd2_path);
                                if current_fdd2.as_ref() != Some(&p2) || deferred_fdd_pending {
                                    mount_fdd_via_disk_control(1, Some(&p2));
                                    current_fdd2 = Some(p2);
                                }
                            }
                            deferred_fdd_pending = false;
                            startup_cmd_sent = true;
                            auto_type_queue.clear();
                            if let Ok(mut inp) = input_state.lock() {
                                inp.release_all();
                            }
                            if retro_unserialize(
                                state_bytes.as_ptr() as *const c_void,
                                state_bytes.len(),
                            ) {
                                if let Ok(mut guard) = ACTIVE_BRIDGE.lock() {
                                    if let Some(ctx) = guard.as_mut() {
                                        ctx.seek_mixer.stop(0);
                                        ctx.seek_mixer.stop(1);
                                        if let Ok(mut q) = ctx.audio_queue.lock() {
                                            q.samples.clear();
                                        }
                                    }
                                }
                                let _ = event_tx.send(Pc98Event::StateLoaded { slot, meta });
                            } else {
                                let _ = event_tx.send(Pc98Event::Info(format!(
                                    "PC-98 core rejected state data in slot {}.",
                                    slot
                                )));
                            }
                        }
                        Err(e) => {
                            let _ = event_tx.send(Pc98Event::Info(format!("{e}")));
                        }
                    }
                }
            }
        }

        if stop_requested.load(Ordering::Relaxed) {
            break;
        }

        retro_run();

        next_frame_instant += frame_duration;
        let now = Instant::now();
        if next_frame_instant > now {
            std::thread::sleep(next_frame_instant - now);
        } else if now - next_frame_instant > frame_duration * 3 {
            next_frame_instant = now;
        }
    }

    if let Some(chan_table) = pcm_channel_table {
        for i in 0..3 {
            *chan_table.add(i) = std::ptr::null_mut();
        }
    }

    retro_unload_game();
    retro_deinit();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_rgb565_to_rgba() {
        // Pure red (0xF800) and pure green (0x07E0)
        let src: [u8; 4] = [0x00, 0xF8, 0xE0, 0x07];
        let rgba = convert_rgb565_to_rgba(&src, 2, 1, 4);
        assert_eq!(rgba, vec![255, 0, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn test_convert_xrgb8888_to_rgba() {
        // 0x00112233 -> R=0x11, G=0x22, B=0x33
        let src: [u8; 4] = [0x33, 0x22, 0x11, 0x00];
        let rgba = convert_xrgb8888_to_rgba(&src, 1, 1, 4);
        assert_eq!(rgba, vec![0x11, 0x22, 0x33, 255]);
    }

    #[test]
    fn test_build_core_variables_defaults() {
        let cfg = Pc98Config::default();
        let vars = build_core_variables(&cfg);
        assert_eq!(
            vars.get("np2kai_model").unwrap().to_str().unwrap(),
            "PC-9801VX"
        );
        assert_eq!(
            vars.get("np2kai_SNDboard").unwrap().to_str().unwrap(),
            "PC9801-26K + 86"
        );
        assert_eq!(
            vars.get("np2kai_inputmouse").unwrap().to_str().unwrap(),
            "ON"
        );
    }

    #[test]
    fn test_embedded_floppy_seek_wavs_parse_and_mix() {
        let seek0 = parse_wav_mono_f32(super::super::core_sync::EMBEDDED_FDDSEEK_WAV)
            .expect("embedded fddseek.wav should parse");
        let seek1 = parse_wav_mono_f32(super::super::core_sync::EMBEDDED_FDDSEEK1_WAV)
            .expect("embedded fddseek1.wav should parse");
        assert_eq!(seek0.len(), 773);
        assert_eq!(seek1.len(), 3439);

        let mut mixer = FloppySeekMixer::new(true, 80);
        // Channel 0 (SOUND_PCMSEEK) expands 773 * 17 = 13,141 samples
        assert_eq!(mixer.channels[0].samples.len(), 773 * 17);
        assert_eq!(mixer.channels[1].samples.len(), 3439);

        // Trigger single-track step (channel 1) and verify non-zero audio is mixed into silence
        mixer.play(1, false);
        assert!(mixer.channels[1].playing);
        let mut stereo_buf = vec![0.0f32; 512];
        mixer.mix_into_stereo(&mut stereo_buf);
        let max_mixed = stereo_buf.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            max_mixed > 0.05,
            "Expected audible seek step waveform, got max_abs={max_mixed}"
        );
    }
}
