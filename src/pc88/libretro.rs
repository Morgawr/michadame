use super::core_sync::{
    ensure_pc88_system_dir, ensure_quasi88_core, pc88_base_dir, pc88_saves_dir,
};
use super::{Pc88Command, Pc88Config, Pc88Event};
use crate::pc98::input::RETROK_F12;
use crate::pc98::libretro::{
    convert_rgb1555_to_rgba, convert_rgb565_to_rgba, convert_xrgb8888_to_rgba,
    ContinuousStereoResampler, EmulatorLifecycleState, FloppySeekMixer,
};
use crate::pc98::Pc98InputState;
use crate::video::types::{ColorRange, RawFrame};
use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender};
use libloading::{Library, Symbol};
use rodio::Source;
use rtrb::{Consumer, Producer, RingBuffer};
use std::cell::{Cell, UnsafeCell};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
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

unsafe extern "C" fn dummy_rumble_set(_port: c_uint, effect: c_uint, strength: u16) -> bool {
    if effect == 0 {
        let rt_ptr = CURRENT_PC88_REALTIME_STATE.with(|c| c.get());
        if !rt_ptr.is_null() {
            (*rt_ptr).handle_rumble(strength);
            return true;
        }
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

const PC88_AUDIO_RING_CAPACITY: usize = 32_768;
const PC88_AUDIO_BATCH_SAMPLES: usize = 512;

struct Pc88AudioSource {
    consumer: Consumer<f32>,
    read_buf: [f32; PC88_AUDIO_BATCH_SAMPLES],
    read_pos: usize,
    read_len: usize,
    volume_bits: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    flush: Arc<AtomicBool>,
}

impl Pc88AudioSource {
    fn new(
        consumer: Consumer<f32>,
        volume_bits: Arc<AtomicU32>,
        stop: Arc<AtomicBool>,
        flush: Arc<AtomicBool>,
    ) -> Self {
        Self {
            consumer,
            read_buf: [0.0; PC88_AUDIO_BATCH_SAMPLES],
            read_pos: 0,
            read_len: 0,
            volume_bits,
            stop,
            flush,
        }
    }
}

impl Iterator for Pc88AudioSource {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.read_pos < self.read_len {
            let sample = self.read_buf[self.read_pos];
            self.read_pos += 1;
            let vol = f32::from_bits(self.volume_bits.load(Ordering::Relaxed));
            return Some(sample * vol);
        }
        self.read_pos = 0;
        self.read_len = 0;

        if self.stop.load(Ordering::Relaxed) {
            return None;
        }

        if self.flush.swap(false, Ordering::Acquire) {
            let drop_even = self.consumer.slots() & !1;
            if drop_even > 0 {
                if let Ok(chunk) = self.consumer.read_chunk(drop_even) {
                    chunk.commit_all();
                }
            }
        }

        // Cap queued latency to ~70ms at 44.1kHz stereo so audio never drifts behind video.
        const MAX_QUEUED_SAMPLES: usize = (44100 * 2 / 14) & !1;
        let total_slots = self.consumer.slots();
        if total_slots > MAX_QUEUED_SAMPLES {
            let drop_even = (total_slots - MAX_QUEUED_SAMPLES) & !1;
            if drop_even > 0 {
                if let Ok(chunk) = self.consumer.read_chunk(drop_even) {
                    chunk.commit_all();
                }
            }
        }

        let available_even = (self.consumer.slots() & !1).min(PC88_AUDIO_BATCH_SAMPLES);
        if available_even >= 2 {
            if let Ok(chunk) = self.consumer.read_chunk(available_even) {
                let (first, second) = chunk.as_slices();
                let first_len = first.len();
                let second_len = second.len();
                self.read_buf[..first_len].copy_from_slice(first);
                if second_len > 0 {
                    self.read_buf[first_len..first_len + second_len].copy_from_slice(second);
                }
                chunk.commit_all();
                self.read_len = first_len + second_len;
                self.read_pos = 1;
                let vol = f32::from_bits(self.volume_bits.load(Ordering::Relaxed));
                return Some(self.read_buf[0] * vol);
            }
        }

        if self.consumer.is_abandoned() {
            return None;
        }

        self.read_buf[0] = 0.0;
        self.read_buf[1] = 0.0;
        self.read_len = 2;
        self.read_pos = 1;
        Some(0.0)
    }
}

impl Source for Pc88AudioSource {
    #[inline]
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    #[inline]
    fn channels(&self) -> u16 {
        PC88_AUDIO_CHANNELS
    }

    #[inline]
    fn sample_rate(&self) -> u32 {
        PC88_AUDIO_SAMPLE_RATE
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

struct Pc88AudioPlayback {
    _guard: crate::devices::audio::SharedPlaybackGuard,
    _handle: rodio::OutputStreamHandle,
    volume_bits: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    flush: Arc<AtomicBool>,
}

impl Drop for Pc88AudioPlayback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone)]
struct Pc88LatchedInputState {
    joypad_buttons: u16,
    analog_axes: [[i16; 2]; 2],
    keys: [bool; 512],
    mouse_dx: i16,
    mouse_dy: i16,
    mouse_left: bool,
    mouse_right: bool,
    mouse_middle: bool,
}

impl Default for Pc88LatchedInputState {
    fn default() -> Self {
        Self {
            joypad_buttons: 0,
            analog_axes: [[0; 2]; 2],
            keys: [false; 512],
            mouse_dx: 0,
            mouse_dy: 0,
            mouse_left: false,
            mouse_right: false,
            mouse_middle: false,
        }
    }
}

impl Pc88LatchedInputState {
    #[inline]
    fn latch_from(&mut self, input: &mut Pc98InputState) {
        self.joypad_buttons = input.joypad_buttons;
        self.analog_axes = input.analog_axes;
        self.keys = input.keys;
        for (key_code, ref mut rem) in input.pulse_keys.iter_mut() {
            if *rem > 0 && (*key_code as usize) < self.keys.len() {
                self.keys[*key_code as usize] = true;
                *rem -= 1;
            }
        }
        input.pulse_keys.retain(|(_, rem)| *rem > 0);

        self.mouse_dx = input.mouse_dx.clamp(-127, 127) as i16;
        self.mouse_dy = input.mouse_dy.clamp(-127, 127) as i16;
        input.mouse_dx -= self.mouse_dx as i32;
        input.mouse_dy -= self.mouse_dy as i32;

        self.mouse_left = input.mouse_left;
        self.mouse_right = input.mouse_right;
        self.mouse_middle = input.mouse_middle;
    }

    #[inline]
    fn query(&self, device: c_uint, index: c_uint, id: c_uint) -> i16 {
        match device & 0xFF {
            RETRO_DEVICE_JOYPAD => {
                let f12_select =
                    (RETROK_F12 as usize) < self.keys.len() && self.keys[RETROK_F12 as usize];
                let buttons = if f12_select {
                    self.joypad_buttons | (1 << RETRO_DEVICE_ID_JOYPAD_SELECT)
                } else {
                    self.joypad_buttons
                };
                if id == 256 {
                    buttons as i16
                } else if id < 16 {
                    i16::from((buttons & (1 << id)) != 0)
                } else {
                    0
                }
            }
            crate::pc98::input::RETRO_DEVICE_ANALOG => self
                .analog_axes
                .get(index as usize)
                .and_then(|axis| axis.get(id as usize))
                .copied()
                .unwrap_or(0),
            RETRO_DEVICE_KEYBOARD => {
                if (id as usize) < self.keys.len() && self.keys[id as usize] {
                    1
                } else {
                    0
                }
            }
            RETRO_DEVICE_MOUSE => match id {
                RETRO_DEVICE_ID_MOUSE_X => self.mouse_dx,
                RETRO_DEVICE_ID_MOUSE_Y => self.mouse_dy,
                RETRO_DEVICE_ID_MOUSE_LEFT => i16::from(self.mouse_left),
                RETRO_DEVICE_ID_MOUSE_RIGHT => i16::from(self.mouse_right),
                RETRO_DEVICE_ID_MOUSE_MIDDLE => i16::from(self.mouse_middle),
                _ => 0,
            },
            _ => 0,
        }
    }
}

struct Pc88RealtimeState {
    input_state: Arc<Mutex<Pc98InputState>>,
    latched_input: Pc88LatchedInputState,
    audio_producer: Option<Producer<f32>>,
    peak_amplitude: Arc<AtomicU64>,
    audio_latency_ms: Arc<AtomicU64>,
    replay_audio: Arc<crate::replay::AudioTap>,
    seek_mixer: FloppySeekMixer,
    rumble_seek_frames: u32,
    resampler: ContinuousStereoResampler,
}

impl Pc88RealtimeState {
    #[inline]
    fn handle_rumble(&mut self, strength: u16) {
        if strength > 0 {
            if self.rumble_seek_frames == 0 {
                self.seek_mixer.stop(1);
                self.seek_mixer.play(1, false);
                self.rumble_seek_frames = 1;
            } else {
                self.rumble_seek_frames = self.rumble_seek_frames.saturating_add(1);
                if self.rumble_seek_frames >= 2 {
                    self.seek_mixer.play(0, true);
                }
            }
        } else if self.rumble_seek_frames > 0 {
            self.seek_mixer.stop(0);
            self.rumble_seek_frames = 0;
        }
    }

    #[inline]
    fn process_audio_batch(&mut self, raw_samples: &[i16]) {
        let mut chunk_f32 = [0.0f32; PC88_AUDIO_BATCH_SAMPLES];
        let mut max_abs = 0.0f32;
        let replay_enabled = self.replay_audio.is_enabled();
        if !replay_enabled {
            self.resampler.reset();
        }
        let now_at = if replay_enabled {
            crate::replay::now_us()
        } else {
            0
        };
        let mut submitted_frames = 0i64;

        for raw_chunk in raw_samples.chunks(PC88_AUDIO_BATCH_SAMPLES) {
            let count = raw_chunk.len() & !1;
            if count == 0 {
                continue;
            }
            let slice_f32 = &mut chunk_f32[..count];
            for (dst, &src) in slice_f32.iter_mut().zip(&raw_chunk[..count]) {
                *dst = (src as f32) / 32768.0;
            }

            self.seek_mixer.mix_into_stereo(slice_f32);

            for &val in slice_f32.iter() {
                let abs = val.abs();
                if abs > max_abs {
                    max_abs = abs;
                }
            }

            if let Some(ref mut prod) = self.audio_producer {
                let writable = (prod.slots() & !1).min(count);
                if writable >= 2 {
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

            self.replay_audio.observe_reactive(
                slice_f32,
                PC88_AUDIO_SAMPLE_RATE,
                PC88_AUDIO_CHANNELS,
            );

            if replay_enabled {
                let replay_tap = &self.replay_audio;
                self.resampler
                    .process_stereo_chunks(slice_f32, |resampled| {
                        let chunk_at = now_at
                            + submitted_frames * 1_000_000 / (PC88_REPLAY_SAMPLE_RATE as i64);
                        replay_tap.submit(
                            resampled,
                            PC88_REPLAY_SAMPLE_RATE,
                            PC88_AUDIO_CHANNELS,
                            chunk_at,
                        );
                        submitted_frames += (resampled.len() / 2) as i64;
                    });
            }
        }

        let peak_bits = (max_abs * 1000.0) as u64;
        self.peak_amplitude.fetch_max(peak_bits, Ordering::Relaxed);
        self.audio_latency_ms.store(18, Ordering::Relaxed);
    }
}

thread_local! {
    static CURRENT_INPUT_LATCH: Cell<*const Pc88LatchedInputState> =
        Cell::new(ptr::null());
    static CURRENT_PC88_REALTIME_STATE: Cell<*mut Pc88RealtimeState> =
        Cell::new(ptr::null_mut());
}

struct Pc88RetroRunScopeGuard;

impl Pc88RetroRunScopeGuard {
    #[inline]
    fn enter(rt_cell: &UnsafeCell<Pc88RealtimeState>) -> Self {
        let rt_ptr = rt_cell.get();
        let latch_ptr = unsafe { ptr::addr_of!((*rt_ptr).latched_input) };
        CURRENT_PC88_REALTIME_STATE.with(|c| c.set(rt_ptr));
        CURRENT_INPUT_LATCH.with(|c| c.set(latch_ptr));
        Self
    }
}

impl Drop for Pc88RetroRunScopeGuard {
    #[inline]
    fn drop(&mut self) {
        CURRENT_INPUT_LATCH.with(|c| c.set(ptr::null()));
        CURRENT_PC88_REALTIME_STATE.with(|c| c.set(ptr::null_mut()));
    }
}

struct Pc88SessionCoordinatorState {
    active_session_id: Option<u64>,
    lifecycle: EmulatorLifecycleState,
    next_session_id: u64,
}

struct Pc88SessionCoordinator {
    state: Mutex<Pc88SessionCoordinatorState>,
    cv: Condvar,
}

impl Pc88SessionCoordinator {
    fn new() -> Self {
        Self {
            state: Mutex::new(Pc88SessionCoordinatorState {
                active_session_id: None,
                lifecycle: EmulatorLifecycleState::Idle,
                next_session_id: 1,
            }),
            cv: Condvar::new(),
        }
    }

    fn current_lifecycle(&self) -> EmulatorLifecycleState {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .lifecycle
    }
}

fn pc88_session_coordinator() -> &'static Pc88SessionCoordinator {
    static COORD: OnceLock<Pc88SessionCoordinator> = OnceLock::new();
    COORD.get_or_init(Pc88SessionCoordinator::new)
}

pub fn pc88_lifecycle_state() -> EmulatorLifecycleState {
    pc88_session_coordinator().current_lifecycle()
}

struct Pc88SessionGuard {
    session_id: u64,
}

impl Pc88SessionGuard {
    fn acquire(stop_requested: &AtomicBool) -> Option<Self> {
        let coord = pc88_session_coordinator();
        let mut guard = coord.state.lock().unwrap_or_else(|p| p.into_inner());
        while guard.active_session_id.is_some() {
            if stop_requested.load(Ordering::Acquire) {
                return None;
            }
            let (next_guard, _) = coord
                .cv
                .wait_timeout(guard, Duration::from_millis(10))
                .unwrap_or_else(|p| p.into_inner());
            guard = next_guard;
        }
        if stop_requested.load(Ordering::Acquire) {
            return None;
        }
        let session_id = guard.next_session_id;
        guard.next_session_id = guard.next_session_id.saturating_add(1);
        guard.active_session_id = Some(session_id);
        guard.lifecycle = EmulatorLifecycleState::Starting(session_id);
        Some(Self { session_id })
    }

    fn install_bridge(&self, mut ctx: LibretroBridgeContext, stop_requested: &AtomicBool) -> bool {
        if stop_requested.load(Ordering::Acquire) {
            return false;
        }
        ctx.session_id = self.session_id;
        {
            let mut bridge_guard = ACTIVE_PC88_BRIDGE.lock().unwrap_or_else(|p| p.into_inner());
            if stop_requested.load(Ordering::Acquire) {
                return false;
            }
            *bridge_guard = Some(Box::new(ctx));
        }
        let coord = pc88_session_coordinator();
        let mut guard = coord.state.lock().unwrap_or_else(|p| p.into_inner());
        if guard.active_session_id == Some(self.session_id) {
            guard.lifecycle = EmulatorLifecycleState::Running(self.session_id);
        }
        true
    }

    fn mark_stopping(&self) {
        let coord = pc88_session_coordinator();
        let mut guard = coord.state.lock().unwrap_or_else(|p| p.into_inner());
        if guard.active_session_id == Some(self.session_id) {
            guard.lifecycle = EmulatorLifecycleState::Stopping(self.session_id);
        }
    }
}

impl Drop for Pc88SessionGuard {
    fn drop(&mut self) {
        if let Ok(mut bridge_guard) = ACTIVE_PC88_BRIDGE.lock() {
            if bridge_guard.as_ref().map(|c| c.session_id) == Some(self.session_id) {
                *bridge_guard = None;
            }
        }
        let coord = pc88_session_coordinator();
        let mut guard = coord.state.lock().unwrap_or_else(|p| p.into_inner());
        if guard.active_session_id == Some(self.session_id) {
            guard.active_session_id = None;
            guard.lifecycle = EmulatorLifecycleState::Idle;
            coord.cv.notify_all();
        }
    }
}

struct LibretroBridgeContext {
    session_id: u64,
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
    peak_amplitude: Arc<AtomicU64>,
    audio_latency_ms: Arc<AtomicU64>,
    replay_audio: Arc<crate::replay::AudioTap>,
    input_state: Arc<Mutex<Pc98InputState>>,
    latched_input: Pc88LatchedInputState,
    seek_mixer: FloppySeekMixer,
    rumble_seek_frames: u32,
    resampler: ContinuousStereoResampler,
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

pub fn populate_quasi88_variables(vars: &mut HashMap<String, CString>, config: &Pc88Config) {
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
        if config.fdc_wait {
            "enabled"
        } else {
            "disabled"
        },
    );
    set_var(
        "q88_pcg-8100",
        if config.pcg_8100 {
            "enabled"
        } else {
            "disabled"
        },
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
        RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY => with_bridge_ctx(|ctx| {
            if !data.is_null() {
                *(data as *mut *const c_char) = ctx.system_dir_c.as_ptr();
            }
            true
        })
        .unwrap_or(false),
        RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY => with_bridge_ctx(|ctx| {
            if !data.is_null() {
                *(data as *mut *const c_char) = ctx.save_dir_c.as_ptr();
            }
            true
        })
        .unwrap_or(false),
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
        RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE => with_bridge_ctx(|ctx| {
            let dirty = ctx.variables_dirty;
            ctx.variables_dirty = false;
            if !data.is_null() {
                *(data as *mut bool) = dirty;
            }
            true
        })
        .unwrap_or(false),
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

    let (pixel_format, frame_tx, event_tx, repaint, res_changed) = {
        let Ok(mut guard) = ACTIVE_PC88_BRIDGE.lock() else {
            return;
        };
        let Some(ctx) = guard.as_mut() else {
            return;
        };
        let changed = ctx.last_resolution != (width, height);
        if changed {
            ctx.last_resolution = (width, height);
        }
        (
            ctx.pixel_format,
            ctx.frame_tx.clone(),
            ctx.event_tx.clone(),
            ctx.request_repaint.clone(),
            changed,
        )
    };

    if res_changed {
        let _ = event_tx.send(Pc88Event::ResolutionChanged { width, height });
    }

    let total_src_bytes = pitch.saturating_mul(height as usize);
    let src_bytes = std::slice::from_raw_parts(data as *const u8, total_src_bytes);

    let rgba = match pixel_format {
        RETRO_PIXEL_FORMAT_XRGB8888 => convert_xrgb8888_to_rgba(src_bytes, width, height, pitch),
        RETRO_PIXEL_FORMAT_RGB565 => convert_rgb565_to_rgba(src_bytes, width, height, pitch),
        RETRO_PIXEL_FORMAT_0RGB1555 => convert_rgb1555_to_rgba(src_bytes, width, height, pitch),
        _ => convert_rgb1555_to_rgba(src_bytes, width, height, pitch),
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

    let rt_ptr = CURRENT_PC88_REALTIME_STATE.with(|c| c.get());
    if !rt_ptr.is_null() {
        (*rt_ptr).process_audio_batch(raw_samples);
        return frames;
    }

    with_bridge_ctx(|ctx| {
        let mut chunk_f32 = [0.0f32; PC88_AUDIO_BATCH_SAMPLES];
        let mut max_abs = 0.0f32;
        let replay_enabled = ctx.replay_audio.is_enabled();
        if !replay_enabled {
            ctx.resampler.reset();
        }
        let now_at = if replay_enabled {
            crate::replay::now_us()
        } else {
            0
        };
        let mut submitted_frames = 0i64;

        for raw_chunk in raw_samples.chunks(PC88_AUDIO_BATCH_SAMPLES) {
            let count = raw_chunk.len() & !1;
            if count == 0 {
                continue;
            }
            let slice_f32 = &mut chunk_f32[..count];
            for (dst, &src) in slice_f32.iter_mut().zip(&raw_chunk[..count]) {
                *dst = (src as f32) / 32768.0;
            }
            ctx.seek_mixer.mix_into_stereo(slice_f32);
            for &val in slice_f32.iter() {
                let abs = val.abs();
                if abs > max_abs {
                    max_abs = abs;
                }
            }
            ctx.replay_audio.observe_reactive(
                slice_f32,
                PC88_AUDIO_SAMPLE_RATE,
                PC88_AUDIO_CHANNELS,
            );
            if replay_enabled {
                let replay_tap = &ctx.replay_audio;
                ctx.resampler.process_stereo_chunks(slice_f32, |resampled| {
                    let chunk_at =
                        now_at + submitted_frames * 1_000_000 / (PC88_REPLAY_SAMPLE_RATE as i64);
                    replay_tap.submit(
                        resampled,
                        PC88_REPLAY_SAMPLE_RATE,
                        PC88_AUDIO_CHANNELS,
                        chunk_at,
                    );
                    submitted_frames += (resampled.len() / 2) as i64;
                });
            }
        }

        let peak_bits = (max_abs * 1000.0) as u64;
        ctx.peak_amplitude.fetch_max(peak_bits, Ordering::Relaxed);
        ctx.audio_latency_ms.store(18, Ordering::Relaxed);
    });

    frames
}

unsafe extern "C" fn bridge_input_poll_cb() {
    let rt_ptr = CURRENT_PC88_REALTIME_STATE.with(|c| c.get());
    if !rt_ptr.is_null() {
        let rt = &mut *rt_ptr;
        if let Ok(mut input) = rt.input_state.lock() {
            rt.latched_input.latch_from(&mut input);
        }
        return;
    }

    let Ok(mut guard) = ACTIVE_PC88_BRIDGE.lock() else {
        return;
    };
    let Some(ctx) = guard.as_mut() else {
        return;
    };

    if let Ok(mut input) = ctx.input_state.lock() {
        ctx.latched_input.latch_from(&mut input);
    };
}

unsafe extern "C" fn bridge_input_state_cb(
    _port: c_uint,
    device: c_uint,
    index: c_uint,
    id: c_uint,
) -> i16 {
    let latch_ptr = CURRENT_INPUT_LATCH.with(|c| c.get());
    if !latch_ptr.is_null() {
        return (*latch_ptr).query(device, index, id);
    }

    let Ok(guard) = ACTIVE_PC88_BRIDGE.lock() else {
        return 0;
    };
    let Some(ctx) = guard.as_ref() else {
        return 0;
    };

    ctx.latched_input.query(device, index, id)
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
    let Some(session_guard) = Pc88SessionGuard::acquire(&params.stop_requested) else {
        return Ok(());
    };

    let sys_dir = ensure_pc88_system_dir(Some(&params.config.games_dir))?;
    if params.stop_requested.load(Ordering::Acquire) {
        return Ok(());
    }

    let save_dir = pc88_saves_dir();
    let core_so_path = ensure_quasi88_core(Some(&params.event_tx))?;
    if params.stop_requested.load(Ordering::Acquire) {
        return Ok(());
    }

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

    if params.stop_requested.load(Ordering::Acquire) {
        return Ok(());
    }

    let boot_plan = prepare_pc88_boot_media(&params.config, &params.active_game_floppies)?;

    let volume_bits = Arc::new(AtomicU32::new(
        params.master_volume.clamp(0.0, 1.0).to_bits(),
    ));
    let audio_stop = Arc::new(AtomicBool::new(false));
    let audio_flush = Arc::new(AtomicBool::new(false));
    let (audio_producer, audio_consumer) = RingBuffer::<f32>::new(PC88_AUDIO_RING_CAPACITY);

    let (audio_playback_opt, audio_producer_opt) =
        match crate::devices::audio::initialize_rodio_playback() {
            Ok((stream_guard, stream_handle)) => {
                let source = Pc88AudioSource::new(
                    audio_consumer,
                    Arc::clone(&volume_bits),
                    Arc::clone(&audio_stop),
                    Arc::clone(&audio_flush),
                );
                match stream_handle.play_raw(source) {
                    Ok(()) => (
                        Some(Pc88AudioPlayback {
                            _guard: stream_guard,
                            _handle: stream_handle,
                            volume_bits: Arc::clone(&volume_bits),
                            stop: Arc::clone(&audio_stop),
                            flush: Arc::clone(&audio_flush),
                        }),
                        Some(audio_producer),
                    ),
                    Err(e) => {
                        tracing::warn!("Failed to start PC-88 raw audio stream: {e}");
                        (None, None)
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to open audio output for PC-88: {e}");
                (None, None)
            }
        };

    let mut variables = HashMap::new();
    populate_quasi88_variables(&mut variables, &params.config);

    let rt_state = UnsafeCell::new(Pc88RealtimeState {
        input_state: params.input_state.clone(),
        latched_input: Pc88LatchedInputState::default(),
        audio_producer: audio_producer_opt,
        peak_amplitude: params.peak_amplitude.clone(),
        audio_latency_ms: params.audio_latency_ms.clone(),
        replay_audio: params.replay_audio.clone(),
        seek_mixer: FloppySeekMixer::new(
            params.config.floppy_seek_sound,
            params.config.floppy_seek_volume,
        ),
        rumble_seek_frames: 0,
        resampler: ContinuousStereoResampler::new(PC88_AUDIO_SAMPLE_RATE, PC88_REPLAY_SAMPLE_RATE),
    });

    let bridge_ctx = LibretroBridgeContext {
        session_id: session_guard.session_id,
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
        peak_amplitude: params.peak_amplitude,
        audio_latency_ms: params.audio_latency_ms,
        replay_audio: params.replay_audio,
        input_state: params.input_state.clone(),
        latched_input: Pc88LatchedInputState::default(),
        seek_mixer: FloppySeekMixer::new(
            params.config.floppy_seek_sound,
            params.config.floppy_seek_volume,
        ),
        rumble_seek_frames: 0,
        resampler: ContinuousStereoResampler::new(PC88_AUDIO_SAMPLE_RATE, PC88_REPLAY_SAMPLE_RATE),
    };

    if !session_guard.install_bridge(bridge_ctx, &params.stop_requested) {
        return Ok(());
    }

    let result = unsafe {
        run_loaded_core(
            temp_so.path(),
            boot_plan,
            &params.config,
            &params.command_rx,
            &params.event_tx,
            &params.stop_requested,
            &rt_state,
            audio_playback_opt.as_ref(),
        )
    };

    session_guard.mark_stopping();
    // `session_guard` drops here AFTER `run_loaded_core` has called `retro_unload_game()`,
    // `retro_deinit()`, and dropped `Library` (`dlclose`).
    drop(session_guard);

    result
}

#[inline]
fn advance_frame_deadline(
    next_frame_deadline: Instant,
    now: Instant,
    frame_interval: Duration,
) -> (Option<Duration>, Instant) {
    if next_frame_deadline > now {
        (
            Some(next_frame_deadline - now),
            next_frame_deadline + frame_interval,
        )
    } else if now.saturating_duration_since(next_frame_deadline) > frame_interval * 4 {
        (None, now + frame_interval)
    } else {
        (None, next_frame_deadline + frame_interval)
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn run_loaded_core(
    core_so_path: &Path,
    mut boot_plan: BootMediaPlan,
    initial_config: &Pc88Config,
    command_rx: &Receiver<Pc88Command>,
    event_tx: &Sender<Pc88Event>,
    stop_requested: &Arc<AtomicBool>,
    rt_cell: &UnsafeCell<Pc88RealtimeState>,
    audio_playback: Option<&Pc88AudioPlayback>,
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
        path: content_path_c.as_ref().map_or(ptr::null(), |c| c.as_ptr()),
        data: ptr::null(),
        size: 0,
        meta: ptr::null(),
    };

    if !retro_load_game(&game_info) {
        retro_deinit();
        anyhow::bail!("Quasi88 retro_load_game returned false");
    }

    let mount_fdd_live =
        |drive: usize, target_path: Option<&Path>, disk_list: &mut Vec<PathBuf>| -> bool {
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
    {
        let _run_guard = Pc88RetroRunScopeGuard::enter(rt_cell);
        retro_run();
    }

    let fps = if av_info.timing.fps > 10.0 && av_info.timing.fps < 240.0 {
        av_info.timing.fps
    } else {
        59.94
    };
    let width = av_info.geometry.base_width.max(320);
    let height = av_info.geometry.base_height.max(200);

    let _ = event_tx.send(Pc88Event::Started { width, height, fps });

    let save_commit_worker =
        crate::pc98::savestate::SaveStateCommitWorker::new("pc88-savestate-commit");

    let frame_interval = Duration::from_secs_f64(1.0 / fps);
    let mut next_frame_deadline = Instant::now() + frame_interval;

    while !stop_requested.load(Ordering::Relaxed) {
        while let Ok(cmd) = command_rx.try_recv() {
            match cmd {
                Pc88Command::Stop => {
                    stop_requested.store(true, Ordering::SeqCst);
                }
                Pc88Command::Reset => {
                    if let Some(playback) = audio_playback {
                        playback.flush.store(true, Ordering::Release);
                    }
                    {
                        let rt = &mut *rt_cell.get();
                        rt.rumble_seek_frames = 0;
                        rt.seek_mixer.stop(0);
                        rt.seek_mixer.stop(1);
                        rt.resampler.reset();
                    }
                    retro_reset();
                    let _ = event_tx.send(Pc88Event::Info("PC-88 system reset.".to_string()));
                }
                Pc88Command::TriggerMenu => {
                    if let Ok(mut guard) = (*rt_cell.get()).input_state.lock() {
                        guard.pulse_keys.push((RETROK_F12, 4));
                    }
                }
                Pc88Command::SetVolume(vol) => {
                    if let Some(playback) = audio_playback {
                        playback
                            .volume_bits
                            .store(vol.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
                    }
                }
                Pc88Command::MountFdd { drive, path } => {
                    if let Err(e) =
                        super::library::try_sync_back_multi_d88_files(&boot_plan.disk_list)
                    {
                        let _ = event_tx.send(Pc88Event::Info(format!(
                            "Failed to sync multi-disk D88 changes: {e}"
                        )));
                    }
                    if mount_fdd_live(drive as usize, path.as_deref(), &mut boot_plan.disk_list) {
                        if drive == 0 {
                            current_fdd1 = path.clone();
                        } else if drive == 1 {
                            current_fdd2 = path.clone();
                        }
                        if let Some(p) = path {
                            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("disk");
                            let _ = event_tx.send(Pc88Event::Info(format!(
                                "Mounted '{}' in FDD{}",
                                name,
                                drive + 1
                            )));
                        } else {
                            let _ =
                                event_tx.send(Pc88Event::Info(format!("Ejected FDD{}", drive + 1)));
                        }
                    } else {
                        let _ = event_tx.send(Pc88Event::Info(format!(
                            "Could not swap FDD{} disk",
                            drive + 1
                        )));
                    }
                }
                Pc88Command::UpdateOptions(new_cfg) => {
                    (*rt_cell.get())
                        .seek_mixer
                        .set_config(new_cfg.floppy_seek_sound, new_cfg.floppy_seek_volume);
                    with_bridge_ctx(|ctx| {
                        populate_quasi88_variables(&mut ctx.variables, &new_cfg);
                        ctx.variables_dirty = true;
                        ctx.seek_mixer
                            .set_config(new_cfg.floppy_seek_sound, new_cfg.floppy_seek_volume);
                    });
                    active_config = new_cfg;
                }
                Pc88Command::SaveState { slot } => {
                    let slot = slot.clamp(1, crate::pc98::savestate::SAVE_SLOT_COUNT as u8);
                    if let Err(e) =
                        super::library::try_sync_back_multi_d88_files(&boot_plan.disk_list)
                    {
                        let _ = event_tx.send(Pc88Event::Info(format!(
                            "Failed to sync multi-disk D88 changes: {e}"
                        )));
                    }
                    let state_size = retro_serialize_size();
                    if state_size == 0 {
                        let _ = event_tx.send(Pc88Event::Info(
                            "PC-88 save state failed (serialize size is 0).".to_string(),
                        ));
                    } else {
                        let mut state_buf = vec![0u8; state_size];
                        if retro_serialize(state_buf.as_mut_ptr() as *mut c_void, state_size) {
                            let (last_frame, last_res, repaint) = with_bridge_ctx(|ctx| {
                                (
                                    ctx.last_frame.clone(),
                                    ctx.last_resolution,
                                    Some(ctx.request_repaint.clone()),
                                )
                            })
                            .unwrap_or((None, (width, height), None));
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
                                state_len: Some(state_buf.len() as u64),
                            };
                            let states_root = crate::pc98::savestate::pc88_states_root();
                            let tx = event_tx.clone();
                            save_commit_worker.submit(
                                crate::pc98::savestate::SaveStateCommitRequest {
                                    states_root,
                                    game_key,
                                    meta,
                                    state_bytes: state_buf,
                                    frame: last_frame,
                                    core_id: Some("quasi88_libretro".to_string()),
                                    on_complete: Box::new(move |res| {
                                        match res {
                                            Ok(committed_meta) => {
                                                let _ = tx.send(Pc88Event::StateSaved {
                                                    slot,
                                                    meta: committed_meta,
                                                });
                                            }
                                            Err(e) => {
                                                let _ = tx.send(Pc88Event::Info(format!(
                                                    "Failed to write PC-88 slot {}: {e}",
                                                    slot
                                                )));
                                            }
                                        }
                                        if let Some(cb) = repaint {
                                            cb();
                                        }
                                    }),
                                },
                            );
                        } else {
                            let _ = event_tx.send(Pc88Event::Info(format!(
                                "PC-88 core failed to serialize state into slot {}.",
                                slot
                            )));
                        }
                    }
                }
                Pc88Command::LoadState { slot } => {
                    save_commit_worker.flush();
                    let slot = slot.clamp(1, crate::pc98::savestate::SAVE_SLOT_COUNT as u8);
                    let (game_key, _) =
                        crate::pc98::savestate::derive_pc88_game_key(&active_config);
                    let max_state_size = retro_serialize_size();
                    match crate::pc98::savestate::validate_slot_for_session_load(
                        &crate::pc98::savestate::pc88_states_root(),
                        &game_key,
                        slot,
                        "PC-88",
                        Some("quasi88_libretro"),
                        if max_state_size > 0 {
                            Some(max_state_size)
                        } else {
                            None
                        },
                    ) {
                        Ok((meta, state_bytes)) => {
                            let pre_load_snapshot = if max_state_size > 0 {
                                let mut snap = vec![0u8; max_state_size];
                                if retro_serialize(snap.as_mut_ptr() as *mut c_void, max_state_size)
                                {
                                    Some(snap)
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            let prev_disk_list = boot_plan.disk_list.clone();

                            let load_res = crate::pc98::savestate::execute_load_state_with_rollback(
                                &meta,
                                &state_bytes,
                                &mut current_fdd1,
                                &mut current_fdd2,
                                |drive, path| {
                                    mount_fdd_live(drive as usize, path, &mut boot_plan.disk_list)
                                },
                                |bytes| {
                                    if retro_unserialize(
                                        bytes.as_ptr() as *const c_void,
                                        bytes.len(),
                                    ) {
                                        true
                                    } else {
                                        if let Some(ref snap) = pre_load_snapshot {
                                            let _ = retro_unserialize(
                                                snap.as_ptr() as *const c_void,
                                                snap.len(),
                                            );
                                        }
                                        false
                                    }
                                },
                            );

                            match load_res {
                                Ok(()) => {
                                    if let Some(playback) = audio_playback {
                                        playback.flush.store(true, Ordering::Release);
                                    }
                                    {
                                        let rt = &mut *rt_cell.get();
                                        rt.latched_input = Pc88LatchedInputState::default();
                                        rt.rumble_seek_frames = 0;
                                        rt.seek_mixer.stop(0);
                                        rt.seek_mixer.stop(1);
                                        rt.resampler.reset();
                                        if let Ok(mut inp) = rt.input_state.lock() {
                                            inp.release_all();
                                        }
                                    }
                                    with_bridge_ctx(|ctx| {
                                        ctx.latched_input = Pc88LatchedInputState::default();
                                        ctx.rumble_seek_frames = 0;
                                        ctx.seek_mixer.stop(0);
                                        ctx.seek_mixer.stop(1);
                                        ctx.resampler.reset();
                                    });
                                    let _ = event_tx.send(Pc88Event::StateLoaded { slot, meta });
                                }
                                Err(e) => {
                                    boot_plan.disk_list = prev_disk_list;
                                    let _ = event_tx.send(Pc88Event::Info(format!("{e}")));
                                }
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

        {
            let _run_guard = Pc88RetroRunScopeGuard::enter(rt_cell);
            retro_run();
        }

        let now = Instant::now();
        let (sleep_dur, updated_deadline) =
            advance_frame_deadline(next_frame_deadline, now, frame_interval);
        next_frame_deadline = updated_deadline;
        if let Some(dur) = sleep_dur {
            std::thread::sleep(dur);
        }
    }

    save_commit_worker.flush();
    drop(save_commit_worker);

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

    if let Err(e) = super::library::try_sync_back_multi_d88_files(&boot_plan.disk_list) {
        let _ = event_tx.send(Pc88Event::Info(format!(
            "Failed to sync multi-disk D88 changes on exit: {e}"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_f18_pc88_single_sample_and_batch_resampling_equivalence() {
        let mut resampler_single =
            ContinuousStereoResampler::new(PC88_AUDIO_SAMPLE_RATE, PC88_REPLAY_SAMPLE_RATE);
        let mut resampler_batch =
            ContinuousStereoResampler::new(PC88_AUDIO_SAMPLE_RATE, PC88_REPLAY_SAMPLE_RATE);

        let in_frames = 44_100usize;
        let mut input = Vec::with_capacity(in_frames * 2);
        for i in 0..in_frames {
            let phase = (i as f32) / 44_100.0;
            input.push((phase * 200.0).sin() * 0.5);
            input.push((phase * 300.0).cos() * 0.5);
        }

        let mut single_out = Vec::with_capacity(48_000 * 2);
        for frame in input.chunks_exact(2) {
            single_out.extend(resampler_single.process_stereo(frame));
        }

        let batch_out = resampler_batch.process_stereo(&input);
        assert_eq!(single_out.len(), 48_000 * 2);
        assert_eq!(batch_out.len(), 48_000 * 2);
        for (i, (&s, &b)) in single_out.iter().zip(batch_out.iter()).enumerate() {
            assert!((s - b).abs() <= 1e-6, "Mismatch at sample {i}: {s} vs {b}");
        }
    }

    #[test]
    fn test_f01_pc88_session_lifecycle_isolation_and_pointer_validity() {
        use std::sync::Barrier;

        let make_ctx = |sys_dir: &str, save_dir: &str| -> LibretroBridgeContext {
            let (frame_tx, _frame_rx) = crossbeam_channel::bounded(2);
            let (event_tx, _event_rx) = crossbeam_channel::unbounded();
            let mut variables = HashMap::new();
            populate_quasi88_variables(&mut variables, &Pc88Config::default());
            LibretroBridgeContext {
                session_id: 0,
                system_dir_c: CString::new(sys_dir).unwrap(),
                save_dir_c: CString::new(save_dir).unwrap(),
                variables,
                variables_dirty: false,
                pixel_format: RETRO_PIXEL_FORMAT_RGB565,
                av_info: RetroSystemAvInfo::default(),
                last_resolution: (640, 400),
                last_frame: None,
                frame_tx,
                event_tx,
                request_repaint: Arc::new(|| {}),
                peak_amplitude: Arc::new(AtomicU64::new(0)),
                audio_latency_ms: Arc::new(AtomicU64::new(0)),
                replay_audio: Arc::new(crate::replay::AudioTap::default()),
                input_state: Arc::new(Mutex::new(Pc98InputState::default())),
                latched_input: Pc88LatchedInputState::default(),
                seek_mixer: FloppySeekMixer::new(false, 0),
                rumble_seek_frames: 0,
                resampler: ContinuousStereoResampler::new(44100, 48000),
            }
        };

        // 1. Start A -> Stop A during download/init -> Start B
        let stop_a1 = Arc::new(AtomicBool::new(false));
        let guard_a1 = Pc88SessionGuard::acquire(&stop_a1).unwrap();
        stop_a1.store(true, Ordering::SeqCst);
        assert!(!guard_a1.install_bridge(make_ctx("/sys/88_a1", "/save/88_a1"), &stop_a1));
        drop(guard_a1);
        assert_eq!(pc88_lifecycle_state(), EmulatorLifecycleState::Idle);

        // 2. Start A -> Stop A while A is shutting down, while Start B is waiting on the session gate
        let stop_a2 = Arc::new(AtomicBool::new(false));
        let stop_b2 = Arc::new(AtomicBool::new(false));
        let a_running_barrier = Arc::new(Barrier::new(2));
        let a_allow_finish_barrier = Arc::new(Barrier::new(2));

        let stop_a2_thread = stop_a2.clone();
        let a_run_b = a_running_barrier.clone();
        let a_fin_b = a_allow_finish_barrier.clone();
        let ctx_a2 = make_ctx("/sys/pc88_a", "/save/pc88_a");

        let handle_a = std::thread::spawn(move || {
            let guard = Pc88SessionGuard::acquire(&stop_a2_thread).unwrap();
            assert!(guard.install_bridge(ctx_a2, &stop_a2_thread));

            let mut sys_ptr: *const c_char = std::ptr::null();
            let mut save_ptr: *const c_char = std::ptr::null();
            unsafe {
                assert!(bridge_environment_cb(
                    RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY,
                    (&mut sys_ptr as *mut *const c_char).cast(),
                ));
                assert!(bridge_environment_cb(
                    RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY,
                    (&mut save_ptr as *mut *const c_char).cast(),
                ));
            }
            a_run_b.wait();
            a_fin_b.wait();

            guard.mark_stopping();
            let sys_str = unsafe { CStr::from_ptr(sys_ptr).to_str().unwrap().to_string() };
            let save_str = unsafe { CStr::from_ptr(save_ptr).to_str().unwrap().to_string() };
            assert_eq!(sys_str, "/sys/pc88_a");
            assert_eq!(save_str, "/save/pc88_a");
            drop(guard);
        });

        a_running_barrier.wait();
        stop_a2.store(true, Ordering::SeqCst);

        let stop_b2_thread = stop_b2.clone();
        let ctx_b2 = make_ctx("/sys/pc88_b", "/save/pc88_b");
        let handle_b = std::thread::spawn(move || {
            let guard = Pc88SessionGuard::acquire(&stop_b2_thread).unwrap();
            assert!(guard.install_bridge(ctx_b2, &stop_b2_thread));
            let mut sys_ptr: *const c_char = std::ptr::null();
            unsafe {
                assert!(bridge_environment_cb(
                    RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY,
                    (&mut sys_ptr as *mut *const c_char).cast(),
                ));
                let sys_str = CStr::from_ptr(sys_ptr).to_str().unwrap();
                assert_eq!(sys_str, "/sys/pc88_b");
            }
            drop(guard);
        });

        std::thread::sleep(Duration::from_millis(25));
        a_allow_finish_barrier.wait();

        handle_a.join().unwrap();
        handle_b.join().unwrap();
        assert_eq!(pc88_lifecycle_state(), EmulatorLifecycleState::Idle);
        assert!(ACTIVE_PC88_BRIDGE.lock().unwrap().is_none());
    }

    #[test]
    fn test_pc88_lock_free_input_latch_and_rtrb_audio_batch() {
        let input_state = Arc::new(Mutex::new(Pc98InputState::default()));
        {
            let mut inp = input_state.lock().unwrap();
            inp.joypad_buttons = 1 << 0;
            inp.analog_axes[0][0] = -12_345;
            inp.keys[crate::pc98::input::RETROK_RETURN as usize] = true;
            inp.pulse_key(RETROK_F12, 1);
            inp.mouse_dx = 32;
            inp.mouse_dy = -16;
            inp.mouse_middle = true;
        }

        let (prod, cons) = RingBuffer::<f32>::new(PC88_AUDIO_RING_CAPACITY);
        let volume_bits = Arc::new(AtomicU32::new(0.5f32.to_bits()));
        let stop = Arc::new(AtomicBool::new(false));
        let flush = Arc::new(AtomicBool::new(false));
        let mut source = Pc88AudioSource::new(
            cons,
            Arc::clone(&volume_bits),
            Arc::clone(&stop),
            Arc::clone(&flush),
        );
        assert_eq!(source.current_frame_len(), None);

        let rt_cell = UnsafeCell::new(Pc88RealtimeState {
            input_state: Arc::clone(&input_state),
            latched_input: Pc88LatchedInputState::default(),
            audio_producer: Some(prod),
            peak_amplitude: Arc::new(AtomicU64::new(0)),
            audio_latency_ms: Arc::new(AtomicU64::new(0)),
            replay_audio: Arc::new(crate::replay::AudioTap::default()),
            seek_mixer: FloppySeekMixer::new(false, 0),
            rumble_seek_frames: 0,
            resampler: ContinuousStereoResampler::new(
                PC88_AUDIO_SAMPLE_RATE,
                PC88_REPLAY_SAMPLE_RATE,
            ),
        });

        // Hold ACTIVE_PC88_BRIDGE locked while running callbacks inside Pc88RetroRunScopeGuard
        // to prove zero global bridge mutex locks occur on the real-time path.
        let _bridge_lock = ACTIVE_PC88_BRIDGE.lock().unwrap();
        {
            let _run_guard = Pc88RetroRunScopeGuard::enter(&rt_cell);
            unsafe {
                bridge_input_poll_cb();

                // Joypad button 0 + pulsed F12 mapped to Joypad Select (id 2)
                assert_eq!(bridge_input_state_cb(0, RETRO_DEVICE_JOYPAD, 0, 0), 1);
                assert_eq!(
                    bridge_input_state_cb(0, RETRO_DEVICE_JOYPAD, 0, RETRO_DEVICE_ID_JOYPAD_SELECT),
                    1
                );
                assert_eq!(
                    bridge_input_state_cb(0, crate::pc98::input::RETRO_DEVICE_ANALOG, 0, 0),
                    -12_345
                );
                assert_eq!(
                    bridge_input_state_cb(
                        0,
                        RETRO_DEVICE_KEYBOARD,
                        0,
                        crate::pc98::input::RETROK_RETURN
                    ),
                    1
                );
                assert_eq!(
                    bridge_input_state_cb(0, RETRO_DEVICE_MOUSE, 0, RETRO_DEVICE_ID_MOUSE_X),
                    32
                );
                assert_eq!(
                    bridge_input_state_cb(0, RETRO_DEVICE_MOUSE, 0, RETRO_DEVICE_ID_MOUSE_Y),
                    -16
                );
                assert_eq!(
                    bridge_input_state_cb(0, RETRO_DEVICE_MOUSE, 0, RETRO_DEVICE_ID_MOUSE_MIDDLE),
                    1
                );

                // Push 300 stereo frames (600 samples > 512 batch size)
                let raw = vec![16_384i16; 600];
                assert_eq!(bridge_audio_sample_batch_cb(raw.as_ptr(), 300), 300);
            }
        }
        assert!(CURRENT_INPUT_LATCH.with(|c| c.get().is_null()));
        assert!(CURRENT_PC88_REALTIME_STATE.with(|c| c.get().is_null()));
        drop(_bridge_lock);

        for _ in 0..600 {
            let s = source.next().unwrap();
            assert!((s - 0.25).abs() < 1e-5, "unexpected scaled sample {s}");
        }

        stop.store(true, Ordering::Relaxed);
        assert_eq!(source.next(), None);
    }

    #[test]
    fn test_pc88_frame_pacing_no_drift_on_overshoot_and_resync_on_lag() {
        let base = Instant::now();
        let frame_interval = Duration::from_micros(16_683);
        let mut deadline = base + frame_interval;

        // 1. Frame finishes 4ms early: sleeps 4ms and advances deadline by exact frame_interval.
        let (sleep_dur, d1) = advance_frame_deadline(
            deadline,
            deadline - Duration::from_millis(4),
            frame_interval,
        );
        assert_eq!(sleep_dur, Some(Duration::from_millis(4)));
        assert_eq!(d1, base + frame_interval * 2);
        deadline = d1;

        // 2. OS sleep overshoots by 250us: does NOT reset to `now + frame_interval` (zero drift!).
        let overshoot_now = deadline + Duration::from_micros(250);
        let (sleep_dur, d2) = advance_frame_deadline(deadline, overshoot_now, frame_interval);
        assert_eq!(sleep_dur, None);
        assert_eq!(d2, base + frame_interval * 3);
        deadline = d2;

        // 3. Up to 4 * frame_interval behind still catches up without phase drift.
        let catchup_now = deadline + frame_interval * 4;
        let (sleep_dur, d3) = advance_frame_deadline(deadline, catchup_now, frame_interval);
        assert_eq!(sleep_dur, None);
        assert_eq!(d3, base + frame_interval * 4);
        deadline = d3;

        // 4. Severe stall (> 4 * frame_interval) resyncs cleanly to `now + frame_interval`.
        let lag_now = deadline + frame_interval * 4 + Duration::from_micros(1);
        let (sleep_dur, d4) = advance_frame_deadline(deadline, lag_now, frame_interval);
        assert_eq!(sleep_dur, None);
        assert_eq!(d4, lag_now + frame_interval);
    }
}
