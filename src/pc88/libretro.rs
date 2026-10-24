use super::core_sync::{ensure_pc88_system_dir, ensure_quasi88_core, pc88_base_dir, pc88_saves_dir};
use super::{Pc88Command, Pc88Config, Pc88Event};
use crate::pc98::input::RETROK_F12;
use crate::pc98::libretro::{
    convert_rgb1555_to_rgba, convert_rgb565_to_rgba, convert_xrgb8888_to_rgba, FloppySeekMixer,
};
use crate::pc98::Pc98InputState;
use crate::video::types::{ColorRange, RawFrame};
use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender};
use libloading::{Library, Symbol};
use rodio::{Sink, Source};
use std::collections::{HashMap, VecDeque};
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RETRO_ENVIRONMENT_SET_ROTATION: c_uint = 1;
const RETRO_ENVIRONMENT_GET_OVERSCAN: c_uint = 2;
const RETRO_ENVIRONMENT_GET_CAN_DUPE: c_uint = 3;
const RETRO_ENVIRONMENT_SET_MESSAGE: c_uint = 6;
const RETRO_ENVIRONMENT_SET_PERFORMANCE_LEVEL: c_uint = 8;
const RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY: c_uint = 9;
const RETRO_ENVIRONMENT_SET_PIXEL_FORMAT: c_uint = 10;
const RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS: c_uint = 11;
const RETRO_ENVIRONMENT_SET_KEYBOARD_CALLBACK: c_uint = 12;
const RETRO_ENVIRONMENT_SET_DISK_CONTROL_INTERFACE: c_uint = 13;
const RETRO_ENVIRONMENT_GET_VARIABLE: c_uint = 15;
const RETRO_ENVIRONMENT_SET_VARIABLES: c_uint = 16;
const RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE: c_uint = 17;
const RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME: c_uint = 18;
const RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE: c_uint = 23;
const RETRO_ENVIRONMENT_GET_LOG_INTERFACE: c_uint = 27;
const RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY: c_uint = 31;
const RETRO_ENVIRONMENT_SET_SYSTEM_AV_INFO: c_uint = 32;
const RETRO_ENVIRONMENT_SET_CONTROLLER_INFO: c_uint = 35;
const RETRO_ENVIRONMENT_SET_GEOMETRY: c_uint = 37;
const RETRO_ENVIRONMENT_GET_LANGUAGE: c_uint = 39;
const RETRO_ENVIRONMENT_GET_INPUT_BITMASKS: c_uint = 51 | 0x10000;
const RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION: c_uint = 52;
const RETRO_ENVIRONMENT_SET_CORE_OPTIONS: c_uint = 53;
const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_INTL: c_uint = 54;
const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_DISPLAY: c_uint = 55;
const RETRO_ENVIRONMENT_GET_PREFERRED_HW_RENDER: c_uint = 56;
const RETRO_ENVIRONMENT_GET_DISK_CONTROL_INTERFACE_VERSION: c_uint = 57;
const RETRO_ENVIRONMENT_SET_DISK_CONTROL_EXT_INTERFACE: c_uint = 58;
const RETRO_ENVIRONMENT_SET_AUDIO_BUFFER_STATUS_CALLBACK: c_uint = 62;
const RETRO_ENVIRONMENT_SET_MINIMUM_AUDIO_LATENCY: c_uint = 63;
const RETRO_ENVIRONMENT_SET_FASTFORWARDING_OVERRIDE: c_uint = 64;
const RETRO_ENVIRONMENT_SET_CONTENT_INFO_OVERRIDE: c_uint = 65;
const RETRO_ENVIRONMENT_GET_GAME_INFO_EXT: c_uint = 66;
const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2: c_uint = 67;
const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2_INTL: c_uint = 68;
const RETRO_ENVIRONMENT_SET_CORE_OPTIONS_UPDATE_DISPLAY_CALLBACK: c_uint = 69;
const RETRO_ENVIRONMENT_SET_VARIABLE: c_uint = 70;

const RETRO_PIXEL_FORMAT_0RGB1555: c_uint = 0;
const RETRO_PIXEL_FORMAT_XRGB8888: c_uint = 1;
const RETRO_PIXEL_FORMAT_RGB565: c_uint = 2;

const RETRO_DEVICE_JOYPAD: c_uint = 1;
const RETRO_DEVICE_MOUSE: c_uint = 2;
const RETRO_DEVICE_KEYBOARD: c_uint = 3;

const RETRO_DEVICE_ID_JOYPAD_SELECT: c_uint = 2;

const RETRO_DEVICE_ID_MOUSE_X: c_uint = 0;
const RETRO_DEVICE_ID_MOUSE_Y: c_uint = 1;
const RETRO_DEVICE_ID_MOUSE_LEFT: c_uint = 2;
const RETRO_DEVICE_ID_MOUSE_RIGHT: c_uint = 3;
const RETRO_DEVICE_ID_MOUSE_MIDDLE: c_uint = 4;

const RETRO_MEMORY_SAVE_RAM: c_uint = 0;

const PC88_AUDIO_SAMPLE_RATE: u32 = 44_100;
const PC88_REPLAY_SAMPLE_RATE: u32 = 48_000;
const PC88_AUDIO_CHANNELS: u16 = 2;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct RetroVariable {
    key: *const c_char,
    value: *const c_char,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
struct RetroGameGeometry {
    base_width: c_uint,
    base_height: c_uint,
    max_width: c_uint,
    max_height: c_uint,
    aspect_ratio: f32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
struct RetroSystemTiming {
    fps: f64,
    sample_rate: f64,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
struct RetroSystemAvInfo {
    geometry: RetroGameGeometry,
    timing: RetroSystemTiming,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct RetroGameInfo {
    path: *const c_char,
    data: *const c_void,
    size: usize,
    meta: *const c_char,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct RetroRumbleInterface {
    set_rumble_state: unsafe extern "C" fn(port: c_uint, effect: c_uint, strength: u16) -> bool,
}

type RetroEnvironmentFn = unsafe extern "C" fn(cmd: c_uint, data: *mut c_void) -> bool;
type RetroVideoRefreshFn =
    unsafe extern "C" fn(data: *const c_void, width: c_uint, height: c_uint, pitch: usize);
type RetroAudioSampleFn = unsafe extern "C" fn(left: i16, right: i16);
type RetroAudioSampleBatchFn = unsafe extern "C" fn(data: *const i16, frames: usize) -> usize;
type RetroInputPollFn = unsafe extern "C" fn();
type RetroInputStateFn =
    unsafe extern "C" fn(port: c_uint, device: c_uint, index: c_uint, id: c_uint) -> i16;

unsafe extern "C" fn dummy_rumble_set(
    _port: c_uint,
    effect: c_uint,
    strength: u16,
) -> bool {
    if effect == 0 {
        with_bridge_ctx(|ctx| {
            if strength > 0 {
                if ctx.rumble_seek_frames == 0 {
                    ctx.seek_mixer.stop(1);
                    ctx.seek_mixer.play(1, false);
                    ctx.rumble_seek_frames = 1;
                } else {
                    ctx.rumble_seek_frames = ctx.rumble_seek_frames.saturating_add(1);
                    if ctx.rumble_seek_frames >= 2 {
                        ctx.seek_mixer.play(0, true);
                    }
                }
            } else if ctx.rumble_seek_frames > 0 {
                ctx.seek_mixer.stop(0);
                ctx.rumble_seek_frames = 0;
            }
        });
    }
    true
}

unsafe extern "C" fn dummy_env_cb(_cmd: c_uint, _data: *mut c_void) -> bool {
    true
}

struct Pc88AudioQueue {
    samples: VecDeque<f32>,
}

struct Pc88AudioSource {
    queue: Arc<Mutex<Pc88AudioQueue>>,
    read_buf: VecDeque<f32>,
}

impl Iterator for Pc88AudioSource {
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

impl Source for Pc88AudioSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        PC88_AUDIO_CHANNELS
    }

    fn sample_rate(&self) -> u32 {
        PC88_AUDIO_SAMPLE_RATE
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

struct LibretroBridgeContext {
    system_dir_c: CString,
    save_dir_c: CString,
    variables: HashMap<String, CString>,
    variables_dirty: bool,
    pixel_format: c_uint,
    av_info: RetroSystemAvInfo,
    last_resolution: (u32, u32),
    last_frame: Option<Arc<RawFrame>>,
    frame_tx: Sender<Arc<RawFrame>>,
    event_tx: Sender<Pc88Event>,
    request_repaint: Arc<dyn Fn() + Send + Sync>,
    audio_queue: Arc<Mutex<Pc88AudioQueue>>,
    peak_amplitude: Arc<AtomicU64>,
    audio_latency_ms: Arc<AtomicU64>,
    replay_audio: Arc<crate::replay::AudioTap>,
    input_state: Arc<Mutex<Pc98InputState>>,
    latched_keys: [bool; 512],
    latched_mouse_dx: i16,
    latched_mouse_dy: i16,
    latched_mouse_left: bool,
    latched_mouse_right: bool,
    latched_mouse_middle: bool,
    seek_mixer: FloppySeekMixer,
    rumble_seek_frames: u32,
}

static ACTIVE_PC88_BRIDGE: Mutex<Option<Box<LibretroBridgeContext>>> = Mutex::new(None);

fn with_bridge_ctx<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut LibretroBridgeContext) -> R,
{
    let mut guard = ACTIVE_PC88_BRIDGE.lock().ok()?;
    let ctx = guard.as_mut()?;
    Some(f(ctx))
}

pub fn populate_quasi88_variables(
    vars: &mut HashMap<String, CString>,
    config: &Pc88Config,
) {
    let mut set_var = |k: &str, v: &str| {
        if let Ok(c) = CString::new(v) {
            vars.insert(k.to_string(), c);
        }
    };

    set_var("q88_basic_mode", &config.basic_mode);
    set_var("q88_sub_cpu_mode", &config.sub_cpu_mode.to_string());
    set_var("q88_cpu_clock", &config.cpu_clock);
    set_var("q88_sound_board", &config.sound_board);
    set_var(
        "q88_use_fdc_wait",
        if config.fdc_wait { "enabled" } else { "disabled" },
    );
    set_var(
        "q88_pcg-8100",
        if config.pcg_8100 { "enabled" } else { "disabled" },
    );
    set_var("q88_screen_size", &config.screen_size);
    set_var(
        "q88_save_to_disk_image",
        if config.save_to_disk_image {
            "enabled"
        } else {
            "disabled"
        },
    );
    set_var(
        "q88_rumble",
        if config.floppy_seek_sound {
            "enabled"
        } else {
            "disabled"
        },
    );
}

unsafe extern "C" fn bridge_environment_cb(cmd: c_uint, data: *mut c_void) -> bool {
    let base_cmd = cmd & !0x10000;
    match cmd {
        RETRO_ENVIRONMENT_GET_OVERSCAN => {
            if !data.is_null() {
                *(data as *mut bool) = false;
            }
            true
        }
        RETRO_ENVIRONMENT_GET_CAN_DUPE => {
            if !data.is_null() {
                *(data as *mut bool) = true;
            }
            true
        }
        RETRO_ENVIRONMENT_SET_PERFORMANCE_LEVEL
        | RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS
        | RETRO_ENVIRONMENT_SET_KEYBOARD_CALLBACK
        | RETRO_ENVIRONMENT_SET_CONTROLLER_INFO
        | RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME
        | RETRO_ENVIRONMENT_SET_VARIABLES
        | RETRO_ENVIRONMENT_SET_CORE_OPTIONS
        | RETRO_ENVIRONMENT_SET_CORE_OPTIONS_INTL
        | RETRO_ENVIRONMENT_SET_CORE_OPTIONS_DISPLAY
        | RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2
        | RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2_INTL
        | RETRO_ENVIRONMENT_SET_CORE_OPTIONS_UPDATE_DISPLAY_CALLBACK
        | RETRO_ENVIRONMENT_SET_AUDIO_BUFFER_STATUS_CALLBACK
        | RETRO_ENVIRONMENT_SET_MINIMUM_AUDIO_LATENCY
        | RETRO_ENVIRONMENT_SET_FASTFORWARDING_OVERRIDE
        | RETRO_ENVIRONMENT_SET_CONTENT_INFO_OVERRIDE
        | RETRO_ENVIRONMENT_SET_ROTATION
        | RETRO_ENVIRONMENT_SET_MESSAGE
        | RETRO_ENVIRONMENT_SET_DISK_CONTROL_INTERFACE
        | RETRO_ENVIRONMENT_SET_DISK_CONTROL_EXT_INTERFACE => true,
        RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE => {
            if !data.is_null() {
                let rumble = data as *mut RetroRumbleInterface;
                (*rumble).set_rumble_state = dummy_rumble_set;
            }
            true
        }
        RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY => {
            with_bridge_ctx(|ctx| {
                if !data.is_null() {
                    *(data as *mut *const c_char) = ctx.system_dir_c.as_ptr();
                }
                true
            })
            .unwrap_or(false)
        }
        RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY => {
            with_bridge_ctx(|ctx| {
                if !data.is_null() {
                    *(data as *mut *const c_char) = ctx.save_dir_c.as_ptr();
                }
                true
            })
            .unwrap_or(false)
        }
        RETRO_ENVIRONMENT_SET_PIXEL_FORMAT => {
            if data.is_null() {
                return false;
            }
            let fmt = *(data as *const c_uint);
            with_bridge_ctx(|ctx| {
                ctx.pixel_format = fmt;
                true
            })
            .unwrap_or(false)
        }
        RETRO_ENVIRONMENT_GET_VARIABLE => {
            if data.is_null() {
                return false;
            }
            let var = &mut *(data as *mut RetroVariable);
            if var.key.is_null() {
                return false;
            }
            let key_str = CStr::from_ptr(var.key).to_string_lossy().into_owned();
            with_bridge_ctx(|ctx| {
                if let Some(val_c) = ctx.variables.get(&key_str) {
                    var.value = val_c.as_ptr();
                    true
                } else {
                    var.value = ptr::null();
                    false
                }
            })
            .unwrap_or(false)
        }
        RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE => {
            with_bridge_ctx(|ctx| {
                let dirty = ctx.variables_dirty;
                ctx.variables_dirty = false;
                if !data.is_null() {
                    *(data as *mut bool) = dirty;
                }
                true
            })
            .unwrap_or(false)
        }
        RETRO_ENVIRONMENT_SET_VARIABLE => {
            if data.is_null() {
                return true;
            }
            let var = &*(data as *const RetroVariable);
            if var.key.is_null() || var.value.is_null() {
                return false;
            }
            let k = CStr::from_ptr(var.key).to_string_lossy().into_owned();
            let v = CStr::from_ptr(var.value).to_string_lossy().into_owned();
            with_bridge_ctx(|ctx| {
                if let Ok(c) = CString::new(v) {
                    ctx.variables.insert(k, c);
                }
                true
            })
            .unwrap_or(false)
        }
        RETRO_ENVIRONMENT_GET_LOG_INTERFACE => {
            unsafe extern "C" fn dummy_log_printf(_level: c_uint, _fmt: *const c_char) {}
            if !data.is_null() {
                *(data as *mut usize) = dummy_log_printf as usize;
                true
            } else {
                false
            }
        }
        RETRO_ENVIRONMENT_SET_SYSTEM_AV_INFO => {
            if data.is_null() {
                return false;
            }
            let info = *(data as *const RetroSystemAvInfo);
            with_bridge_ctx(|ctx| {
                ctx.av_info = info;
                let w = info.geometry.base_width;
                let h = info.geometry.base_height;
                if w > 0 && h > 0 && (w, h) != ctx.last_resolution {
                    ctx.last_resolution = (w, h);
                    let _ = ctx.event_tx.send(Pc88Event::ResolutionChanged {
                        width: w,
                        height: h,
                    });
                }
                true
            })
            .unwrap_or(false)
        }
        RETRO_ENVIRONMENT_SET_GEOMETRY => {
            if data.is_null() {
                return false;
            }
            let geom = *(data as *const RetroGameGeometry);
            with_bridge_ctx(|ctx| {
                ctx.av_info.geometry = geom;
                let w = geom.base_width;
                let h = geom.base_height;
                if w > 0 && h > 0 && (w, h) != ctx.last_resolution {
                    ctx.last_resolution = (w, h);
                    let _ = ctx.event_tx.send(Pc88Event::ResolutionChanged {
                        width: w,
                        height: h,
                    });
                }
                true
            })
            .unwrap_or(false)
        }
        RETRO_ENVIRONMENT_GET_LANGUAGE => {
            if !data.is_null() {
                *(data as *mut c_uint) = 1; // RETRO_LANGUAGE_JAPANESE
            }
            true
        }
        RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION => {
            if !data.is_null() {
                *(data as *mut c_uint) = 0;
            }
            true
        }
        RETRO_ENVIRONMENT_GET_DISK_CONTROL_INTERFACE_VERSION => {
            if !data.is_null() {
                *(data as *mut c_uint) = 0;
            }
            true
        }
        RETRO_ENVIRONMENT_GET_PREFERRED_HW_RENDER => {
            if !data.is_null() {
                *(data as *mut c_uint) = 0;
            }
            true
        }
        RETRO_ENVIRONMENT_GET_INPUT_BITMASKS => false,
        RETRO_ENVIRONMENT_GET_GAME_INFO_EXT => false,
        _ => {
            if base_cmd == 51 {
                return false;
            }
            false
        }
    }
}

unsafe extern "C" fn bridge_video_refresh_cb(
    data: *const c_void,
    width: c_uint,
    height: c_uint,
    pitch: usize,
) {
    if data.is_null() || width == 0 || height == 0 || pitch == 0 {
        return;
    }

    let (pixel_format, frame_tx, event_tx, repaint, prev_res) = {
        let Ok(guard) = ACTIVE_PC88_BRIDGE.lock() else {
            return;
        };
        let Some(ctx) = guard.as_ref() else {
            return;
        };
        (
            ctx.pixel_format,
            ctx.frame_tx.clone(),
            ctx.event_tx.clone(),
            ctx.request_repaint.clone(),
            ctx.last_resolution,
        )
    };

    if prev_res != (width, height) {
        if let Ok(mut guard) = ACTIVE_PC88_BRIDGE.lock() {
            if let Some(ctx) = guard.as_mut() {
                ctx.last_resolution = (width, height);
            }
        }
        let _ = event_tx.send(Pc88Event::ResolutionChanged { width, height });
    }

    let total_src_bytes = pitch.saturating_mul(height as usize);
    let src_bytes = std::slice::from_raw_parts(data as *const u8, total_src_bytes);

    let rgba = match pixel_format {
        RETRO_PIXEL_FORMAT_XRGB8888 => convert_xrgb8888_to_rgba(src_bytes, width, height, pitch),
        RETRO_PIXEL_FORMAT_RGB565 => convert_rgb565_to_rgba(src_bytes, width, height, pitch),
        RETRO_PIXEL_FORMAT_0RGB1555 | _ => {
            convert_rgb1555_to_rgba(src_bytes, width, height, pitch)
        }
    };

    let raw_frame = Arc::new(RawFrame {
        data: rgba,
        width,
        height,
        format: ffmpeg_next::format::Pixel::RGBA,
        color_range: ColorRange::Full,
        captured_at: crate::replay::now_us(),
        rate: crate::replay::config::Rate {
            num: 5994,
            den: 100,
        },
    });

    if let Ok(mut guard) = ACTIVE_PC88_BRIDGE.lock() {
        if let Some(ctx) = guard.as_mut() {
            ctx.last_frame = Some(raw_frame.clone());
        }
    }

    if frame_tx.try_send(raw_frame).is_ok() {
        repaint();
    }
}

unsafe extern "C" fn bridge_audio_sample_cb(left: i16, right: i16) {
    let pair = [left, right];
    bridge_audio_sample_batch_cb(pair.as_ptr(), 1);
}

unsafe extern "C" fn bridge_audio_sample_batch_cb(data: *const i16, frames: usize) -> usize {
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
        let Ok(mut guard) = ACTIVE_PC88_BRIDGE.lock() else {
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
        const MAX_QUEUED_SAMPLES: usize = 44100 * 2 / 14;
        if q.samples.len() + f32_samples.len() > MAX_QUEUED_SAMPLES {
            let drop_n = (q.samples.len() + f32_samples.len()) - MAX_QUEUED_SAMPLES;
            let drop_even = ((drop_n / 2) * 2).min(q.samples.len());
            q.samples.drain(..drop_even);
        }
        q.samples.extend(&f32_samples);
    }

    if replay_audio.is_enabled() {
        let resampled = resample_stereo_44100_to_48000(&f32_samples);
        replay_audio.submit(
            &resampled,
            PC88_REPLAY_SAMPLE_RATE,
            PC88_AUDIO_CHANNELS,
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
        (in_frames * PC88_REPLAY_SAMPLE_RATE as usize) / PC88_AUDIO_SAMPLE_RATE as usize;
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

unsafe extern "C" fn bridge_input_poll_cb() {
    let Ok(mut guard) = ACTIVE_PC88_BRIDGE.lock() else {
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

unsafe extern "C" fn bridge_input_state_cb(
    _port: c_uint,
    device: c_uint,
    _index: c_uint,
    id: c_uint,
) -> i16 {
    let Ok(guard) = ACTIVE_PC88_BRIDGE.lock() else {
        return 0;
    };
    let Some(ctx) = guard.as_ref() else {
        return 0;
    };

    match device & 0xFF {
        RETRO_DEVICE_JOYPAD => {
            if id == RETRO_DEVICE_ID_JOYPAD_SELECT
                && (RETROK_F12 as usize) < ctx.latched_keys.len()
                && ctx.latched_keys[RETROK_F12 as usize]
            {
                1
            } else {
                0
            }
        }
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

/// Parameters required to spawn the PC-88 Libretro worker thread.
pub struct Pc88WorkerParams {
    pub config: Pc88Config,
    pub active_game_floppies: Vec<PathBuf>,
    pub master_volume: f32,
    pub input_state: Arc<Mutex<Pc98InputState>>,
    pub frame_tx: Sender<Arc<RawFrame>>,
    pub event_tx: Sender<Pc88Event>,
    pub command_rx: Receiver<Pc88Command>,
    pub stop_requested: Arc<AtomicBool>,
    pub request_repaint: Arc<dyn Fn() + Send + Sync>,
    pub peak_amplitude: Arc<AtomicU64>,
    pub audio_latency_ms: Arc<AtomicU64>,
    pub replay_audio: Arc<crate::replay::AudioTap>,
}

struct BootMediaPlan {
    content_path: Option<PathBuf>,
    disk_list: Vec<PathBuf>,
}

fn prepare_pc88_boot_media(
    config: &Pc88Config,
    active_game_floppies: &[PathBuf],
) -> Result<BootMediaPlan> {
    let fdd1 = config.fdd1_path.trim();
    let fdd2 = config.fdd2_path.trim();
    let tape = config.tape_path.trim();

    if fdd1.is_empty() && fdd2.is_empty() && !tape.is_empty() {
        let tape_p = PathBuf::from(tape);
        if tape_p.is_file() {
            return Ok(BootMediaPlan {
                content_path: Some(tape_p),
                disk_list: Vec::new(),
            });
        }
    }

    let mut disk_list: Vec<PathBuf> = Vec::new();
    let mut push_unique = |p: PathBuf| {
        if p.is_file() && !disk_list.contains(&p) && disk_list.len() < 16 {
            disk_list.push(p);
        }
    };

    if !fdd1.is_empty() {
        push_unique(PathBuf::from(fdd1));
    }
    if !fdd2.is_empty() {
        push_unique(PathBuf::from(fdd2));
    }
    for extra in active_game_floppies {
        push_unique(extra.clone());
    }

    if disk_list.is_empty() {
        return Ok(BootMediaPlan {
            content_path: None,
            disk_list,
        });
    }

    let m3u_path = pc88_base_dir().join("boot.m3u");
    let m3u_body: String = disk_list
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&m3u_path, m3u_body)
        .with_context(|| format!("Failed to write PC-88 playlist {}", m3u_path.display()))?;

    Ok(BootMediaPlan {
        content_path: Some(m3u_path),
        disk_list,
    })
}

/// Entry point for the background PC-88 emulator thread.
pub fn run_pc88_worker(params: Pc88WorkerParams) -> Result<()> {
    let sys_dir = ensure_pc88_system_dir(Some(&params.config.games_dir))?;
    let save_dir = pc88_saves_dir();
    let core_so_path = ensure_quasi88_core(Some(&params.event_tx))?;

    let _ = params.event_tx.send(Pc88Event::SyncProgress(
        "Booting NEC PC-8801 hardware...".to_string(),
    ));

    // Copy the core .so to a unique temporary file so each session maps a fresh ELF image
    // with clean static globals.
    let temp_so = tempfile::Builder::new()
        .prefix("michadame-quasi88-")
        .suffix(".so")
        .tempfile()
        .context("Failed to create temporary Quasi88 core copy")?;
    fs::copy(&core_so_path, temp_so.path())
        .with_context(|| format!("Failed to copy {:?} to temp path", core_so_path))?;

    let boot_plan = prepare_pc88_boot_media(&params.config, &params.active_game_floppies)?;

    let audio_queue = Arc::new(Mutex::new(Pc88AudioQueue {
        samples: VecDeque::with_capacity(8192),
    }));

    let audio_stream_opt = match crate::devices::audio::initialize_rodio_playback() {
        Ok((stream_guard, stream_handle)) => match Sink::try_new(&stream_handle) {
            Ok(sink) => {
                sink.set_volume(params.master_volume.clamp(0.0, 1.0));
                sink.append(Pc88AudioSource {
                    queue: audio_queue.clone(),
                    read_buf: VecDeque::with_capacity(128),
                });
                sink.play();
                Some((stream_guard, sink))
            }
            Err(e) => {
                tracing::warn!("Failed to create PC-88 audio sink: {e}");
                None
            }
        },
        Err(e) => {
            tracing::warn!("Failed to open audio output for PC-88: {e}");
            None
        }
    };

    let mut variables = HashMap::new();
    populate_quasi88_variables(&mut variables, &params.config);

    {
        let mut guard = ACTIVE_PC88_BRIDGE.lock().unwrap();
        *guard = Some(Box::new(LibretroBridgeContext {
            system_dir_c: CString::new(sys_dir.to_string_lossy().as_bytes())?,
            save_dir_c: CString::new(save_dir.to_string_lossy().as_bytes())?,
            variables,
            variables_dirty: false,
            pixel_format: RETRO_PIXEL_FORMAT_RGB565,
            av_info: RetroSystemAvInfo {
                geometry: RetroGameGeometry {
                    base_width: 640,
                    base_height: 400,
                    max_width: 640,
                    max_height: 400,
                    aspect_ratio: 1.6,
                },
                timing: RetroSystemTiming {
                    fps: 59.94,
                    sample_rate: 44100.0,
                },
            },
            last_resolution: (640, 400),
            last_frame: None,
            frame_tx: params.frame_tx.clone(),
            event_tx: params.event_tx.clone(),
            request_repaint: params.request_repaint.clone(),
            audio_queue,
            peak_amplitude: params.peak_amplitude,
            audio_latency_ms: params.audio_latency_ms,
            replay_audio: params.replay_audio,
            input_state: params.input_state.clone(),
            latched_keys: [false; 512],
            latched_mouse_dx: 0,
            latched_mouse_dy: 0,
            latched_mouse_left: false,
            latched_mouse_right: false,
            latched_mouse_middle: false,
            seek_mixer: FloppySeekMixer::new(
                params.config.floppy_seek_sound,
                params.config.floppy_seek_volume,
            ),
            rumble_seek_frames: 0,
        }));
    }

    let result = unsafe {
        run_loaded_core(
            temp_so.path(),
            boot_plan,
            &params.config,
            &params.command_rx,
            &params.event_tx,
            &params.stop_requested,
            audio_stream_opt.as_ref().map(|(_, sink)| sink),
        )
    };

    if let Ok(mut guard) = ACTIVE_PC88_BRIDGE.lock() {
        *guard = None;
    }

    result
}

unsafe fn run_loaded_core(
    core_so_path: &Path,
    mut boot_plan: BootMediaPlan,
    initial_config: &Pc88Config,
    command_rx: &Receiver<Pc88Command>,
    event_tx: &Sender<Pc88Event>,
    stop_requested: &Arc<AtomicBool>,
    audio_sink: Option<&Sink>,
) -> Result<()> {
    let lib = Library::new(core_so_path)
        .with_context(|| format!("Failed to dlopen {}", core_so_path.display()))?;

    let retro_set_environment: Symbol<unsafe extern "C" fn(RetroEnvironmentFn)> =
        lib.get(b"retro_set_environment\0")?;
    let retro_set_video_refresh: Symbol<unsafe extern "C" fn(RetroVideoRefreshFn)> =
        lib.get(b"retro_set_video_refresh\0")?;
    let retro_set_audio_sample: Symbol<unsafe extern "C" fn(RetroAudioSampleFn)> =
        lib.get(b"retro_set_audio_sample\0")?;
    let retro_set_audio_sample_batch: Symbol<unsafe extern "C" fn(RetroAudioSampleBatchFn)> =
        lib.get(b"retro_set_audio_sample_batch\0")?;
    let retro_set_input_poll: Symbol<unsafe extern "C" fn(RetroInputPollFn)> =
        lib.get(b"retro_set_input_poll\0")?;
    let retro_set_input_state: Symbol<unsafe extern "C" fn(RetroInputStateFn)> =
        lib.get(b"retro_set_input_state\0")?;

    let retro_init: Symbol<unsafe extern "C" fn()> = lib.get(b"retro_init\0")?;
    let retro_deinit: Symbol<unsafe extern "C" fn()> = lib.get(b"retro_deinit\0")?;
    let retro_get_system_av_info: Symbol<unsafe extern "C" fn(*mut RetroSystemAvInfo)> =
        lib.get(b"retro_get_system_av_info\0")?;
    let retro_load_game: Symbol<unsafe extern "C" fn(*const RetroGameInfo) -> bool> =
        lib.get(b"retro_load_game\0")?;
    let retro_unload_game: Symbol<unsafe extern "C" fn()> = lib.get(b"retro_unload_game\0")?;
    let retro_run: Symbol<unsafe extern "C" fn()> = lib.get(b"retro_run\0")?;
    let retro_reset: Symbol<unsafe extern "C" fn()> = lib.get(b"retro_reset\0")?;
    let retro_serialize_size: Symbol<unsafe extern "C" fn() -> usize> =
        lib.get(b"retro_serialize_size\0")?;
    let retro_serialize: Symbol<unsafe extern "C" fn(*mut c_void, usize) -> bool> =
        lib.get(b"retro_serialize\0")?;
    let retro_unserialize: Symbol<unsafe extern "C" fn(*const c_void, usize) -> bool> =
        lib.get(b"retro_unserialize\0")?;

    let retro_get_memory_data: Option<Symbol<unsafe extern "C" fn(c_uint) -> *mut c_void>> =
        lib.get(b"retro_get_memory_data\0").ok();
    let retro_get_memory_size: Option<Symbol<unsafe extern "C" fn(c_uint) -> usize>> =
        lib.get(b"retro_get_memory_size\0").ok();

    let retro_disks_append: Option<Symbol<unsafe extern "C" fn(*const c_char) -> c_int>> =
        lib.get(b"retro_disks_append\0").ok();
    let retro_disks_start: Option<Symbol<unsafe extern "C" fn(RetroEnvironmentFn, c_int)>> =
        lib.get(b"retro_disks_start\0").ok();
    let retro_disks_cycle: Option<Symbol<unsafe extern "C" fn(RetroEnvironmentFn, c_int)>> =
        lib.get(b"retro_disks_cycle\0").ok();
    let retro_disks_set: Option<Symbol<unsafe extern "C" fn(RetroEnvironmentFn)>> =
        lib.get(b"retro_disks_set\0").ok();

    retro_set_environment(bridge_environment_cb);
    retro_set_video_refresh(bridge_video_refresh_cb);
    retro_set_audio_sample(bridge_audio_sample_cb);
    retro_set_audio_sample_batch(bridge_audio_sample_batch_cb);
    retro_set_input_poll(bridge_input_poll_cb);
    retro_set_input_state(bridge_input_state_cb);

    retro_init();

    let content_path_c = match &boot_plan.content_path {
        Some(p) => Some(CString::new(p.to_string_lossy().as_bytes())?),
        None => None,
    };
    let game_info = RetroGameInfo {
        path: content_path_c
            .as_ref()
            .map_or(ptr::null(), |c| c.as_ptr()),
        data: ptr::null(),
        size: 0,
        meta: ptr::null(),
    };

    if !retro_load_game(&game_info) {
        retro_deinit();
        anyhow::bail!("Quasi88 retro_load_game returned false");
    }

    let mount_fdd_live = |drive: usize,
                          target_path: Option<&Path>,
                          disk_list: &mut Vec<PathBuf>|
     -> bool {
        let (Some(start_fn), Some(cycle_fn), Some(set_fn)) =
            (&retro_disks_start, &retro_disks_cycle, &retro_disks_set)
        else {
            return false;
        };

        let target_idx = if let Some(p) = target_path {
            if let Some(pos) = disk_list.iter().position(|x| x == p) {
                Some(pos)
            } else if disk_list.len() < 16 {
                if let Some(append_fn) = &retro_disks_append {
                    if let Ok(c_path) = CString::new(p.to_string_lossy().as_bytes()) {
                        if unsafe { append_fn(c_path.as_ptr()) } != 0 {
                            disk_list.push(p.to_path_buf());
                            Some(disk_list.len() - 1)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        if target_path.is_some() && target_idx.is_none() {
            return false;
        }

        let drive_inv: c_int = if drive == 0 { 1 } else { 0 };
        unsafe {
            start_fn(dummy_env_cb, drive_inv);
            for _ in 0..16 {
                cycle_fn(dummy_env_cb, 0);
            }
            if let Some(idx) = target_idx {
                for _ in 0..=idx {
                    cycle_fn(dummy_env_cb, 1);
                }
            }
            set_fn(dummy_env_cb);
        }
        true
    };

    // Note: retro_load_game(&game_info) with boot.m3u already populates the
    // internal disk list and mounts index 0 in Drive 1 and index 1 in Drive 2
    // before calling quasi88_init() -> quasi88_reset().
    // We only need mount_fdd_live if fdd1_path was empty ( ejected ) while fdd2_path was set.
    if !boot_plan.disk_list.is_empty() && initial_config.fdd1_path.trim().is_empty() {
        let fdd2_opt = if initial_config.fdd2_path.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(initial_config.fdd2_path.trim()))
        };
        mount_fdd_live(0, None, &mut boot_plan.disk_list);
        mount_fdd_live(1, fdd2_opt.as_deref(), &mut boot_plan.disk_list);
    }

    let sram_path = pc88_saves_dir().join("pc88_system.srm");
    if let (Some(get_data), Some(get_size)) = (&retro_get_memory_data, &retro_get_memory_size) {
        let size = get_size(RETRO_MEMORY_SAVE_RAM);
        let ptr = get_data(RETRO_MEMORY_SAVE_RAM) as *mut u8;
        if size > 0 && !ptr.is_null() && sram_path.is_file() {
            if let Ok(bytes) = fs::read(&sram_path) {
                let copy_len = bytes.len().min(size);
                ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, copy_len);
            }
        }
    }

    let mut active_config = initial_config.clone();
    let mut current_fdd1: Option<PathBuf> = {
        let s = initial_config.fdd1_path.trim();
        if !s.is_empty() && Path::new(s).is_file() {
            Some(PathBuf::from(s))
        } else {
            None
        }
    };
    let mut current_fdd2: Option<PathBuf> = {
        let s = initial_config.fdd2_path.trim();
        if !s.is_empty() && Path::new(s).is_file() {
            Some(PathBuf::from(s))
        } else {
            None
        }
    };

    let mut av_info = RetroSystemAvInfo::default();
    retro_get_system_av_info(&mut av_info);
    with_bridge_ctx(|ctx| {
        ctx.av_info = av_info;
    });

    // Execute one frame before emitting Started so the core is fully stepped and ready
    // for immediate save/load state commands on boot.
    retro_run();

    let fps = if av_info.timing.fps > 10.0 && av_info.timing.fps < 240.0 {
        av_info.timing.fps
    } else {
        59.94
    };
    let width = av_info.geometry.base_width.max(320);
    let height = av_info.geometry.base_height.max(200);

    let _ = event_tx.send(Pc88Event::Started { width, height, fps });

    let frame_period = Duration::from_secs_f64(1.0 / fps);
    let mut next_frame_deadline = Instant::now() + frame_period;

    while !stop_requested.load(Ordering::Relaxed) {
        while let Ok(cmd) = command_rx.try_recv() {
            match cmd {
                Pc88Command::Stop => {
                    stop_requested.store(true, Ordering::SeqCst);
                }
                Pc88Command::Reset => {
                    retro_reset();
                    let _ = event_tx.send(Pc88Event::Info("PC-88 system reset.".to_string()));
                }
                Pc88Command::TriggerMenu => {
                    with_bridge_ctx(|ctx| {
                        if let Ok(mut guard) = ctx.input_state.lock() {
                            guard.pulse_keys.push((RETROK_F12, 4));
                        }
                    });
                }
                Pc88Command::SetVolume(vol) => {
                    if let Some(sink) = audio_sink {
                        sink.set_volume(vol.clamp(0.0, 1.0));
                    }
                }
                Pc88Command::MountFdd { drive, path } => {
                    super::library::sync_back_multi_d88_files(&boot_plan.disk_list);
                    if drive == 0 {
                        current_fdd1 = path.clone();
                    } else if drive == 1 {
                        current_fdd2 = path.clone();
                    }
                    if mount_fdd_live(drive as usize, path.as_deref(), &mut boot_plan.disk_list) {
                        if let Some(p) = path {
                            let name = p
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("disk");
                            let _ = event_tx.send(Pc88Event::Info(format!(
                                "Mounted '{}' in FDD{}",
                                name,
                                drive + 1
                            )));
                        } else {
                            let _ = event_tx
                                .send(Pc88Event::Info(format!("Ejected FDD{}", drive + 1)));
                        }
                    } else {
                        let _ = event_tx.send(Pc88Event::Info(format!(
                            "Could not swap FDD{} disk",
                            drive + 1
                        )));
                    }
                }
                Pc88Command::UpdateOptions(new_cfg) => {
                    with_bridge_ctx(|ctx| {
                        populate_quasi88_variables(&mut ctx.variables, &new_cfg);
                        ctx.variables_dirty = true;
                        ctx.seek_mixer.set_config(
                            new_cfg.floppy_seek_sound,
                            new_cfg.floppy_seek_volume,
                        );
                    });
                    active_config = new_cfg;
                }
                Pc88Command::SaveState { slot } => {
                    let slot = slot.clamp(1, crate::pc98::savestate::SAVE_SLOT_COUNT as u8);
                    super::library::sync_back_multi_d88_files(&boot_plan.disk_list);
                    let state_size = retro_serialize_size();
                    if state_size == 0 {
                        let _ = event_tx.send(Pc88Event::Info(
                            "PC-88 save state failed (serialize size is 0).".to_string(),
                        ));
                    } else {
                        let mut state_buf = vec![0u8; state_size];
                        if retro_serialize(state_buf.as_mut_ptr() as *mut c_void, state_size) {
                            let (last_frame, last_res) = with_bridge_ctx(|ctx| {
                                (ctx.last_frame.clone(), ctx.last_resolution)
                            })
                            .unwrap_or((None, (width, height)));
                            let (game_key, game_title) =
                                crate::pc98::savestate::derive_pc88_game_key(&active_config);
                            let meta = crate::pc98::savestate::SaveStateMeta {
                                slot,
                                created_at: crate::pc98::savestate::current_unix_timestamp(),
                                system: "PC-88".to_string(),
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
                                hdd1_path: String::new(),
                                tape_path: active_config.tape_path.trim().to_string(),
                                resolution: last_res,
                                size_bytes: state_buf.len(),
                            };
                            match crate::pc98::savestate::write_slot_files(
                                &crate::pc98::savestate::pc88_states_root(),
                                &game_key,
                                &meta,
                                &state_buf,
                                last_frame.as_deref(),
                            ) {
                                Ok(()) => {
                                    let _ = event_tx.send(Pc88Event::StateSaved { slot, meta });
                                }
                                Err(e) => {
                                    let _ = event_tx.send(Pc88Event::Info(format!(
                                        "Failed to write PC-88 slot {}: {e}",
                                        slot
                                    )));
                                }
                            }
                        } else {
                            let _ = event_tx.send(Pc88Event::Info(format!(
                                "PC-88 core failed to serialize state into slot {}.",
                                slot
                            )));
                        }
                    }
                }
                Pc88Command::LoadState { slot } => {
                    let slot = slot.clamp(1, crate::pc98::savestate::SAVE_SLOT_COUNT as u8);
                    let (game_key, _) =
                        crate::pc98::savestate::derive_pc88_game_key(&active_config);
                    match crate::pc98::savestate::read_slot_state(
                        &crate::pc98::savestate::pc88_states_root(),
                        &game_key,
                        slot,
                    ) {
                        Ok((meta, state_bytes)) => {
                            if !meta.fdd1_path.is_empty() && Path::new(&meta.fdd1_path).is_file() {
                                let p1 = PathBuf::from(&meta.fdd1_path);
                                if current_fdd1.as_ref() != Some(&p1) {
                                    mount_fdd_live(0, Some(&p1), &mut boot_plan.disk_list);
                                    current_fdd1 = Some(p1);
                                }
                            }
                            if !meta.fdd2_path.is_empty() && Path::new(&meta.fdd2_path).is_file() {
                                let p2 = PathBuf::from(&meta.fdd2_path);
                                if current_fdd2.as_ref() != Some(&p2) {
                                    mount_fdd_live(1, Some(&p2), &mut boot_plan.disk_list);
                                    current_fdd2 = Some(p2);
                                }
                            }
                            if retro_unserialize(
                                state_bytes.as_ptr() as *const c_void,
                                state_bytes.len(),
                            ) {
                                with_bridge_ctx(|ctx| {
                                    ctx.rumble_seek_frames = 0;
                                    ctx.seek_mixer.stop(0);
                                    ctx.seek_mixer.stop(1);
                                    if let Ok(mut inp) = ctx.input_state.lock() {
                                        inp.release_all();
                                    }
                                    if let Ok(mut q) = ctx.audio_queue.lock() {
                                        q.samples.clear();
                                    }
                                });
                                let _ = event_tx.send(Pc88Event::StateLoaded { slot, meta });
                            } else {
                                let _ = event_tx.send(Pc88Event::Info(format!(
                                    "PC-88 core rejected state data in slot {}.",
                                    slot
                                )));
                            }
                        }
                        Err(e) => {
                            let _ = event_tx.send(Pc88Event::Info(format!("{e}")));
                        }
                    }
                }
            }
        }

        if stop_requested.load(Ordering::Relaxed) {
            break;
        }

        retro_run();

        let now = Instant::now();
        if next_frame_deadline > now {
            std::thread::sleep(next_frame_deadline - now);
            next_frame_deadline += frame_period;
        } else {
            next_frame_deadline = now + frame_period;
        }
    }

    if let (Some(get_data), Some(get_size)) = (&retro_get_memory_data, &retro_get_memory_size) {
        let size = get_size(RETRO_MEMORY_SAVE_RAM);
        let ptr = get_data(RETRO_MEMORY_SAVE_RAM) as *const u8;
        if size > 0 && !ptr.is_null() {
            let slice = std::slice::from_raw_parts(ptr, size);
            if slice.iter().any(|&b| b != 0) {
                let _ = fs::write(&sram_path, slice);
            }
        }
    }

    retro_unload_game();
    retro_deinit();

    super::library::sync_back_multi_d88_files(&boot_plan.disk_list);

    Ok(())
}

