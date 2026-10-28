use crate::config;
use crate::devices::{self};
use crate::video::{types::RawFrame, VideoFormat};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8},
    Arc, Mutex,
};
use std::thread::JoinHandle;
use std::time::Instant;

pub struct HardwareState {
    pub video_queue_drops: Arc<AtomicU64>,
    pub audio_peak_amplitude: Arc<AtomicU64>,
    pub audio_latency_ms: Arc<AtomicU64>,
    pub audio_buffer_size: u32,
    pub audio_sample_rate: u32,
    pub audio_sample_format: String,
    pub master_volume: f32,
    pub live_audio_peak: f32,
    pub live_mic_peak: f32,
    pub video_devices: Vec<String>,
    pub usb_devices: Vec<(String, String)>,
    pub selected_usb_device: Option<String>,
    pub selected_video_device: String,
    pub audio_sources: Vec<(String, String)>,
    pub selected_audio_source_name: Option<String>,
    pub active_audio_stream: Option<devices::audio::AudioStreamHandle>,
    pub supported_formats: Vec<VideoFormat>,
    pub selected_format_index: usize,
    pub selected_resolution: (u32, u32),
    pub selected_framerate: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    Shaders,
    GeometryBezel,
    SpecialFx,
    CaptureReplay,
    AudioMic,
    Pc98,
    OcrDict,
    Twitch,
    Hotkeys,
}

pub struct UiState {
    pub debug_open: bool,
    pub is_fullscreen: bool,
    pub reset_usb_on_startup: bool,
    pub show_first_run_dialog: bool,
    pub show_quit_dialog: bool,
    pub show_stop_stream_dialog: bool,
    pub video_window_open: bool,
    pub control_window_open: bool,
    pub studio_window_open: bool,
    pub dismissed_config_error: bool,
    pub active_settings_tab: SettingsTab,
    pub config_save_pending: bool,
    pub in_egui_frame: bool,
    pub any_pointer_down: bool,
}

pub struct CrtSettings {
    pub hard_scan: f32,
    pub warp_x: f32,
    pub warp_y: f32,
    pub shadow_mask: f32,
    pub brightboost: f32,
    pub hard_bloom_pix: f32,
    pub hard_bloom_scan: f32,
    pub bloom_amount: f32,
    pub shape: f32,
    pub hard_pix: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HaloSettings {
    pub brightboost: f32,
    pub brightboost1: f32,
    pub beam_min: f32,
    pub beam_max: f32,
    pub beam_size: f32,
    pub h_sharp: f32,
    pub glow: f32,
    pub bloom: f32,
    pub halation: f32,
    pub shadow_mask: f32,
    pub masksize: f32,
    pub maskstr: f32,
    pub mcut: f32,
    pub slotmask: f32,
    pub slotmask1: f32,
    pub double_slot: f32,
    pub smoothmask: f32,
    pub halo_zoom: f32,
    pub halo_intensity: f32,
    pub corner_size: f32,
    pub curvature: bool,
}

impl Default for HaloSettings {
    fn default() -> Self {
        Self {
            brightboost: 1.30,
            brightboost1: 1.65,
            beam_min: 1.80,
            beam_max: 1.65,
            beam_size: 1.25,
            h_sharp: 4.20,
            glow: 0.53,
            bloom: 0.57,
            halation: 0.23,
            shadow_mask: 6.0,
            masksize: 2.0,
            maskstr: 0.50,
            mcut: 0.80,
            slotmask: 0.45,
            slotmask1: 0.30,
            double_slot: 3.0,
            smoothmask: 0.80,
            halo_zoom: 100.0,
            halo_intensity: 0.95,
            corner_size: 0.0,
            curvature: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CathodeInterferenceSettings {
    pub enabled: bool,
    pub intensity: f32,
    pub frequency: f32,
    pub randomization: f32,
    pub electricity_glow: f32,
    pub flicker_depth: f32,
    pub interference: f32,
    pub lightbulb_effect: f32,
}

impl Default for CathodeInterferenceSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            intensity: 0.5,
            frequency: 1.0,
            randomization: 0.5,
            electricity_glow: 0.5,
            flicker_depth: 0.4,
            interference: 0.3,
            lightbulb_effect: 0.35,
        }
    }
}

pub const AUDIO_REACTIVE_DEFAULT_PRESET: &str = "Arcade Woofer Punch";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioReactiveParams {
    pub master_intensity: f32,
    pub input_gain: f32,
    pub gate_threshold: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub low_weight: f32,
    pub mid_weight: f32,
    pub high_weight: f32,
    pub deflection_shake: f32,
    pub stereo_tug: f32,
    pub flyback_pump: f32,
    pub rgb_deconvergence: f32,
    pub scanline_wave: f32,
    pub phosphor_surge: f32,
    pub sync_tear: f32,
}

impl Default for AudioReactiveParams {
    fn default() -> Self {
        Self::preset_arcade_woofer_punch()
    }
}

impl AudioReactiveParams {
    pub fn preset_subtle_speaker_hum() -> Self {
        Self {
            master_intensity: 0.55,
            input_gain: 1.15,
            gate_threshold: 0.04,
            attack_ms: 14.0,
            release_ms: 180.0,
            low_weight: 1.00,
            mid_weight: 0.70,
            high_weight: 0.50,
            deflection_shake: 0.25,
            stereo_tug: 0.22,
            flyback_pump: 0.20,
            rgb_deconvergence: 0.22,
            scanline_wave: 0.18,
            phosphor_surge: 0.18,
            sync_tear: 0.05,
        }
    }

    pub fn preset_arcade_woofer_punch() -> Self {
        Self {
            master_intensity: 1.00,
            input_gain: 1.35,
            gate_threshold: 0.03,
            attack_ms: 8.0,
            release_ms: 140.0,
            low_weight: 1.35,
            mid_weight: 0.90,
            high_weight: 0.80,
            deflection_shake: 0.65,
            stereo_tug: 0.50,
            flyback_pump: 0.55,
            rgb_deconvergence: 0.45,
            scanline_wave: 0.35,
            phosphor_surge: 0.42,
            sync_tear: 0.22,
        }
    }

    pub fn preset_fm_chiptune_oscilloscope() -> Self {
        Self {
            master_intensity: 1.05,
            input_gain: 1.40,
            gate_threshold: 0.02,
            attack_ms: 5.0,
            release_ms: 110.0,
            low_weight: 0.85,
            mid_weight: 1.45,
            high_weight: 1.30,
            deflection_shake: 0.35,
            stereo_tug: 0.55,
            flyback_pump: 0.25,
            rgb_deconvergence: 0.60,
            scanline_wave: 0.85,
            phosphor_surge: 0.48,
            sync_tear: 0.28,
        }
    }

    pub fn preset_overdriven_flyback() -> Self {
        Self {
            master_intensity: 1.35,
            input_gain: 1.60,
            gate_threshold: 0.02,
            attack_ms: 5.0,
            release_ms: 165.0,
            low_weight: 1.50,
            mid_weight: 1.10,
            high_weight: 1.20,
            deflection_shake: 0.90,
            stereo_tug: 0.70,
            flyback_pump: 0.95,
            rgb_deconvergence: 0.80,
            scanline_wave: 0.65,
            phosphor_surge: 0.85,
            sync_tear: 0.60,
        }
    }

    pub fn preset_max_cyber_overload() -> Self {
        Self {
            master_intensity: 1.75,
            input_gain: 1.85,
            gate_threshold: 0.015,
            attack_ms: 3.5,
            release_ms: 190.0,
            low_weight: 1.65,
            mid_weight: 1.45,
            high_weight: 1.60,
            deflection_shake: 1.30,
            stereo_tug: 1.00,
            flyback_pump: 1.20,
            rgb_deconvergence: 1.25,
            scanline_wave: 1.15,
            phosphor_surge: 1.15,
            sync_tear: 1.05,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioReactiveConfig {
    // Scalar fields before nested tables for TOML compatibility
    pub enabled: bool,
    pub active_preset: String,
    pub current: AudioReactiveParams,
    pub presets: BTreeMap<String, AudioReactiveParams>,
}

impl Default for AudioReactiveConfig {
    fn default() -> Self {
        let presets = Self::builtin_presets();
        let active_preset = AUDIO_REACTIVE_DEFAULT_PRESET.to_string();
        let current = presets.get(&active_preset).cloned().unwrap_or_default();
        Self {
            enabled: false,
            active_preset,
            current,
            presets,
        }
    }
}

impl AudioReactiveConfig {
    pub fn builtin_presets() -> BTreeMap<String, AudioReactiveParams> {
        let mut map = BTreeMap::new();
        map.insert(
            "Subtle Speaker Hum".to_string(),
            AudioReactiveParams::preset_subtle_speaker_hum(),
        );
        map.insert(
            "Arcade Woofer Punch".to_string(),
            AudioReactiveParams::preset_arcade_woofer_punch(),
        );
        map.insert(
            "FM Chiptune Oscilloscope".to_string(),
            AudioReactiveParams::preset_fm_chiptune_oscilloscope(),
        );
        map.insert(
            "Overdriven Flyback (Juiced)".to_string(),
            AudioReactiveParams::preset_overdriven_flyback(),
        );
        map.insert(
            "Max Cyber-Overload".to_string(),
            AudioReactiveParams::preset_max_cyber_overload(),
        );
        map
    }

    pub fn builtin_preset(name: &str) -> Option<AudioReactiveParams> {
        Self::builtin_presets().remove(name)
    }

    pub fn is_builtin_preset(name: &str) -> bool {
        matches!(
            name,
            "Subtle Speaker Hum"
                | "Arcade Woofer Punch"
                | "FM Chiptune Oscilloscope"
                | "Overdriven Flyback (Juiced)"
                | "Max Cyber-Overload"
        )
    }

    pub fn ensure_builtin_presets(&mut self) {
        for (name, preset) in Self::builtin_presets() {
            self.presets.entry(name).or_insert(preset);
        }
        if self.active_preset.trim().is_empty() || !self.presets.contains_key(&self.active_preset) {
            self.active_preset = AUDIO_REACTIVE_DEFAULT_PRESET.to_string();
        }
    }
}

#[derive(Clone, Debug)]
pub struct AudioReactiveRuntime {
    pub low_env: f32,
    pub mid_env: f32,
    pub high_env: f32,
    pub transient_env: f32,
    pub slow_energy: f32,
    pub stereo_balance: f32,
    pub spring_pos_x: f32,
    pub spring_vel_x: f32,
    pub spring_pos_y: f32,
    pub spring_vel_y: f32,
    pub phase_accum: f32,
    pub last_step: Instant,
    pub new_preset_name: String,
}

impl Default for AudioReactiveRuntime {
    fn default() -> Self {
        Self {
            low_env: 0.0,
            mid_env: 0.0,
            high_env: 0.0,
            transient_env: 0.0,
            slow_energy: 0.0,
            stereo_balance: 0.0,
            spring_pos_x: 0.0,
            spring_vel_x: 0.0,
            spring_pos_y: 0.0,
            spring_vel_y: 0.0,
            phase_accum: 0.0,
            last_step: Instant::now(),
            new_preset_name: String::new(),
        }
    }
}

impl AudioReactiveRuntime {
    pub fn step(
        &mut self,
        raw: (f32, f32, f32, f32, f32),
        fallback_peak: f32,
        params: &AudioReactiveParams,
        active: bool,
    ) {
        let now = Instant::now();
        let dt = now
            .duration_since(self.last_step)
            .as_secs_f32()
            .clamp(0.001, 0.08);
        self.last_step = now;

        if !active {
            let decay = (-dt * 12.0).exp();
            self.low_env *= decay;
            self.mid_env *= decay;
            self.high_env *= decay;
            self.transient_env *= decay;
            self.slow_energy *= decay;
            self.stereo_balance *= decay;
            self.spring_pos_x *= decay;
            self.spring_vel_x *= decay;
            self.spring_pos_y *= decay;
            self.spring_vel_y *= decay;
            return;
        }

        let (mut r_low, mut r_mid, mut r_high, r_left, r_right) = raw;
        if (r_low + r_mid + r_high) < 0.0005 && fallback_peak > 0.002 {
            let p = fallback_peak.powf(0.65) * 1.35;
            r_low = p * 0.95;
            r_mid = p * 0.80;
            r_high = p * 0.60;
        }

        let gate_thresh = (params.gate_threshold * 0.65).clamp(0.0, 0.5);
        let gain = params.input_gain.max(0.0);
        let gate_map = |val: f32, weight: f32| -> f32 {
            let v = val * gain;
            if v <= gate_thresh {
                0.0
            } else {
                let norm = (v - gate_thresh) / (1.0 - gate_thresh * 0.5).max(0.1);
                (norm * weight.max(0.0)).clamp(0.0, 3.0)
            }
        };

        let target_low = gate_map(r_low, params.low_weight);
        let target_mid = gate_map(r_mid, params.mid_weight);
        let target_high = gate_map(r_high, params.high_weight);

        let attack_tau = (params.attack_ms.max(1.0)) * 0.001;
        let release_tau = (params.release_ms.max(15.0)) * 0.001;
        let alpha_atk = 1.0 - (-dt / attack_tau).exp();
        let alpha_rel = 1.0 - (-dt / release_tau).exp();

        let smooth_step = |cur: f32, target: f32| -> f32 {
            let a = if target > cur { alpha_atk } else { alpha_rel };
            (cur + (target - cur) * a).clamp(0.0, 3.0)
        };

        let prev_low = self.low_env;
        self.low_env = smooth_step(self.low_env, target_low);
        self.mid_env = smooth_step(self.mid_env, target_mid);
        self.high_env = smooth_step(self.high_env, target_high);

        // Fast-minus-slow transient impulse detector
        let inst_energy = target_low * 0.55 + target_mid * 0.35 + target_high * 0.40;
        let slow_alpha = 1.0 - (-dt / 0.18).exp();
        self.slow_energy += (inst_energy - self.slow_energy) * slow_alpha;
        let raw_impulse = (inst_energy - self.slow_energy * 0.78).max(0.0) * 2.35;
        let trans_alpha = if raw_impulse > self.transient_env {
            1.0 - (-dt / 0.003).exp()
        } else {
            1.0 - (-dt / 0.085).exp()
        };
        self.transient_env =
            (self.transient_env + (raw_impulse - self.transient_env) * trans_alpha).clamp(0.0, 3.0);

        // Stereo balance [-1.0 (left), +1.0 (right)]
        let lr_sum = r_left + r_right;
        let target_stereo = if lr_sum > 0.004 {
            ((r_right - r_left) / (lr_sum + 0.012)).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let stereo_alpha = 1.0 - (-dt / 0.030).exp();
        self.stereo_balance = (self.stereo_balance
            + (target_stereo - self.stereo_balance) * stereo_alpha)
            .clamp(-1.0, 1.0);

        // Underdamped 2D harmonic spring oscillator for magnetic deflection yoke shake
        let bass_kick = (self.low_env - prev_low).max(0.0) * 32.0
            + self.transient_env * 11.5
            + self.low_env * 3.2;
        let sign_x = (self.phase_accum * 3.7).sin() + self.stereo_balance * 0.75;
        let sign_y = (self.phase_accum * 2.3 + 1.1).cos();
        self.spring_vel_x += bass_kick * sign_x;
        self.spring_vel_y += bass_kick * sign_y;

        // Sub-step spring physics for unconditional stability across frame rates
        let substeps = 4;
        let sub_dt = dt / (substeps as f32);
        let omega_x = 46.0f32;
        let omega_y = 38.0f32;
        let damping = 12.0f32;
        for _ in 0..substeps {
            let acc_x = -omega_x * omega_x * self.spring_pos_x - damping * self.spring_vel_x;
            let acc_y = -omega_y * omega_y * self.spring_pos_y - damping * self.spring_vel_y;
            self.spring_vel_x = (self.spring_vel_x + acc_x * sub_dt).clamp(-30.0, 30.0);
            self.spring_vel_y = (self.spring_vel_y + acc_y * sub_dt).clamp(-30.0, 30.0);
            self.spring_pos_x = (self.spring_pos_x + self.spring_vel_x * sub_dt).clamp(-2.2, 2.2);
            self.spring_pos_y = (self.spring_pos_y + self.spring_vel_y * sub_dt).clamp(-2.2, 2.2);
        }

        // Advance continuous phase for scanline voice-coil wave
        let phase_speed = 6.0 + self.mid_env * 22.0 + self.low_env * 10.0;
        self.phase_accum = (self.phase_accum + dt * phase_speed) % 1000.0;
    }
}

pub const LIGHT_SWITCH_FLICKER_DURATION_SECS: f32 = 0.73;

pub struct VideoSettings {
    pub pixelate_filter_enabled: bool,
    pub use_magenta_background: bool,
    pub retro_pc_frame: bool,
    pub retro_pc_frame_dark_mode: bool,
    pub retro_pc_ambient_glow: f32,
    pub retro_software_mouse: bool,
    pub lights_off_night_mode: bool,
    pub realistic_light_switch: bool,
    pub light_switch_flicker_start: Option<std::time::Instant>,
    pub night_mode_glow_intensity: f32,
    pub crt_glass_enabled: bool,
    pub crt_glass_intensity: f32,
    pub crt_glass_glossiness: f32,
    pub crt_glass_ceiling_light_enabled: bool,
    pub crt_glass_photographer_enabled: bool,
    pub crt_glass_photographer_intensity: f32,
    pub crt_glass_flash_enabled: bool,
    pub crt_glass_flash_intensity: f32,
    pub horizontal_stretch: f32,
    pub median_filter_enabled: bool,
    pub median_mix: f32,
    pub deinterlace_filter_enabled: bool,
    pub deinterlace_mode: u8,
    pub deinterlace_blend: f32,
    pub deinterlace_motion_threshold: f32,
    pub deinterlace_line_spacing: f32,
    pub deinterlace_spatial_mix: f32,
    pub vibrance: f32,
    pub overscan_x: f32,
    pub overscan_y: f32,
    pub underscan_x: f32,
    pub underscan_y: f32,
    pub border_crop_left: f32,
    pub border_crop_right: f32,
    pub border_crop_top: f32,
    pub border_crop_bottom: f32,
    pub fft_filter_enabled: bool,
    pub fft_mask_window_open: bool,
}

impl VideoSettings {
    /// Evaluates whether the ceiling light is in a dark (off) phase at `elapsed_secs` seconds
    /// into a dark-to-light (`lights_off_night_mode`: `true` -> `false`) transition.
    ///
    /// Simulates the starter/ballast ignition strikes of an overhead ceiling tube flickering
    /// on and off several times before stabilizing fully ON.
    #[inline]
    pub fn evaluate_light_switch_flicker_is_dark(elapsed_secs: f32) -> bool {
        if elapsed_secs < 0.0 || elapsed_secs >= LIGHT_SWITCH_FLICKER_DURATION_SECS {
            return false;
        }
        // Ignition strike timeline (0.73s total, 4 distinct flashes before catching):
        //   0.00..0.07s: Dark (starter pre-heat delay right after switch clack)
        //   0.07..0.14s: Strike 1 ON
        //   0.14..0.28s: Dark (arc drops out)
        //   0.28..0.34s: Strike 2 ON (quick stutter flash)
        //   0.34..0.41s: Dark (sputter gap)
        //   0.41..0.51s: Strike 3 ON (longer warm strike)
        //   0.51..0.62s: Dark (brief dropout before catch)
        //   0.62..0.68s: Strike 4 ON (rapid catch flash)
        //   0.68..0.73s: Dark (final quick blink)
        //   >= 0.73s:    Stabilized ON (false)
        matches!(
            elapsed_secs,
            t if t < 0.07
                || (0.14..0.28).contains(&t)
                || (0.34..0.41).contains(&t)
                || (0.51..0.62).contains(&t)
                || (0.68..LIGHT_SWITCH_FLICKER_DURATION_SECS).contains(&t)
        )
    }

    /// Advances the dark-to-light ceiling tube flicker animation state and returns true
    /// if the viewport should schedule a 60Hz repaint for the transition.
    #[inline]
    pub fn advance_light_switch_flicker(&mut self) -> bool {
        if let Some(start) = self.light_switch_flicker_start {
            if !self.realistic_light_switch || self.lights_off_night_mode {
                self.light_switch_flicker_start = None;
                false
            } else if start.elapsed().as_secs_f32() < LIGHT_SWITCH_FLICKER_DURATION_SECS {
                true
            } else {
                self.light_switch_flicker_start = None;
                true
            }
        } else {
            false
        }
    }

    /// Returns the effective `lights_off_night_mode` state for rendering, accounting for
    /// transient ceiling tube flickering during a dark-to-light transition when
    /// `realistic_light_switch` is enabled.
    #[inline]
    pub fn effective_lights_off_night_mode(&self) -> bool {
        if self.lights_off_night_mode {
            return true;
        }
        if self.realistic_light_switch {
            if let Some(start) = self.light_switch_flicker_start {
                return Self::evaluate_light_switch_flicker_is_dark(start.elapsed().as_secs_f32());
            }
        }
        false
    }
}

pub struct PendingAudioStream {
    pub source_name: String,
    pub buffer_size: u32,
    pub sample_rate: u32,
    pub sample_format: String,
}

pub struct VideoPlayerState {
    pub file_path: std::path::PathBuf,
    pub duration_secs: f64,
    pub current_position_secs: f64,
    pub is_paused: bool,
    pub is_eof: bool,
    pub is_seeking: bool,
    pub was_playing_before_scrub: bool,
    pub command_sender: crossbeam_channel::Sender<crate::video::player::PlayerCommand>,
    pub position_us: Arc<std::sync::atomic::AtomicI64>,
    pub is_paused_shared: Arc<AtomicBool>,
    pub is_eof_shared: Arc<AtomicBool>,
    pub stop_requested: Arc<AtomicBool>,
    pub file_width: u32,
    pub file_height: u32,
    pub player_thread: Option<JoinHandle<()>>,
}

pub struct MicState {
    pub config: crate::config::MicConfig,
    pub active_stream: Option<crate::devices::mic::MicStreamHandle>,
    pub current_level_db: Arc<AtomicU32>,
    pub peak_level_db: Arc<AtomicU32>,
    pub is_clipping: Arc<AtomicBool>,
}

impl Default for MicState {
    fn default() -> Self {
        Self {
            config: crate::config::MicConfig::default(),
            active_stream: None,
            current_level_db: Arc::new(AtomicU32::new((-100.0f32).to_bits())),
            peak_level_db: Arc::new(AtomicU32::new((-100.0f32).to_bits())),
            is_clipping: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebcamResizeCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

pub struct WebcamState {
    pub config: crate::config::WebcamConfig,
    pub supported_formats: Vec<VideoFormat>,
    pub formats_probed_for_device: Option<String>,
    pub selected_format_index: usize,
    pub active_stream: Option<crate::devices::webcam::WebcamStreamHandle>,
    pub texture: Option<egui::TextureHandle>,
    pub latest_frame: Option<Arc<crate::devices::webcam::WebcamFrame>>,
    pub frame_receiver:
        Option<crossbeam_channel::Receiver<Arc<crate::devices::webcam::WebcamFrame>>>,
    pub is_dragging: bool,
    pub drag_start_pointer: Option<egui::Pos2>,
    pub drag_start_pos: Option<[f32; 2]>,
    pub resizing_corner: Option<WebcamResizeCorner>,
    pub resize_start_pointer: Option<egui::Pos2>,
    pub resize_start_width_pct: Option<f32>,
    pub resize_start_pos: Option<[f32; 2]>,
    pub last_outer_rect: Option<egui::Rect>,
}

impl Default for WebcamState {
    fn default() -> Self {
        Self {
            config: crate::config::WebcamConfig::default(),
            supported_formats: Vec::new(),
            formats_probed_for_device: None,
            selected_format_index: 0,
            active_stream: None,
            texture: None,
            latest_frame: None,
            frame_receiver: None,
            is_dragging: false,
            drag_start_pointer: None,
            drag_start_pos: None,
            resizing_corner: None,
            resize_start_pointer: None,
            resize_start_width_pct: None,
            resize_start_pos: None,
            last_outer_rect: None,
        }
    }
}

impl WebcamState {
    pub fn poll_frame(&mut self) -> bool {
        let mut received = false;
        if let Some(rx) = &self.frame_receiver {
            while let Ok(frame) = rx.try_recv() {
                self.latest_frame = Some(frame);
                received = true;
            }
        }
        received
    }
}

pub struct AppState {
    pub steam_deck: Option<crate::steam_deck::SteamDeckState>,
    pub replay: crate::replay::Replay,
    pub hardware: HardwareState,
    pub ui: UiState,
    pub crt: CrtSettings,
    pub halo: HaloSettings,
    pub halo_defaults: HaloSettings,
    pub cathode_interference: CathodeInterferenceSettings,
    pub cathode_interference_defaults: CathodeInterferenceSettings,
    pub audio_reactive: AudioReactiveConfig,
    pub audio_reactive_runtime: AudioReactiveRuntime,
    pub selected_crt_filter: crate::devices::filter_type::CrtFilter,
    pub video: VideoSettings,
    pub toasts: egui_toast::Toasts,

    pub video_thread: Option<JoinHandle<()>>,
    pub video_stop_requested: Option<Arc<AtomicBool>>,
    pub video_texture: Option<egui::TextureHandle>,
    pub latest_frame: Option<Arc<RawFrame>>,
    pub frame_receiver: Option<crossbeam_channel::Receiver<Arc<RawFrame>>>,
    pub video_status_receiver:
        Option<crossbeam_channel::Receiver<crate::video::decoder::VideoThreadEvent>>,
    pub video_player: Option<VideoPlayerState>,
    pub video_player_event_receiver:
        Option<crossbeam_channel::Receiver<crate::video::player::PlayerEvent>>,
    pub video_file_dialog_receiver: Option<crossbeam_channel::Receiver<std::path::PathBuf>>,
    pub pending_audio_stream: Option<PendingAudioStream>,
    pub device_scan_receiver: Option<crossbeam_channel::Receiver<devices::DeviceScanResult>>,
    pub logo_texture: Option<egui::TextureHandle>,
    pub gui_fps: f32,
    pub video_fps: f32,
    pub last_fps_check: Instant,
    pub frames_since_last_check: u32,
    pub last_video_fps_check: Instant,
    pub video_frames_since_last_check: u32,
    pub last_window_title: String,
    pub next_gui_frame_time: Option<Instant>,

    pub crt_filter: Arc<AtomicU8>,
    pub scaler_filter: Arc<AtomicU8>,
    pub color_range: Arc<AtomicU8>,
    pub crt_renderer: Option<Arc<Mutex<crate::video::gpu::CrtFilterRenderer>>>,
    pub fullscreen_toggle_frame_count: Option<u8>,

    pub profiles: BTreeMap<String, config::Profile>,
    pub active_profile: String,
    pub new_profile_name: String,
    pub video_profile: config::Profile,
    pub gamescope_profile: config::Profile,
    pub pc98_profile: config::Profile,
    pub pc88_profile: config::Profile,
    pub previous_normal_profile: Option<String>,
    pub previous_gamescope_normal_profile: Option<String>,
    pub previous_pc98_normal_profile: Option<String>,
    pub previous_pc88_normal_profile: Option<String>,
    pub previous_capture_resolution: (u32, u32),
    pub previous_capture_framerate: u32,
    pub gamescope_hook: Option<crate::gamescope::GamescopeHookState>,
    pub gamescope_launcher_script: String,
    pub steam_games: Vec<crate::gamescope::steam::SteamGame>,
    pub selected_steam_game: Option<String>,
    pub gamescope_script_dialog_receiver: Option<crossbeam_channel::Receiver<std::path::PathBuf>>,
    pub retro_pc_system: crate::pc88::RetroPcSystem,
    pub pc98: crate::pc98::Pc98State,
    pub pc88: crate::pc88::Pc88State,

    pub fft_filter: Option<Arc<Mutex<crate::video::gpu::FftFilter>>>,
    pub fft_mask_data: Vec<u8>,
    pub fft_mask_dirty: bool,
    pub fft_mask_resolution: (u32, u32),
    pub fft_brush_radius: f32,
    pub fft_mask_threshold: f32,
    pub fft_black_threshold: f32,
    pub fft_mask_save_name: String,
    pub fft_available_masks: Vec<String>,
    pub ocr: crate::ocr::OcrState,
    pub dict: crate::dict::DictState,
    pub bank: crate::bank::BankState,
    pub twitch: crate::twitch::TwitchState,
    pub mic: MicState,
    pub webcam: WebcamState,
    pub whiteboard: crate::ui::whiteboard::WhiteboardState,
    pub config_load_error: Option<String>,
    pub config_quarantine_path: Option<std::path::PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn test_app_state_default() {
        let state = AppState::default();
        assert_eq!(state.active_profile, "Default");
        assert!(state.profiles.contains_key("Default"));
        assert!(state.hardware.video_devices.is_empty());
        assert!(!state.ui.is_fullscreen);
        assert!(state.ui.control_window_open);
        assert!(!state.ui.studio_window_open);
        assert!(!state.ui.video_window_open);
        assert!(state.video_status_receiver.is_none());
        assert!(state.pending_audio_stream.is_none());
        assert_eq!(state.crt.hard_scan, -8.0);
        assert_eq!(state.halo.brightboost, 1.30);
        assert_eq!(state.halo_defaults, state.halo);
        assert_eq!(
            state.cathode_interference_defaults,
            state.cathode_interference
        );
        assert!(!state.cathode_interference.enabled);
        assert!(state.config_load_error.is_none());
        assert!(state.config_quarantine_path.is_none());
        assert_eq!(
            state.selected_crt_filter,
            crate::devices::filter_type::CrtFilter::Lottes
        );
        assert_eq!(state.video.horizontal_stretch, 1.0);
    }

    #[test]
    fn test_hardware_state_defaults() {
        let state = AppState::default();
        assert_eq!(state.hardware.audio_buffer_size, 1024);
        assert_eq!(state.hardware.audio_sample_rate, 48000);
        assert_eq!(state.hardware.audio_sample_format, "S16LE");
        assert_eq!(state.hardware.master_volume, 1.0);
        assert_eq!(state.hardware.live_audio_peak, 0.0);
        assert_eq!(state.hardware.live_mic_peak, 0.0);
        assert!(state.hardware.video_devices.is_empty());
    }

    #[test]
    fn test_atomic_defaults() {
        let state = AppState::default();
        assert_eq!(
            state.crt_filter.load(Ordering::Relaxed),
            crate::devices::filter_type::CrtFilter::Off as u8
        );
        assert_eq!(
            state.scaler_filter.load(Ordering::Relaxed),
            crate::video::types::ScalerFilter::Bicubic as u8
        );
        assert_eq!(
            state.color_range.load(Ordering::Relaxed),
            crate::video::types::ColorRange::Full as u8
        );
    }

    #[test]
    fn test_audio_reactive_dsp_and_presets() {
        let mut state = AppState::default();
        assert!(!state.audio_reactive.enabled);
        assert_eq!(
            state.audio_reactive.active_preset,
            AUDIO_REACTIVE_DEFAULT_PRESET
        );
        assert_eq!(state.audio_reactive.presets.len(), 5);

        state.toggle_audio_reactive();
        assert!(state.audio_reactive.enabled);
        assert!(state.replay.audio.is_reactive_listening());

        // Synthesize a 90 Hz bass tone on the right channel at 48 kHz
        let mut samples = Vec::with_capacity(960 * 2);
        for i in 0..960 {
            let t = i as f32 / 48000.0;
            let s = (std::f32::consts::TAU * 90.0 * t).sin() * 0.8;
            samples.push(0.05); // Left quiet
            samples.push(s); // Right loud
        }
        state.replay.audio.observe_reactive(&samples, 48000, 2);
        let raw = state.replay.audio.take_reactive_raw();
        assert!(
            raw.0 > 0.1,
            "Low band should detect 90Hz bass: got {}",
            raw.0
        );
        assert!(raw.4 > raw.3, "Right channel should be louder than left");

        state.audio_reactive_runtime.last_step =
            Instant::now() - std::time::Duration::from_millis(16);
        state
            .audio_reactive_runtime
            .step(raw, 0.0, &state.audio_reactive.current, true);
        assert!(state.audio_reactive_runtime.low_env > 0.0);
        assert!(state.audio_reactive_runtime.stereo_balance > 0.0);
        assert!(
            state.audio_reactive_runtime.spring_pos_x.abs() > 0.0
                || state.audio_reactive_runtime.spring_pos_y.abs() > 0.0
        );
    }
}
