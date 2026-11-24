use crate::config;
use crate::devices::{self};
use crate::video::{types::RawFrame, VideoFormat};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
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
    /// Pre-fader linear peak (`0.0..=1.0`) for compact meters and audio-reactive fallback.
    pub live_audio_peak: f32,
    /// Post-fader dBFS peak normalized to `0.0..=1.0` (`-60..0 dBFS`) for the Studio VU meter.
    pub live_audio_db_peak: f32,
    /// Post-fader dBFS peak normalized to `0.0..=1.0` (`-60..0 dBFS`) for the Studio Mic VU meter.
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
    pub video_hwdec: bool,
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
    Ps5,
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
    pub config_save_deadline: Option<Instant>,
    pub in_egui_frame: bool,
    pub any_pointer_down: bool,
    pub is_window_focused: bool,
    pub last_cursor_grab: Option<egui::CursorGrab>,
    pub last_cursor_visible: Option<bool>,
    /// Guards async native file dialogs (`rfd::FileDialog`) so only one opens at a time.
    pub file_dialog_in_flight: Arc<AtomicBool>,
    /// Resolution-aware UI scale (desktop only).
    pub scale: crate::ui::scaling::UiScaleState,
}

impl UiState {
    pub fn is_file_dialog_open(&self) -> bool {
        self.file_dialog_in_flight
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn set_cursor_grab(&mut self, ctx: &egui::Context, grab: egui::CursorGrab) {
        if self.last_cursor_grab != Some(grab) {
            self.last_cursor_grab = Some(grab);
            ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(grab));
        }
    }

    #[allow(dead_code)]
    pub fn set_cursor_visible(&mut self, ctx: &egui::Context, visible: bool) {
        if self.last_cursor_visible != Some(visible) {
            self.last_cursor_visible = Some(visible);
            ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(visible));
        }
    }

    pub fn reset_cursor_state_cache(&mut self) {
        self.last_cursor_grab = None;
        self.last_cursor_visible = None;
    }
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

pub const HALO_PRESET_SONY_WEGA: &str = "Sony Trinitron WEGA";
pub const HALO_PRESET_SONY_90S: &str = "Sony Trinitron (90s)";
pub const HALO_PRESET_SONY_PVM_20M2: &str = "Sony PVM-20M2 (90s Broadcast)";
pub const HALO_PRESET_JVC_D_SERIES: &str = "JVC D-Series (Late 90s)";
pub const HALO_PRESET_TOSHIBA_AF: &str = "Toshiba AF Series (Early 00s)";
pub const HALO_PRESET_TOSHIBA_BLACKSTRIPE: &str = "Toshiba Blackstripe (Late 80s)";
pub const HALO_PRESET_PANASONIC_GAOO: &str = "Panasonic Gaoo (Early 90s)";
pub const HALO_PRESET_PANASONIC_TAU: &str = "Panasonic Tau (Early 00s)";
pub const HALO_PRESET_PHILIPS_MATCHLINE: &str = "Philips Matchline (90s Euro RGB)";
pub const HALO_PRESET_COMMODORE_1084S: &str = "Commodore 1084S / NEC (Late 80s)";
pub const HALO_PRESET_MITSUBISHI_DIAMONDTRON: &str = "Mitsubishi Diamondtron VGA (00s)";
pub const HALO_PRESET_BUDGET_13_RF: &str = "Budget 13\" Dorm TV (Early 90s RF)";
pub const HALO_DEFAULT_PRESET: &str = HALO_PRESET_SONY_WEGA;
pub const BEAM_DEFAULT_PRESET: &str = HALO_PRESET_SONY_WEGA;

fn default_halo_vibrance() -> f32 {
    1.0
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
    #[serde(default = "default_halo_vibrance")]
    pub vibrance: f32,
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
            vibrance: 1.0,
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

impl HaloSettings {
    /// Early-2000s flat-glass Sony FD Trinitron WEGA consumer TV (`KV-FS` / `KV-FV` series):
    /// uninterrupted vertical Aperture Grille (`slotmask = 0`), dynamic scanline blooming on
    /// bright highlights (`beam_max = 1.10`, `beam_size = 1.40`), flat glass (`curvature = false`,
    /// `corner_size = 0.005`), and Sony's warm phosphor saturation (`vibrance = 1.10`).
    pub fn preset_sony_trinitron_wega() -> Self {
        Self {
            brightboost: 1.20,
            brightboost1: 1.40,
            beam_min: 1.70,
            beam_max: 1.10,
            beam_size: 1.40,
            h_sharp: 3.50,
            vibrance: 1.10,
            glow: 0.35,
            bloom: 0.50,
            halation: 0.16,
            shadow_mask: 6.0,
            masksize: 1.0,
            maskstr: 0.40,
            mcut: 0.70,
            slotmask: 0.0,
            slotmask1: 0.0,
            double_slot: 3.0,
            smoothmask: 1.00,
            halo_zoom: 100.0,
            halo_intensity: 0.75,
            corner_size: 0.005,
            curvature: false,
        }
    }

    /// 1990s curved dark-charcoal Sony Trinitron consumer TV (`KV-S` / `KV-V` series):
    /// uninterrupted vertical Aperture Grille (`slotmask = 0`), curved cylindrical tube
    /// (`curvature = true`, `corner_size = 0.025`), dynamic scanline bloom, and vivid phosphor saturation.
    pub fn preset_sony_trinitron_90s() -> Self {
        Self {
            brightboost: 1.20,
            brightboost1: 1.40,
            beam_min: 1.70,
            beam_max: 1.15,
            beam_size: 1.35,
            h_sharp: 3.50,
            vibrance: 1.10,
            glow: 0.38,
            bloom: 0.52,
            halation: 0.18,
            shadow_mask: 6.0,
            masksize: 1.0,
            maskstr: 0.42,
            mcut: 0.72,
            slotmask: 0.0,
            slotmask1: 0.0,
            double_slot: 3.0,
            smoothmask: 0.95,
            halo_zoom: 100.0,
            halo_intensity: 0.85,
            corner_size: 0.025,
            curvature: true,
        }
    }

    /// Mid-1990s Sony PVM-20M2MDU / PVM-20L2 600-TVL professional broadcast/medical monitor:
    /// high-density HR Trinitron Aperture Grille, wide RGB video bandwidth (`h_sharp = 6.50`),
    /// regulated high-voltage supply (tight beam with crisp dark scanlines), low halation,
    /// and calibrated SMPTE-C phosphors.
    pub fn preset_sony_pvm_20m2() -> Self {
        Self {
            brightboost: 1.18,
            brightboost1: 1.32,
            beam_min: 2.05,
            beam_max: 1.25,
            beam_size: 1.15,
            h_sharp: 6.50,
            vibrance: 1.05,
            glow: 0.22,
            bloom: 0.35,
            halation: 0.08,
            shadow_mask: 6.0,
            masksize: 1.0,
            maskstr: 0.35,
            mcut: 0.65,
            slotmask: 0.0,
            slotmask1: 0.0,
            double_slot: 3.0,
            smoothmask: 0.75,
            halo_zoom: 100.0,
            halo_intensity: 0.55,
            corner_size: 0.018,
            curvature: true,
        }
    }

    /// Late-1990s / Early-2000s JVC D-Series (`AV-27D201` / `AV-32D500`):
    /// dark-tinted curved Invar Slot Mask tube, dynamic focus, strong horizontal bandwidth,
    /// punchy contrast, and lush organic scanlines that rivaled Sony Trinitron in the consumer space.
    pub fn preset_jvc_d_series() -> Self {
        Self {
            brightboost: 1.32,
            brightboost1: 1.50,
            beam_min: 1.75,
            beam_max: 1.15,
            beam_size: 1.35,
            h_sharp: 3.80,
            vibrance: 1.12,
            glow: 0.36,
            bloom: 0.48,
            halation: 0.16,
            shadow_mask: 6.0,
            masksize: 1.0,
            maskstr: 0.38,
            mcut: 0.68,
            slotmask: 0.22,
            slotmask1: 0.32,
            double_slot: 2.0,
            smoothmask: 0.90,
            halo_zoom: 100.0,
            halo_intensity: 0.80,
            corner_size: 0.025,
            curvature: true,
        }
    }

    /// Early-2000s Toshiba AF Series (`14AF43` / `20AF41` / `27AF42` FST Pure):
    /// flat-glass Invar Slot Mask tube with component input and Toshiba's aggressive horizontal
    /// edge peaking (`h_sharp = 4.20`), crisp staggered slot mask, and tight scanlines.
    pub fn preset_toshiba_af() -> Self {
        Self {
            brightboost: 1.32,
            brightboost1: 1.48,
            beam_min: 1.75,
            beam_max: 1.15,
            beam_size: 1.30,
            h_sharp: 4.20,
            vibrance: 1.08,
            glow: 0.32,
            bloom: 0.45,
            halation: 0.14,
            shadow_mask: 6.0,
            masksize: 1.0,
            maskstr: 0.40,
            mcut: 0.70,
            slotmask: 0.24,
            slotmask1: 0.35,
            double_slot: 2.0,
            smoothmask: 0.95,
            halo_zoom: 100.0,
            halo_intensity: 0.70,
            corner_size: 0.006,
            curvature: false,
        }
    }

    /// Late-1980s Toshiba Blackstripe II (`CF2005` / `CX2088`):
    /// spherical bubble-curved vertical-stripe slot mask with black matrix guard bands,
    /// warm 1980s RF/composite video bandwidth (`h_sharp = 2.10`), pronounced corner rounding,
    /// and generous phosphor bloom/halation.
    pub fn preset_toshiba_blackstripe_80s() -> Self {
        Self {
            brightboost: 1.35,
            brightboost1: 1.55,
            beam_min: 1.50,
            beam_max: 1.00,
            beam_size: 1.50,
            h_sharp: 2.10,
            vibrance: 1.05,
            glow: 0.46,
            bloom: 0.58,
            halation: 0.24,
            shadow_mask: 6.0,
            masksize: 1.0,
            maskstr: 0.45,
            mcut: 0.75,
            slotmask: 0.28,
            slotmask1: 0.40,
            double_slot: 3.0,
            smoothmask: 0.85,
            halo_zoom: 100.0,
            halo_intensity: 0.95,
            corner_size: 0.038,
            curvature: true,
        }
    }

    /// Early-to-Mid 1990s Panasonic "Gaoo" / SuperFlat (`CT-27SF` / Matsushita `TH-29GF`):
    /// ultra-dark tinted PanaBlack curved Invar Slot Mask tube prized for deep black levels,
    /// rich saturated color (`vibrance = 1.15`), and creamy, natural scanlines.
    pub fn preset_panasonic_gaoo_90s() -> Self {
        Self {
            brightboost: 1.30,
            brightboost1: 1.52,
            beam_min: 1.70,
            beam_max: 1.10,
            beam_size: 1.40,
            h_sharp: 3.20,
            vibrance: 1.15,
            glow: 0.38,
            bloom: 0.52,
            halation: 0.17,
            shadow_mask: 6.0,
            masksize: 1.0,
            maskstr: 0.40,
            mcut: 0.72,
            slotmask: 0.20,
            slotmask1: 0.30,
            double_slot: 2.0,
            smoothmask: 0.95,
            halo_zoom: 100.0,
            halo_intensity: 0.80,
            corner_size: 0.022,
            curvature: true,
        }
    }

    /// Early-2000s Panasonic Tau PureFlat (`CT-27SX10` / `CT-20SL14`):
    /// flat dark-tinted fine-pitch Invar Slot Mask tube with anti-reflective coating,
    /// low halation, clean 3-line digital comb filtering, and balanced modern color.
    pub fn preset_panasonic_tau_00s() -> Self {
        Self {
            brightboost: 1.28,
            brightboost1: 1.45,
            beam_min: 1.68,
            beam_max: 1.12,
            beam_size: 1.35,
            h_sharp: 3.70,
            vibrance: 1.08,
            glow: 0.30,
            bloom: 0.44,
            halation: 0.12,
            shadow_mask: 6.0,
            masksize: 1.0,
            maskstr: 0.36,
            mcut: 0.68,
            slotmask: 0.18,
            slotmask1: 0.28,
            double_slot: 2.0,
            smoothmask: 1.00,
            halo_zoom: 100.0,
            halo_intensity: 0.68,
            corner_size: 0.005,
            curvature: false,
        }
    }

    /// Mid-to-Late 1990s Philips Matchline (`25PT` / `29PT` Black Line S, European RGB SCART):
    /// European Black Line S Invar Slot Mask tube with warm D65 color balance, smooth organic
    /// horizontal transitions without artificial ringing, and classic curved pillow geometry.
    pub fn preset_philips_matchline_90s() -> Self {
        Self {
            brightboost: 1.30,
            brightboost1: 1.48,
            beam_min: 1.62,
            beam_max: 1.12,
            beam_size: 1.38,
            h_sharp: 2.90,
            vibrance: 1.06,
            glow: 0.40,
            bloom: 0.50,
            halation: 0.18,
            shadow_mask: 6.0,
            masksize: 1.0,
            maskstr: 0.38,
            mcut: 0.70,
            slotmask: 0.22,
            slotmask1: 0.34,
            double_slot: 2.0,
            smoothmask: 0.95,
            halo_zoom: 100.0,
            halo_intensity: 0.82,
            corner_size: 0.028,
            curvature: true,
        }
    }

    /// Late-1980s Commodore 1084S / Philips CM8833 / NEC PC-KD854 14-inch 15kHz RGB monitor:
    /// medium-pitch staggered dot-triad shadow mask (`shadow_mask = 1.0`), direct analog RGB
    /// horizontal clarity (`h_sharp = 4.50`), tight beam for 80-column text and pixel art,
    /// and curved 14" monitor corners.
    pub fn preset_commodore_1084s_80s() -> Self {
        Self {
            brightboost: 1.25,
            brightboost1: 1.40,
            beam_min: 1.85,
            beam_max: 1.25,
            beam_size: 1.20,
            h_sharp: 4.50,
            vibrance: 1.05,
            glow: 0.28,
            bloom: 0.38,
            halation: 0.10,
            shadow_mask: 1.0,
            masksize: 1.0,
            maskstr: 0.36,
            mcut: 0.65,
            slotmask: 0.0,
            slotmask1: 0.0,
            double_slot: 3.0,
            smoothmask: 0.85,
            halo_zoom: 100.0,
            halo_intensity: 0.65,
            corner_size: 0.032,
            curvature: true,
        }
    }

    /// Early-2000s Mitsubishi Diamondtron NF / NEC MultiSync FE flat PC VGA CRT monitor:
    /// ultra-fine 0.24mm Aperture Grille (`shadow_mask = 5.0`), very high horizontal video
    /// bandwidth (`h_sharp = 7.50`), razor-sharp scanlines, minimal halation, and flat glass.
    pub fn preset_mitsubishi_diamondtron_00s() -> Self {
        Self {
            brightboost: 1.15,
            brightboost1: 1.25,
            beam_min: 2.15,
            beam_max: 1.35,
            beam_size: 1.05,
            h_sharp: 7.50,
            vibrance: 1.02,
            glow: 0.18,
            bloom: 0.28,
            halation: 0.06,
            shadow_mask: 5.0,
            masksize: 1.0,
            maskstr: 0.32,
            mcut: 0.60,
            slotmask: 0.0,
            slotmask1: 0.0,
            double_slot: 3.0,
            smoothmask: 0.70,
            halo_zoom: 100.0,
            halo_intensity: 0.45,
            corner_size: 0.004,
            curvature: false,
        }
    }

    /// Early-1990s Budget 13" Bedroom/Dorm TV (Orion / Funai / Emerson / GoldStar RF & Composite):
    /// small-neck 90-degree deflection tube with unregulated HV (heavy bright-area beam bloom),
    /// coarse slot mask, narrow composite/RF luma bandwidth (`h_sharp = 1.45`), strong bubble
    /// curvature (`corner_size = 0.045`), and heavy glass halation.
    pub fn preset_budget_13_dorm_tv() -> Self {
        Self {
            brightboost: 1.38,
            brightboost1: 1.60,
            beam_min: 1.35,
            beam_max: 0.85,
            beam_size: 1.65,
            h_sharp: 1.45,
            vibrance: 0.96,
            glow: 0.52,
            bloom: 0.65,
            halation: 0.28,
            shadow_mask: 6.0,
            masksize: 1.0,
            maskstr: 0.48,
            mcut: 0.78,
            slotmask: 0.32,
            slotmask1: 0.45,
            double_slot: 3.0,
            smoothmask: 1.15,
            halo_zoom: 100.0,
            halo_intensity: 1.05,
            corner_size: 0.045,
            curvature: true,
        }
    }

    pub fn builtin_preset_list() -> Vec<(&'static str, Self)> {
        vec![
            (HALO_PRESET_SONY_WEGA, Self::preset_sony_trinitron_wega()),
            (HALO_PRESET_SONY_90S, Self::preset_sony_trinitron_90s()),
            (HALO_PRESET_SONY_PVM_20M2, Self::preset_sony_pvm_20m2()),
            (HALO_PRESET_JVC_D_SERIES, Self::preset_jvc_d_series()),
            (HALO_PRESET_TOSHIBA_AF, Self::preset_toshiba_af()),
            (
                HALO_PRESET_TOSHIBA_BLACKSTRIPE,
                Self::preset_toshiba_blackstripe_80s(),
            ),
            (
                HALO_PRESET_PANASONIC_GAOO,
                Self::preset_panasonic_gaoo_90s(),
            ),
            (HALO_PRESET_PANASONIC_TAU, Self::preset_panasonic_tau_00s()),
            (
                HALO_PRESET_PHILIPS_MATCHLINE,
                Self::preset_philips_matchline_90s(),
            ),
            (
                HALO_PRESET_COMMODORE_1084S,
                Self::preset_commodore_1084s_80s(),
            ),
            (
                HALO_PRESET_MITSUBISHI_DIAMONDTRON,
                Self::preset_mitsubishi_diamondtron_00s(),
            ),
            (HALO_PRESET_BUDGET_13_RF, Self::preset_budget_13_dorm_tv()),
        ]
    }

    pub fn builtin_presets() -> BTreeMap<String, Self> {
        let mut map = BTreeMap::new();
        for (name, preset) in Self::builtin_preset_list() {
            map.insert(name.to_string(), preset);
        }
        map
    }

    pub fn builtin_preset(name: &str) -> Option<Self> {
        Self::builtin_preset_list()
            .into_iter()
            .find(|(n, _)| *n == name)
            .map(|(_, p)| p)
    }

    pub fn is_builtin_preset(name: &str) -> bool {
        Self::builtin_preset_list().iter().any(|(n, _)| *n == name)
    }

    pub fn sanitized(&self) -> Self {
        let def = Self::default();
        let clean = |v: f32, fallback: f32, min: f32, max: f32| -> f32 {
            if v.is_finite() {
                v.clamp(min, max)
            } else {
                fallback.clamp(min, max)
            }
        };
        Self {
            brightboost: clean(self.brightboost, def.brightboost, 0.5, 3.0),
            brightboost1: clean(self.brightboost1, def.brightboost1, 0.5, 3.0),
            beam_min: clean(self.beam_min, def.beam_min, 0.5, 3.0),
            beam_max: clean(self.beam_max, def.beam_max, 0.2, 2.0),
            beam_size: clean(self.beam_size, def.beam_size, 0.0, 2.0),
            h_sharp: clean(self.h_sharp, def.h_sharp, 1.0, 10.0),
            vibrance: clean(self.vibrance, def.vibrance, 0.0, 3.0),
            glow: clean(self.glow, def.glow, 0.0, 1.0),
            bloom: clean(self.bloom, def.bloom, 0.0, 1.0),
            halation: clean(self.halation, def.halation, 0.0, 0.5),
            shadow_mask: clean(self.shadow_mask, def.shadow_mask, 0.0, 6.0),
            masksize: clean(self.masksize, def.masksize, 1.0, 4.0),
            maskstr: clean(self.maskstr, def.maskstr, 0.0, 1.0),
            mcut: clean(self.mcut, def.mcut, 0.0, 1.0),
            slotmask: clean(self.slotmask, def.slotmask, 0.0, 1.0),
            slotmask1: clean(self.slotmask1, def.slotmask1, 0.0, 1.0),
            double_slot: clean(self.double_slot, def.double_slot, 1.0, 4.0),
            smoothmask: clean(self.smoothmask, def.smoothmask, 0.0, 2.0),
            halo_zoom: clean(self.halo_zoom, def.halo_zoom, 50.0, 100.0),
            halo_intensity: clean(self.halo_intensity, def.halo_intensity, 0.0, 2.0),
            corner_size: clean(self.corner_size, def.corner_size, 0.0, 0.1),
            curvature: self.curvature,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BeamMaskPreset {
    pub curvature: bool,
    pub warp_x: f32,
    pub warp_y: f32,
    pub corner_size: f32,
    pub screen_scale: f32,
    pub scanline_strength: f32,
    pub beam_min: f32,
    pub beam_max: f32,
    pub h_sharp: f32,
    pub brightboost: f32,
    pub shadow_mask: f32,
    pub mask_strength: f32,
    pub mask_size: f32,
    pub mask_cutoff: f32,
    pub slot_mask: f32,
    pub double_slot: f32,
    pub smooth_mask: f32,
    pub halation: f32,
}

impl Default for BeamMaskPreset {
    fn default() -> Self {
        Self::preset_sony_trinitron_wega()
    }
}

impl BeamMaskPreset {
    /// Early-2000s flat-glass Sony FD Trinitron WEGA consumer TV (`KV-FS` / `KV-FV` series):
    /// uninterrupted vertical Aperture Grille (`shadow_mask = 6.0`, `slot_mask = 0.0`),
    /// dynamic scanline blooming on bright highlights, flat glass (`curvature = false`),
    /// and balanced horizontal video bandwidth (`h_sharp = 3.50`).
    pub fn preset_sony_trinitron_wega() -> Self {
        Self {
            curvature: false,
            warp_x: 0.012,
            warp_y: 0.018,
            corner_size: 0.005,
            screen_scale: 100.0,
            scanline_strength: 0.45,
            beam_min: 1.65,
            beam_max: 1.10,
            h_sharp: 3.50,
            brightboost: 1.20,
            shadow_mask: 6.0,
            mask_strength: 0.35,
            mask_size: 1.0,
            mask_cutoff: 0.72,
            slot_mask: 0.0,
            double_slot: 3.0,
            smooth_mask: 0.65,
            halation: 0.12,
        }
    }

    /// 1990s curved dark-charcoal Sony Trinitron consumer TV (`KV-S` / `KV-V` series):
    /// uninterrupted vertical Aperture Grille (`shadow_mask = 6.0`, `slot_mask = 0.0`),
    /// cylindrical vertical-only tube curvature (`warp_x = 0.006`, `warp_y = 0.036`),
    /// dynamic scanline bloom, and organic glass halation.
    pub fn preset_sony_trinitron_90s() -> Self {
        Self {
            curvature: true,
            warp_x: 0.006,
            warp_y: 0.036,
            corner_size: 0.022,
            screen_scale: 100.0,
            scanline_strength: 0.48,
            beam_min: 1.70,
            beam_max: 1.12,
            h_sharp: 3.50,
            brightboost: 1.22,
            shadow_mask: 6.0,
            mask_strength: 0.38,
            mask_size: 1.0,
            mask_cutoff: 0.72,
            slot_mask: 0.0,
            double_slot: 3.0,
            smooth_mask: 0.65,
            halation: 0.14,
        }
    }

    /// Mid-1990s Sony PVM-20M2MDU / PVM-20L2 600-TVL professional broadcast monitor:
    /// high-density HR Trinitron Aperture Grille, wide RGB video bandwidth (`h_sharp = 6.50`),
    /// regulated high-voltage supply (tight beam with crisp dark scanlines), and low halation.
    pub fn preset_sony_pvm_20m2() -> Self {
        Self {
            curvature: true,
            warp_x: 0.005,
            warp_y: 0.028,
            corner_size: 0.018,
            screen_scale: 100.0,
            scanline_strength: 0.62,
            beam_min: 2.05,
            beam_max: 1.25,
            h_sharp: 6.50,
            brightboost: 1.18,
            shadow_mask: 6.0,
            mask_strength: 0.32,
            mask_size: 1.0,
            mask_cutoff: 0.68,
            slot_mask: 0.0,
            double_slot: 3.0,
            smooth_mask: 0.50,
            halation: 0.06,
        }
    }

    /// Late-1990s / Early-2000s JVC D-Series (`AV-27D201` / `AV-32D500`):
    /// dark-tinted curved Invar Slot Mask tube (`slot_mask = 0.24`), dynamic focus,
    /// strong horizontal bandwidth (`h_sharp = 3.80`), and punchy organic scanlines.
    pub fn preset_jvc_d_series() -> Self {
        Self {
            curvature: true,
            warp_x: 0.028,
            warp_y: 0.038,
            corner_size: 0.025,
            screen_scale: 100.0,
            scanline_strength: 0.46,
            beam_min: 1.75,
            beam_max: 1.15,
            h_sharp: 3.80,
            brightboost: 1.28,
            shadow_mask: 6.0,
            mask_strength: 0.36,
            mask_size: 1.0,
            mask_cutoff: 0.70,
            slot_mask: 0.24,
            double_slot: 2.0,
            smooth_mask: 0.65,
            halation: 0.13,
        }
    }

    /// Early-2000s Toshiba AF Series (`14AF43` / `20AF41` / `27AF42` FST Pure):
    /// flat-glass Invar Slot Mask tube with component input and Toshiba's crisp horizontal
    /// edge peaking (`h_sharp = 4.20`), staggered slot mask (`slot_mask = 0.26`), and tight scanlines.
    pub fn preset_toshiba_af() -> Self {
        Self {
            curvature: false,
            warp_x: 0.010,
            warp_y: 0.014,
            corner_size: 0.006,
            screen_scale: 100.0,
            scanline_strength: 0.46,
            beam_min: 1.75,
            beam_max: 1.15,
            h_sharp: 4.20,
            brightboost: 1.26,
            shadow_mask: 6.0,
            mask_strength: 0.38,
            mask_size: 1.0,
            mask_cutoff: 0.72,
            slot_mask: 0.26,
            double_slot: 2.0,
            smooth_mask: 0.68,
            halation: 0.11,
        }
    }

    /// Late-1980s Toshiba Blackstripe II (`CF2005` / `CX2088`):
    /// spherical bubble-curved vertical-stripe slot mask with black matrix guard bands,
    /// warm 1980s RF/composite video bandwidth (`h_sharp = 2.10`), pronounced corner rounding,
    /// and generous phosphor halation.
    pub fn preset_toshiba_blackstripe_80s() -> Self {
        Self {
            curvature: true,
            warp_x: 0.042,
            warp_y: 0.052,
            corner_size: 0.038,
            screen_scale: 100.0,
            scanline_strength: 0.38,
            beam_min: 1.50,
            beam_max: 1.00,
            h_sharp: 2.10,
            brightboost: 1.32,
            shadow_mask: 6.0,
            mask_strength: 0.42,
            mask_size: 1.0,
            mask_cutoff: 0.76,
            slot_mask: 0.30,
            double_slot: 3.0,
            smooth_mask: 0.75,
            halation: 0.20,
        }
    }

    /// Early-to-Mid 1990s Panasonic "Gaoo" / SuperFlat (`CT-27SF` / Matsushita `TH-29GF`):
    /// ultra-dark tinted PanaBlack curved Invar Slot Mask tube prized for deep black levels
    /// and creamy, natural scanlines.
    pub fn preset_panasonic_gaoo_90s() -> Self {
        Self {
            curvature: true,
            warp_x: 0.024,
            warp_y: 0.034,
            corner_size: 0.022,
            screen_scale: 100.0,
            scanline_strength: 0.44,
            beam_min: 1.70,
            beam_max: 1.10,
            h_sharp: 3.20,
            brightboost: 1.28,
            shadow_mask: 6.0,
            mask_strength: 0.38,
            mask_size: 1.0,
            mask_cutoff: 0.72,
            slot_mask: 0.22,
            double_slot: 2.0,
            smooth_mask: 0.70,
            halation: 0.14,
        }
    }

    /// Early-2000s Panasonic Tau PureFlat (`CT-27SX10` / `CT-20SL14`):
    /// flat dark-tinted fine-pitch Invar Slot Mask tube with anti-reflective coating,
    /// low halation, and clean 3-line digital comb filtering.
    pub fn preset_panasonic_tau_00s() -> Self {
        Self {
            curvature: false,
            warp_x: 0.010,
            warp_y: 0.014,
            corner_size: 0.005,
            screen_scale: 100.0,
            scanline_strength: 0.42,
            beam_min: 1.68,
            beam_max: 1.12,
            h_sharp: 3.70,
            brightboost: 1.24,
            shadow_mask: 6.0,
            mask_strength: 0.34,
            mask_size: 1.0,
            mask_cutoff: 0.68,
            slot_mask: 0.20,
            double_slot: 2.0,
            smooth_mask: 0.72,
            halation: 0.09,
        }
    }

    /// Mid-to-Late 1990s Philips Matchline (`25PT` / `29PT` Black Line S, European RGB SCART):
    /// European Black Line S Invar Slot Mask tube with smooth organic horizontal transitions
    /// without artificial ringing, and classic curved pillow geometry.
    pub fn preset_philips_matchline_90s() -> Self {
        Self {
            curvature: true,
            warp_x: 0.030,
            warp_y: 0.040,
            corner_size: 0.028,
            screen_scale: 100.0,
            scanline_strength: 0.42,
            beam_min: 1.62,
            beam_max: 1.12,
            h_sharp: 2.90,
            brightboost: 1.26,
            shadow_mask: 6.0,
            mask_strength: 0.36,
            mask_size: 1.0,
            mask_cutoff: 0.70,
            slot_mask: 0.24,
            double_slot: 2.0,
            smooth_mask: 0.70,
            halation: 0.15,
        }
    }

    /// Late-1980s Commodore 1084S / Philips CM8833 / NEC PC-KD854 14-inch 15kHz RGB monitor:
    /// medium-pitch staggered dot-triad shadow mask (`shadow_mask = 1.0`), direct analog RGB
    /// horizontal clarity (`h_sharp = 4.50`), tight beam for 80-column text and pixel art,
    /// and curved 14" monitor corners.
    pub fn preset_commodore_1084s_80s() -> Self {
        Self {
            curvature: true,
            warp_x: 0.032,
            warp_y: 0.042,
            corner_size: 0.032,
            screen_scale: 100.0,
            scanline_strength: 0.52,
            beam_min: 1.85,
            beam_max: 1.25,
            h_sharp: 4.50,
            brightboost: 1.22,
            shadow_mask: 1.0,
            mask_strength: 0.34,
            mask_size: 1.0,
            mask_cutoff: 0.65,
            slot_mask: 0.0,
            double_slot: 3.0,
            smooth_mask: 0.60,
            halation: 0.08,
        }
    }

    /// Early-2000s Mitsubishi Diamondtron NF / NEC MultiSync FE flat PC VGA CRT monitor:
    /// ultra-fine 0.24mm Aperture Grille (`shadow_mask = 5.0`), very high horizontal video
    /// bandwidth (`h_sharp = 7.50`), razor-sharp scanlines, minimal halation, and flat glass.
    pub fn preset_mitsubishi_diamondtron_00s() -> Self {
        Self {
            curvature: false,
            warp_x: 0.006,
            warp_y: 0.008,
            corner_size: 0.004,
            screen_scale: 100.0,
            scanline_strength: 0.65,
            beam_min: 2.15,
            beam_max: 1.35,
            h_sharp: 7.50,
            brightboost: 1.15,
            shadow_mask: 5.0,
            mask_strength: 0.30,
            mask_size: 1.0,
            mask_cutoff: 0.60,
            slot_mask: 0.0,
            double_slot: 3.0,
            smooth_mask: 0.45,
            halation: 0.04,
        }
    }

    /// Early-1990s Budget 13" Bedroom/Dorm TV (Orion / Funai / Emerson / GoldStar RF & Composite):
    /// small-neck 90-degree deflection tube with unregulated HV (heavy bright-area beam bloom),
    /// coarse slot mask, narrow composite/RF luma bandwidth (`h_sharp = 1.45`), strong bubble
    /// curvature (`corner_size = 0.045`), and heavy glass halation.
    pub fn preset_budget_13_dorm_tv() -> Self {
        Self {
            curvature: true,
            warp_x: 0.050,
            warp_y: 0.062,
            corner_size: 0.045,
            screen_scale: 100.0,
            scanline_strength: 0.34,
            beam_min: 1.35,
            beam_max: 0.85,
            h_sharp: 1.45,
            brightboost: 1.35,
            shadow_mask: 6.0,
            mask_strength: 0.45,
            mask_size: 1.0,
            mask_cutoff: 0.78,
            slot_mask: 0.34,
            double_slot: 3.0,
            smooth_mask: 0.85,
            halation: 0.24,
        }
    }

    pub fn builtin_preset_list() -> Vec<(&'static str, Self)> {
        vec![
            (HALO_PRESET_SONY_WEGA, Self::preset_sony_trinitron_wega()),
            (HALO_PRESET_SONY_90S, Self::preset_sony_trinitron_90s()),
            (HALO_PRESET_SONY_PVM_20M2, Self::preset_sony_pvm_20m2()),
            (HALO_PRESET_JVC_D_SERIES, Self::preset_jvc_d_series()),
            (HALO_PRESET_TOSHIBA_AF, Self::preset_toshiba_af()),
            (
                HALO_PRESET_TOSHIBA_BLACKSTRIPE,
                Self::preset_toshiba_blackstripe_80s(),
            ),
            (
                HALO_PRESET_PANASONIC_GAOO,
                Self::preset_panasonic_gaoo_90s(),
            ),
            (HALO_PRESET_PANASONIC_TAU, Self::preset_panasonic_tau_00s()),
            (
                HALO_PRESET_PHILIPS_MATCHLINE,
                Self::preset_philips_matchline_90s(),
            ),
            (
                HALO_PRESET_COMMODORE_1084S,
                Self::preset_commodore_1084s_80s(),
            ),
            (
                HALO_PRESET_MITSUBISHI_DIAMONDTRON,
                Self::preset_mitsubishi_diamondtron_00s(),
            ),
            (HALO_PRESET_BUDGET_13_RF, Self::preset_budget_13_dorm_tv()),
        ]
    }

    pub fn builtin_presets() -> BTreeMap<String, Self> {
        let mut map = BTreeMap::new();
        for (name, preset) in Self::builtin_preset_list() {
            map.insert(name.to_string(), preset);
        }
        map
    }

    pub fn builtin_preset(name: &str) -> Option<Self> {
        Self::builtin_preset_list()
            .into_iter()
            .find(|(n, _)| *n == name)
            .map(|(_, p)| p)
    }

    pub fn is_builtin_preset(name: &str) -> bool {
        Self::builtin_preset_list().iter().any(|(n, _)| *n == name)
    }

    pub fn sanitized(&self) -> Self {
        let def = Self::default();
        let clean = |v: f32, fallback: f32, min: f32, max: f32| -> f32 {
            if v.is_finite() {
                v.clamp(min, max)
            } else {
                fallback.clamp(min, max)
            }
        };
        Self {
            curvature: self.curvature,
            warp_x: clean(self.warp_x, def.warp_x, 0.0, 0.25),
            warp_y: clean(self.warp_y, def.warp_y, 0.0, 0.25),
            corner_size: clean(self.corner_size, def.corner_size, 0.0, 0.10),
            screen_scale: clean(self.screen_scale, def.screen_scale, 50.0, 100.0),
            scanline_strength: clean(self.scanline_strength, def.scanline_strength, 0.0, 1.0),
            beam_min: clean(self.beam_min, def.beam_min, 0.3, 3.0),
            beam_max: clean(self.beam_max, def.beam_max, 0.2, 3.0),
            h_sharp: clean(self.h_sharp, def.h_sharp, 1.0, 10.0),
            brightboost: clean(self.brightboost, def.brightboost, 0.5, 2.5),
            shadow_mask: clean(self.shadow_mask, def.shadow_mask, 0.0, 6.0).round(),
            mask_strength: clean(self.mask_strength, def.mask_strength, 0.0, 1.0),
            mask_size: clean(self.mask_size, def.mask_size, 1.0, 4.0).round(),
            mask_cutoff: clean(self.mask_cutoff, def.mask_cutoff, 0.0, 1.0),
            slot_mask: clean(self.slot_mask, def.slot_mask, 0.0, 1.0),
            double_slot: clean(self.double_slot, def.double_slot, 1.0, 4.0).round(),
            smooth_mask: clean(self.smooth_mask, def.smooth_mask, 0.0, 1.5),
            halation: clean(self.halation, def.halation, 0.0, 1.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BeamSettings {
    pub frames_per_hz: f32,
    pub target_hz: u32,
    pub gamma: f32,
    pub gain_vs_blur: f32,
    pub fps_divisor: f32,
    pub lcd_anti_retention: bool,
    pub lcd_inversion_compensation_slew: f32,
    pub scan_direction: u8,
    pub splitscreen: bool,
    pub splitscreen_x: f32,
    pub splitscreen_y: f32,
    pub splitscreen_border_px: u32,
    pub splitscreen_match_brightness: bool,

    // Stage 2: CRT Geometry, Scanlines & Phosphor Mask (Gamma-Curve-Preserving)
    pub curvature: bool,
    pub warp_x: f32,
    pub warp_y: f32,
    pub corner_size: f32,
    pub screen_scale: f32,
    pub scanline_strength: f32,
    pub beam_min: f32,
    pub beam_max: f32,
    pub h_sharp: f32,
    pub brightboost: f32,
    pub shadow_mask: f32,
    pub mask_strength: f32,
    pub mask_size: f32,
    pub mask_cutoff: f32,
    pub slot_mask: f32,
    pub double_slot: f32,
    pub smooth_mask: f32,
    pub halation: f32,
}

impl Default for BeamSettings {
    fn default() -> Self {
        Self {
            frames_per_hz: 2.0,
            target_hz: 120,
            gamma: 2.4,
            gain_vs_blur: 0.75,
            fps_divisor: 1.0,
            lcd_anti_retention: false,
            lcd_inversion_compensation_slew: 0.001,
            scan_direction: 1,
            splitscreen: false,
            splitscreen_x: 0.50,
            splitscreen_y: 0.00,
            splitscreen_border_px: 2,
            splitscreen_match_brightness: true,

            curvature: true,
            warp_x: 0.031,
            warp_y: 0.041,
            corner_size: 0.020,
            screen_scale: 100.0,
            scanline_strength: 0.35,
            beam_min: 1.35,
            beam_max: 1.05,
            h_sharp: 2.50,
            brightboost: 1.0,
            shadow_mask: 6.0,
            mask_strength: 0.30,
            mask_size: 1.0,
            mask_cutoff: 0.75,
            slot_mask: 0.0,
            double_slot: 2.0,
            smooth_mask: 0.50,
            halation: 0.08,
        }
    }
}

#[derive(Clone)]
pub struct BeamLockstepState {
    pub sweep_start_time: Option<Instant>,
    pub last_present_time: Option<Instant>,
    pub next_subframe_time: Option<Instant>,
    pub last_source_frame_arrival: Option<Instant>,
    pub last_live_frame_arrival: Option<Instant>,
    pub last_captured_at_us: Option<i64>,
    pub prev_candidate_sweep_dt: Option<std::time::Duration>,
    pub staged_video_frame: Option<std::sync::Arc<crate::video::types::RawFrame>>,
    pub staged_dmabuf_frame: Option<std::sync::Arc<crate::gamescope::pipewire_dma::DmaBufFrame>>,
    pub staged_just_after_sub0: bool,
    pub sweeps_since_live_arrival: u32,
    pub sweeps_per_frame: u32,
    pub smoothed_sweep_duration: std::time::Duration,
    pub current_sweep_duration: std::time::Duration,
    pub last_raw_frame_dt: std::time::Duration,
    pub frame_jitter_ema: std::time::Duration,
    pub sub0_render_dur: std::time::Duration,
    pub sub_render_ema: std::time::Duration,
    pub has_measured_interval: bool,
    pub phase_locked: bool,
    pub received_on_last_sweep: bool,
    pub subframe_index: u32,
    pub sweep_counter: u32,
    pub current_i_frame: i32,
    pub last_effective_fph: f32,
    pub last_fps_divisor: f32,
    pub last_target_hz: u32,
}

impl Default for BeamLockstepState {
    fn default() -> Self {
        let default_sweep = std::time::Duration::from_nanos(16_683_333);
        Self {
            sweep_start_time: None,
            last_present_time: None,
            next_subframe_time: None,
            last_source_frame_arrival: None,
            last_live_frame_arrival: None,
            last_captured_at_us: None,
            prev_candidate_sweep_dt: None,
            staged_video_frame: None,
            staged_dmabuf_frame: None,
            staged_just_after_sub0: false,
            sweeps_since_live_arrival: 0,
            sweeps_per_frame: 1,
            // Default to 59.94 Hz NTSC period (1001 / 60000 s = 16,683,333 ns)
            smoothed_sweep_duration: default_sweep,
            current_sweep_duration: default_sweep,
            last_raw_frame_dt: default_sweep,
            frame_jitter_ema: std::time::Duration::ZERO,
            sub0_render_dur: std::time::Duration::ZERO,
            sub_render_ema: std::time::Duration::ZERO,
            has_measured_interval: false,
            phase_locked: false,
            received_on_last_sweep: false,
            subframe_index: 0,
            sweep_counter: 0,
            current_i_frame: 0,
            last_effective_fph: 2.0,
            last_fps_divisor: 1.0,
            last_target_hz: 144,
        }
    }
}

impl BeamLockstepState {
    pub fn reset(&mut self, effective_fph: f32, fps_divisor: f32) {
        let default_sweep = std::time::Duration::from_nanos(16_683_333);
        self.sweep_start_time = None;
        self.last_present_time = None;
        self.next_subframe_time = None;
        self.last_source_frame_arrival = None;
        self.last_live_frame_arrival = None;
        self.last_captured_at_us = None;
        self.prev_candidate_sweep_dt = None;
        self.staged_video_frame = None;
        self.staged_dmabuf_frame = None;
        self.staged_just_after_sub0 = false;
        self.sweeps_since_live_arrival = 0;
        self.sweeps_per_frame = 1;
        self.smoothed_sweep_duration = default_sweep;
        self.current_sweep_duration = default_sweep;
        self.last_raw_frame_dt = default_sweep;
        self.frame_jitter_ema = std::time::Duration::ZERO;
        self.sub0_render_dur = std::time::Duration::ZERO;
        self.sub_render_ema = std::time::Duration::ZERO;
        self.has_measured_interval = false;
        self.phase_locked = false;
        self.received_on_last_sweep = false;
        self.subframe_index = 0;
        self.sweep_counter = 0;
        self.current_i_frame = 0;
        self.last_effective_fph = effective_fph;
        self.last_fps_divisor = fps_divisor;
    }

    /// Clears transient sweep and arrival timestamps when the Beam CRT shader is deactivated
    /// (e.g. toggled off with `C` or switched to another filter) so re-enabling Beam starts a
    /// clean sweep without stale timestamps.
    pub fn deactivate(&mut self) {
        if self.sweep_start_time.is_some() || self.next_subframe_time.is_some() {
            let n_subframes = (self.last_effective_fph.round() as u32).max(1);
            self.sweep_counter = (self.sweep_counter + 1) % (8_000_000 / n_subframes);
        }
        self.sweep_start_time = None;
        self.last_present_time = None;
        self.next_subframe_time = None;
        self.last_source_frame_arrival = None;
        self.last_live_frame_arrival = None;
        self.last_captured_at_us = None;
        self.prev_candidate_sweep_dt = None;
        self.staged_just_after_sub0 = false;
        self.sweeps_since_live_arrival = 0;
        self.phase_locked = false;
        self.received_on_last_sweep = false;
        self.subframe_index = 0;
        self.sub0_render_dur = std::time::Duration::ZERO;
        self.sub_render_ema = std::time::Duration::ZERO;
    }

    /// Minimum CRT sweep duration so that `N` sub-frames per sweep never exceed the display's
    /// VRR ceiling (at least 144Hz or `N * 65Hz` so 60fps streams are never throttled by a lower
    /// manual `target_hz` setting).
    pub fn min_sweep_duration(&self) -> std::time::Duration {
        let n_subframes = (self.last_effective_fph.round() as u64).max(1);
        let min_lockstep_hz = (n_subframes * 65).max(144);
        let target_hz = (self.last_target_hz as u64).max(min_lockstep_hz);
        // Include a 120us safety margin below the physical display refresh ceiling so VRR swaps
        // never collide with the monitor's maximum scanout frequency.
        let min_vrr_subframe_ns = (1_000_000_000 / target_hz) + 120_000;
        std::time::Duration::from_nanos(min_vrr_subframe_ns * n_subframes)
    }

    pub fn max_sweep_duration(&self) -> std::time::Duration {
        std::time::Duration::from_micros(28_500)
    }

    fn choose_sweeps_per_frame(&self, raw_dt: std::time::Duration) -> u32 {
        let min_sweep = self.min_sweep_duration();
        let current_k = self.sweeps_per_frame.clamp(1, 4);
        for k in [4u32, 3, 2] {
            let threshold = if current_k >= k {
                // Hysteresis to stay in k-sweep LFC mode
                (min_sweep * k)
                    .checked_sub(std::time::Duration::from_micros(400))
                    .unwrap_or(min_sweep * k)
            } else {
                // Hysteresis to enter k-sweep LFC mode
                min_sweep * k + std::time::Duration::from_micros(900)
            };
            if raw_dt >= threshold {
                return k;
            }
        }
        1
    }

    /// Records a newly arrived source frame using its producer `captured_at` timestamp (microseconds
    /// from `CLOCK_MONOTONIC`), the host `arrival` `Instant` when it was popped from the channel,
    /// and whether it arrived in the narrow window right after Sub-frame 0 (`just_after_sub0`).
    pub fn record_source_frame(
        &mut self,
        captured_at_us: i64,
        arrival: Instant,
        just_after_sub0: bool,
    ) {
        let mut measured_raw_dt = None;

        if captured_at_us > 0 {
            if let Some(prev_us) = self.last_captured_at_us {
                let diff_us = captured_at_us.saturating_sub(prev_us);
                if (4_000..=200_000).contains(&diff_us) {
                    measured_raw_dt = Some(std::time::Duration::from_micros(diff_us as u64));
                }
            }
            self.last_captured_at_us = Some(captured_at_us);
        }

        if measured_raw_dt.is_none() {
            if let Some(prev_arrival) = self.last_source_frame_arrival {
                let dt = arrival.saturating_duration_since(prev_arrival);
                if dt >= std::time::Duration::from_micros(4_000)
                    && dt <= std::time::Duration::from_micros(200_000)
                {
                    let sweeps = self.sweeps_since_live_arrival.max(1);
                    if captured_at_us <= 0 && sweeps > 1 && sweeps <= 4 {
                        let per_sweep = dt / sweeps;
                        if per_sweep >= self.min_sweep_duration()
                            && per_sweep <= std::time::Duration::from_micros(21_000)
                        {
                            measured_raw_dt = Some(per_sweep);
                        } else {
                            measured_raw_dt = Some(dt);
                        }
                    } else {
                        measured_raw_dt = Some(dt);
                    }
                }
            }
        }

        self.last_source_frame_arrival = Some(arrival);
        self.last_live_frame_arrival = Some(arrival);
        self.staged_just_after_sub0 = just_after_sub0;

        if let Some(raw_dt) = measured_raw_dt {
            self.last_raw_frame_dt = raw_dt;
            let min_sweep = self.min_sweep_duration();
            let max_sweep = self.max_sweep_duration();

            let new_k = self.choose_sweeps_per_frame(raw_dt);
            let k_changed = new_k != self.sweeps_per_frame;
            self.sweeps_per_frame = new_k;
            let candidate_sweep_dt =
                (raw_dt / self.sweeps_per_frame.max(1)).clamp(min_sweep, max_sweep);

            if !self.has_measured_interval {
                self.smoothed_sweep_duration = candidate_sweep_dt;
                self.current_sweep_duration = candidate_sweep_dt;
                self.prev_candidate_sweep_dt = Some(candidate_sweep_dt);
                self.frame_jitter_ema = std::time::Duration::ZERO;
                self.has_measured_interval = true;
            } else if k_changed {
                self.smoothed_sweep_duration = candidate_sweep_dt;
                self.prev_candidate_sweep_dt = Some(candidate_sweep_dt);
            } else {
                let old_ns = self.smoothed_sweep_duration.as_nanos() as f64;
                let new_ns = candidate_sweep_dt.as_nanos() as f64;
                let diff_ns = (new_ns - old_ns).abs();
                let jitter_ns = self.frame_jitter_ema.as_nanos() as f64 * 0.70 + diff_ns * 0.30;
                self.frame_jitter_ema = std::time::Duration::from_nanos(jitter_ns.round() as u64)
                    .min(std::time::Duration::from_millis(6));

                let prev_cand_ns = self
                    .prev_candidate_sweep_dt
                    .map(|d| d.as_nanos() as f64)
                    .unwrap_or(old_ns);
                self.prev_candidate_sweep_dt = Some(candidate_sweep_dt);

                let delta_prev = prev_cand_ns - old_ns;
                let delta_curr = new_ns - old_ns;

                let (target_ns, alpha) =
                    if delta_prev * delta_curr < 0.0 && delta_curr.abs() > 150_000.0 {
                        // Opposite-sign bounce (e.g. a delayed frame `T + d` followed by an
                        // on-schedule catch-up frame `T - d`): blend toward the 2-frame midpoint
                        // `(prev + curr) / 2` so transit/scheduling jitter cancels out to zero!
                        (0.5 * (prev_cand_ns + new_ns), 0.65)
                    } else if delta_prev * delta_curr > 0.0
                        && delta_prev.abs() > 200_000.0
                        && delta_curr.abs() > 200_000.0
                    {
                        // 2+ consecutive frames shifted in the same direction: genuine framerate
                        // change (e.g. 60 -> 48 FPS or 48 -> 40 FPS), lock on rapidly.
                        (new_ns, 0.65)
                    } else {
                        // Isolated 1-frame deviation or sub-200us steady-state ripple: filter gently.
                        (new_ns, 0.25)
                    };

                let blended = (old_ns * (1.0 - alpha) + target_ns * alpha).round() as u64;
                self.smoothed_sweep_duration =
                    std::time::Duration::from_nanos(blended).clamp(min_sweep, max_sweep);
            }
        }

        self.sweeps_since_live_arrival = 0;
        self.phase_locked = true;
    }

    /// Commits the exact duration for the CRT sweep starting at `sweep_start`.
    pub fn commit_next_sweep_duration(
        &mut self,
        _sweep_start: Instant,
        got_frame_this_sweep: bool,
        stream_active: bool,
    ) -> std::time::Duration {
        let min_sweep = self.min_sweep_duration();
        let max_sweep = self.max_sweep_duration();

        let base_s = self.smoothed_sweep_duration.as_secs_f64();

        // Only apply a small phase nudge (+0.25ms) if a frame arrived in the narrow window
        // immediately after Sub-frame 0 started (`staged_just_after_sub0`). For the remaining
        // ~92% of the sweep cycle, `trim_s = 0.0` so the UI maintains an exact 1:1 frequency lock.
        let trim_s = if stream_active && got_frame_this_sweep && self.staged_just_after_sub0 {
            0.000_25
        } else {
            0.0
        };
        self.staged_just_after_sub0 = false;

        let target_sweep_ns = ((base_s + trim_s) * 1e9).round();

        // Limit sweep-to-sweep duration step so adjacent sweeps transition smoothly across
        // framerate drops without a sudden cross-sweep brightness step.
        let prev_ns = self.current_sweep_duration.as_nanos() as f64;
        let max_step_ns = 1_600_000.0;
        let slewed_ns = if self.sweep_counter == 0 {
            target_sweep_ns
        } else {
            target_sweep_ns.clamp(prev_ns - max_step_ns, prev_ns + max_step_ns)
        };

        let committed =
            std::time::Duration::from_nanos(slewed_ns.max(1.0) as u64).clamp(min_sweep, max_sweep);
        self.current_sweep_duration = committed;
        committed
    }
}

impl BeamSettings {
    pub fn sanitized(&self) -> Self {
        let def = Self::default();
        let clean = |v: f32, fallback: f32, min: f32, max: f32| -> f32 {
            if v.is_finite() {
                v.clamp(min, max)
            } else {
                fallback.clamp(min, max)
            }
        };
        Self {
            frames_per_hz: clean(self.frames_per_hz, def.frames_per_hz, 1.0, 16.0),
            target_hz: self.target_hz.clamp(60, 480),
            gamma: clean(self.gamma, def.gamma, 1.0, 5.0),
            gain_vs_blur: clean(self.gain_vs_blur, def.gain_vs_blur, 0.01, 1.0),
            fps_divisor: clean(self.fps_divisor, def.fps_divisor, 0.05, 1.0),
            lcd_anti_retention: self.lcd_anti_retention,
            lcd_inversion_compensation_slew: clean(
                self.lcd_inversion_compensation_slew,
                def.lcd_inversion_compensation_slew,
                0.0,
                0.05,
            ),
            scan_direction: self.scan_direction.min(4),
            splitscreen: self.splitscreen,
            splitscreen_x: clean(self.splitscreen_x, def.splitscreen_x, 0.0, 1.0),
            splitscreen_y: clean(self.splitscreen_y, def.splitscreen_y, 0.0, 1.0),
            splitscreen_border_px: self.splitscreen_border_px.min(32),
            splitscreen_match_brightness: self.splitscreen_match_brightness,
            curvature: self.curvature,
            warp_x: clean(self.warp_x, def.warp_x, 0.0, 0.25),
            warp_y: clean(self.warp_y, def.warp_y, 0.0, 0.25),
            corner_size: clean(self.corner_size, def.corner_size, 0.0, 0.10),
            screen_scale: clean(self.screen_scale, def.screen_scale, 50.0, 100.0),
            scanline_strength: clean(self.scanline_strength, def.scanline_strength, 0.0, 1.0),
            beam_min: clean(self.beam_min, def.beam_min, 0.3, 3.0),
            beam_max: clean(self.beam_max, def.beam_max, 0.2, 3.0),
            h_sharp: clean(self.h_sharp, def.h_sharp, 1.0, 10.0),
            brightboost: clean(self.brightboost, def.brightboost, 0.5, 2.5),
            shadow_mask: clean(self.shadow_mask, def.shadow_mask, 0.0, 6.0).round(),
            mask_strength: clean(self.mask_strength, def.mask_strength, 0.0, 1.0),
            mask_size: clean(self.mask_size, def.mask_size, 1.0, 4.0).round(),
            mask_cutoff: clean(self.mask_cutoff, def.mask_cutoff, 0.0, 1.0),
            slot_mask: clean(self.slot_mask, def.slot_mask, 0.0, 1.0),
            double_slot: clean(self.double_slot, def.double_slot, 1.0, 4.0).round(),
            smooth_mask: clean(self.smooth_mask, def.smooth_mask, 0.0, 1.5),
            halation: clean(self.halation, def.halation, 0.0, 1.0),
        }
    }

    pub fn effective_target_hz(&self) -> u32 {
        self.target_hz.clamp(60, 480)
    }

    pub fn mask_preset(&self) -> BeamMaskPreset {
        BeamMaskPreset {
            curvature: self.curvature,
            warp_x: self.warp_x,
            warp_y: self.warp_y,
            corner_size: self.corner_size,
            screen_scale: self.screen_scale,
            scanline_strength: self.scanline_strength,
            beam_min: self.beam_min,
            beam_max: self.beam_max,
            h_sharp: self.h_sharp,
            brightboost: self.brightboost,
            shadow_mask: self.shadow_mask,
            mask_strength: self.mask_strength,
            mask_size: self.mask_size,
            mask_cutoff: self.mask_cutoff,
            slot_mask: self.slot_mask,
            double_slot: self.double_slot,
            smooth_mask: self.smooth_mask,
            halation: self.halation,
        }
    }

    /// Applies only the Stage 2 CRT Tube / Scanline / Phosphor Mask preset parameters,
    /// leaving all Stage 1 Blur Busters CRT Beam Simulator parameters untouched.
    pub fn apply_mask_preset(&mut self, preset: &BeamMaskPreset) {
        let clean = preset.sanitized();
        self.curvature = clean.curvature;
        self.warp_x = clean.warp_x;
        self.warp_y = clean.warp_y;
        self.corner_size = clean.corner_size;
        self.screen_scale = clean.screen_scale;
        self.scanline_strength = clean.scanline_strength;
        self.beam_min = clean.beam_min;
        self.beam_max = clean.beam_max;
        self.h_sharp = clean.h_sharp;
        self.brightboost = clean.brightboost;
        self.shadow_mask = clean.shadow_mask;
        self.mask_strength = clean.mask_strength;
        self.mask_size = clean.mask_size;
        self.mask_cutoff = clean.mask_cutoff;
        self.slot_mask = clean.slot_mask;
        self.double_slot = clean.double_slot;
        self.smooth_mask = clean.smooth_mask;
        self.halation = clean.halation;
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
    pub fn has_active_energy(&self, threshold: f32) -> bool {
        self.low_env > threshold
            || self.mid_env > threshold
            || self.high_env > threshold
            || self.transient_env > threshold
            || self.slow_energy > threshold
            || self.spring_pos_x.abs() > threshold
            || self.spring_pos_y.abs() > threshold
            || self.spring_vel_x.abs() > threshold
            || self.spring_vel_y.abs() > threshold
    }

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
    pub fsr1_sharpness: f32,
    pub median_filter_enabled: bool,
    pub median_mix: f32,
    pub deinterlace_filter_enabled: bool,
    pub deinterlace_mode: u8,
    pub deinterlace_blend: f32,
    pub deinterlace_motion_threshold: f32,
    pub deinterlace_line_spacing: f32,
    pub deinterlace_spatial_mix: f32,
    pub vibrance: f32,
    pub brightness: f32,
    pub gamma: f32,
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
    pub new_vc_preset_name: String,
    pub vc_telemetry: Arc<crate::devices::voice_changer::VoiceChangerTelemetry>,
    pub vc_sync_in_progress: bool,
    pub vc_sync_target: Option<crate::devices::voice_changer::model_sync::VcSyncTarget>,
    pub vc_sync_progress: Option<(String, f32)>,
    pub vc_sync_error: Option<String>,
    pub vc_sync_rx:
        Option<crossbeam_channel::Receiver<crate::devices::voice_changer::model_sync::VcSyncEvent>>,
}

impl Default for MicState {
    fn default() -> Self {
        Self {
            config: crate::config::MicConfig::default(),
            active_stream: None,
            current_level_db: Arc::new(AtomicU32::new((-100.0f32).to_bits())),
            peak_level_db: Arc::new(AtomicU32::new((-100.0f32).to_bits())),
            is_clipping: Arc::new(AtomicBool::new(false)),
            new_vc_preset_name: String::new(),
            vc_telemetry: Arc::new(crate::devices::voice_changer::VoiceChangerTelemetry::new()),
            vc_sync_in_progress: false,
            vc_sync_target: None,
            vc_sync_progress: None,
            vc_sync_error: None,
            vc_sync_rx: None,
        }
    }
}

impl MicState {
    pub fn start_vc_download(
        &mut self,
        target: crate::devices::voice_changer::model_sync::VcSyncTarget,
        repaint_ctx: Option<egui::Context>,
    ) {
        if self.vc_sync_in_progress {
            return;
        }
        self.vc_sync_in_progress = true;
        self.vc_sync_target = Some(target);
        self.vc_sync_error = None;
        self.vc_sync_progress = Some(("Connecting to model repository...".to_string(), 0.0));

        let (tx, rx) = crossbeam_channel::unbounded();
        self.vc_sync_rx = Some(rx);
        let dir = crate::devices::voice_changer::model_sync::resolve_voice_changer_directory();

        let _ = std::thread::Builder::new()
            .name("vc-model-download".to_string())
            .spawn(move || {
                use crate::devices::voice_changer::model_sync::{
                    download_and_install_base_engine, download_and_install_voice_model,
                    VcSyncEvent, VcSyncTarget,
                };
                let tx_prog = tx.clone();
                let ctx_prog = repaint_ctx.clone();
                let res = match target {
                    VcSyncTarget::BaseEngine => {
                        download_and_install_base_engine(&dir, move |message, progress| {
                            let _ = tx_prog.send(VcSyncEvent::Progress { message, progress });
                            if let Some(ctx) = &ctx_prog {
                                ctx.request_repaint();
                            }
                        })
                    }
                    VcSyncTarget::VoiceModel(model_id) => download_and_install_voice_model(
                        &dir,
                        model_id,
                        move |message, progress| {
                            let _ = tx_prog.send(VcSyncEvent::Progress { message, progress });
                            if let Some(ctx) = &ctx_prog {
                                ctx.request_repaint();
                            }
                        },
                    ),
                };
                let _ = tx.send(VcSyncEvent::Finished {
                    target,
                    result: res,
                });
                if let Some(ctx) = repaint_ctx {
                    ctx.request_repaint();
                }
            });
    }

    /// Polls background voice model download progress and returns `(is_error, toast_message)` notifications.
    pub fn poll_vc_sync(&mut self) -> Vec<(bool, String)> {
        use crate::devices::voice_changer::model_sync::{VcSyncEvent, VcSyncTarget};
        let mut notes = Vec::new();
        let mut clear_rx = false;
        if let Some(rx) = &self.vc_sync_rx {
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    VcSyncEvent::Progress { message, progress } => {
                        self.vc_sync_progress = Some((message, progress));
                    }
                    VcSyncEvent::Finished {
                        target,
                        result: Ok(()),
                    } => {
                        self.vc_sync_in_progress = false;
                        self.vc_sync_target = None;
                        self.vc_sync_progress = None;
                        self.vc_sync_error = None;
                        clear_rx = true;
                        self.vc_telemetry.request_reload();
                        let label = match target {
                            VcSyncTarget::BaseEngine => {
                                "Voice Changer ONNX engine & base models installed.".to_string()
                            }
                            VcSyncTarget::VoiceModel(m) => {
                                format!("Voice model '{}' downloaded and ready.", m.short_label())
                            }
                        };
                        notes.push((false, label));
                    }
                    VcSyncEvent::Finished {
                        target: _,
                        result: Err(err),
                    } => {
                        self.vc_sync_in_progress = false;
                        self.vc_sync_target = None;
                        self.vc_sync_progress = None;
                        self.vc_sync_error = Some(err.clone());
                        clear_rx = true;
                        notes.push((true, format!("Voice Changer download failed: {err}")));
                    }
                }
            }
        }
        if clear_rx {
            self.vc_sync_rx = None;
        }
        notes
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
    pub vtuber_supported_formats: Vec<VideoFormat>,
    pub vtuber_formats_probed_for_device: Option<String>,
    pub vtuber_selected_format_index: usize,
    pub vtuber_format_probe_gen: u64,
    pub vtuber_format_probing: bool,
    pub new_local_profile_name: String,
    pub new_vtuber_profile_name: String,
    pub active_stream: Option<crate::devices::webcam::WebcamStreamHandle>,
    pub stream_routed_to_vtuber: bool,
    pub texture: Option<egui::TextureHandle>,
    pub latest_frame: Option<Arc<crate::devices::webcam::WebcamFrame>>,
    pub frame_receiver:
        Option<crossbeam_channel::Receiver<Arc<crate::devices::webcam::WebcamFrame>>>,
    pub vtuber: crate::vtuber::VtuberState,
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
            vtuber_supported_formats: Vec::new(),
            vtuber_formats_probed_for_device: None,
            vtuber_selected_format_index: 0,
            vtuber_format_probe_gen: 0,
            vtuber_format_probing: false,
            new_local_profile_name: String::new(),
            new_vtuber_profile_name: String::new(),
            active_stream: None,
            stream_routed_to_vtuber: false,
            texture: None,
            latest_frame: None,
            frame_receiver: None,
            vtuber: crate::vtuber::VtuberState::default(),
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

#[derive(Debug, Clone)]
pub enum AsyncFileDialogResult {
    VideoFile(std::path::PathBuf),
    GamescopeScript(std::path::PathBuf),
    PitchDictZip(std::path::PathBuf),
    Pc98Media(crate::pc98::Pc98MediaSlot, std::path::PathBuf),
    Pc88Media(crate::pc88::Pc88MediaSlot, std::path::PathBuf),
    VtuberModelFile(std::path::PathBuf),
    VtuberBackgroundImage(std::path::PathBuf),
}

#[derive(Debug, Clone)]
pub enum V4l2FormatProbeTarget {
    Capture {
        saved_fourcc: Option<String>,
        saved_res: Option<(u32, u32)>,
        saved_fps: Option<u32>,
        announce: bool,
    },
    Webcam {
        select_first_defaults: bool,
        announce_err: bool,
    },
    VtuberTracker {
        select_first_defaults: bool,
        announce_err: bool,
    },
}

pub struct V4l2FormatProbeResult {
    pub generation: u64,
    pub device: String,
    pub target: V4l2FormatProbeTarget,
    pub formats: Result<Vec<VideoFormat>, String>,
}

/// Presentation phase for AMD FidelityFX 30 -> 60 FPS frame generation on PS5 Remote Play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ps5FrameGenPhase {
    /// Presenting the real decoded PS5 frame `F(t)`.
    Real,
    /// Presenting the synthesized optical-flow midpoint frame `F(t - 0.5)`.
    Interpolated,
}

/// Paces 30 FPS PS5 Remote Play source frames into a locked 60 FPS alternating
/// `(Interpolated F(t - 0.5), Real F(t))` presentation cadence with centered +/-16.7 ms jitter
/// absorption, presentation-aligned phase locking (compensating `Interpolated` vs `Real` GPU
/// compute asymmetry on non-VSync desktop), hardware 60 Hz VSync detection, non-skipping frame
/// staging, and strictly monotonic Replay/Twitch timestamps.
#[derive(Clone)]
pub struct Ps5FrameGenPacer {
    pub phase: Ps5FrameGenPhase,
    pub prev_captured_at: Option<i64>,
    pub curr_captured_at: Option<i64>,
    /// Capture timestamp (`RawFrame.captured_at`) of the most recently enqueued PS5 source frame,
    /// used to measure true source period independent of GUI VSync/sleep jitter.
    pub last_enqueued_captured_at: Option<i64>,
    pub last_source_arrival: Option<Instant>,
    pub last_resolution: Option<(u32, u32)>,
    pub smoothed_interval: std::time::Duration,
    /// Bounded staging FIFO (`<= 3` frames) so an early PS5 arrival or LAN burst never clobbers
    /// an unshown `Real` frame or drops a source frame while `primed` holds 1 cushion frame.
    pub pending_frames: VecDeque<(Instant, Arc<RawFrame>)>,
    /// True once the 1-beat (16.67 ms) startup jitter cushion has been primed, centering
    /// `Interpolated` promotions in the middle of the 33.33 ms PS5 arrival window.
    pub primed: bool,
    /// Base 60 Hz phase grid target `Instant` (un-offset) for the next presentation beat.
    pub next_beat_at: Option<Instant>,
    /// Wall-clock `Instant` right after `sleep_until_precise` on the prior beat (includes
    /// `recv_timeout` + render + `SwapBuffers`) used to detect hardware 60 Hz VSync.
    pub last_beat_wake: Option<Instant>,
    /// Wall-clock `Instant` right after `recv_timeout` and immediately before render + `SwapBuffers`
    /// on the prior beat, used to measure pure per-phase GPU/render duration.
    pub last_render_start: Option<Instant>,
    /// Which phase (`Interpolated` vs `Real`) was rendered on the most recent beat.
    pub last_phase_rendered: Option<Ps5FrameGenPhase>,
    /// Count of consecutive beats whose wake-to-entry cycle was `>= 14.5 ms`. Requiring `>= 2`
    /// consecutive beats distinguishes true hardware 60 Hz VSync (where even the lightweight `Real`
    /// beat blocks `>= 14.5 ms`) from a single heavy `Interpolated` compute beat on desktop.
    pub consecutive_60hz_swaps: u8,
    /// True when hardware 60 Hz VSync is actively pacing `SwapBuffers()` on both phases.
    pub is_60hz_vsync_locked: bool,
    /// Live EMA of pure `Interpolated` beat render+swap duration (used to align `Real` presentation).
    pub ema_interp_work: Option<std::time::Duration>,
    /// Live EMA of pure `Real` beat render+swap duration.
    pub ema_real_work: Option<std::time::Duration>,
    /// Monotonic smoothed Replay/Twitch presentation timestamp for the active beat (us).
    pub current_beat_ts: Option<i64>,
    /// Last emitted Replay/Twitch presentation timestamp (us).
    pub last_emitted_ts: i64,
}

impl Default for Ps5FrameGenPacer {
    fn default() -> Self {
        Self {
            phase: Ps5FrameGenPhase::Real,
            prev_captured_at: None,
            curr_captured_at: None,
            last_enqueued_captured_at: None,
            last_source_arrival: None,
            last_resolution: None,
            smoothed_interval: Self::DEFAULT_INTERVAL,
            pending_frames: VecDeque::with_capacity(4),
            primed: false,
            next_beat_at: None,
            last_beat_wake: None,
            last_render_start: None,
            last_phase_rendered: None,
            consecutive_60hz_swaps: 0,
            is_60hz_vsync_locked: false,
            ema_interp_work: None,
            ema_real_work: None,
            current_beat_ts: None,
            last_emitted_ts: 0,
        }
    }
}

impl Ps5FrameGenPacer {
    pub const DEFAULT_INTERVAL: std::time::Duration = std::time::Duration::from_micros(33_333);
    /// Symmetric single-frame source delta window (`33_333 +/- 15_333 us`) for EMA sampling so
    /// even heavy +/-15 ms LAN jitter is never asymmetrically clipped.
    const MIN_SAMPLE_INTERVAL_US: i64 = 18_000;
    const MAX_SAMPLE_INTERVAL_US: i64 = 48_666;
    /// Crystal-locked bounds around PS5 29.97 Hz (`33_367 us`) / 30.00 Hz (`33_333 us`) encoding
    /// (+/-13 us margin, preventing LAN jitter from causing duplicate or skipped 60 Hz CFR ticks).
    const MIN_SMOOTHED_INTERVAL: std::time::Duration = std::time::Duration::from_micros(33_320);
    const MAX_SMOOTHED_INTERVAL: std::time::Duration = std::time::Duration::from_micros(33_380);
    /// Skip interpolation for one beat only on a genuine multi-frame stall/dropout (`> 80 ms`),
    /// avoiding false resets on normal single-frame LAN jitter swings (e.g. -10 ms / +16 ms).
    const DISCONTINUITY_GAP: std::time::Duration = std::time::Duration::from_millis(80);
    /// Maximum staged source frames: holds 1 primed cushion frame + 1 early arrival + 1 burst
    /// frame without silently dropping source frames while capping worst-case stall latency.
    pub const MAX_PENDING_FRAMES: usize = 3;
    /// Continue driving 60 Hz beats for this window after the most recent PS5 frame arrival.
    pub const ACTIVE_STREAM_WINDOW: std::time::Duration = std::time::Duration::from_millis(160);
    /// Swap-cycle threshold above which `SwapBuffers()` may be hardware-paced at ~60 Hz VSync
    /// (requires 2 consecutive beats so a single heavy `Interpolated` beat never false-triggers).
    const VSYNC_60HZ_MIN_CYCLE: std::time::Duration = std::time::Duration::from_micros(14_500);
    /// Maximum `recv_timeout` wait on 60 Hz VSync vs non-60Hz-VSync displays.
    pub const WAIT_BUDGET_60HZ: std::time::Duration = std::time::Duration::from_millis(6);
    pub const WAIT_BUDGET_HIGH_HZ: std::time::Duration = std::time::Duration::from_millis(5);
    /// Minimum age (us) of the first staged continuation frame to consider it already centered
    /// in the 33.33 ms PS5 arrival window (half a 60 Hz beat = 8.33 ms).
    const PRIME_ALREADY_CENTERED_MIN_AGE_US: i64 = 8_333;
    const PRIME_ALREADY_CENTERED_MAX_AGE_US: i64 = 40_000;
    const MIN_WORK_EMA: std::time::Duration = std::time::Duration::from_micros(200);
    const MAX_WORK_EMA: std::time::Duration = std::time::Duration::from_millis(22);

    pub fn reset(&mut self) {
        self.phase = Ps5FrameGenPhase::Real;
        self.prev_captured_at = None;
        self.curr_captured_at = None;
        self.last_enqueued_captured_at = None;
        self.last_source_arrival = None;
        self.last_resolution = None;
        self.smoothed_interval = Self::DEFAULT_INTERVAL;
        self.pending_frames.clear();
        self.primed = false;
        self.next_beat_at = None;
        self.last_beat_wake = None;
        self.last_render_start = None;
        self.last_phase_rendered = None;
        self.consecutive_60hz_swaps = 0;
        self.is_60hz_vsync_locked = false;
        self.ema_interp_work = None;
        self.ema_real_work = None;
        self.current_beat_ts = None;
        // Preserve `last_emitted_ts` monotonically so Replay/Twitch timestamps never go backwards
        // across a quick toggle or reset.
    }

    pub fn take_all_pending(&mut self) -> Vec<Arc<RawFrame>> {
        self.pending_frames.drain(..).map(|(_, f)| f).collect()
    }

    /// Enqueues a newly decoded PS5 source frame without clobbering the currently displayed pair.
    /// Measures `smoothed_interval` strictly from `frame.captured_at` deltas inside a symmetric
    /// acceptance window so GUI sleep/VSync timing and LAN jitter can never bias the interval.
    pub fn enqueue_frame(&mut self, arrived_at: Instant, frame: Arc<RawFrame>) {
        let last_seen_ts = self
            .pending_frames
            .back()
            .map(|(_, f)| f.captured_at)
            .or(self.curr_captured_at);
        if last_seen_ts == Some(frame.captured_at) {
            return;
        }
        if let Some(prev_src_ts) = self.last_enqueued_captured_at {
            let dt_us = frame.captured_at.saturating_sub(prev_src_ts);
            if (Self::MIN_SAMPLE_INTERVAL_US..=Self::MAX_SAMPLE_INTERVAL_US).contains(&dt_us) {
                let ema_us =
                    self.smoothed_interval.as_micros() as f64 * 0.98 + (dt_us as f64) * 0.02;
                self.smoothed_interval = std::time::Duration::from_micros(ema_us.round() as u64)
                    .clamp(Self::MIN_SMOOTHED_INTERVAL, Self::MAX_SMOOTHED_INTERVAL);
            }
        }
        self.last_enqueued_captured_at = Some(frame.captured_at);
        self.last_source_arrival = Some(arrived_at);
        self.pending_frames.push_back((arrived_at, frame));
        while self.pending_frames.len() > Self::MAX_PENDING_FRAMES {
            self.pending_frames.pop_front();
        }
    }

    /// Target 60 Hz beat period (~16.67 ms), with proportional queue-pressure PLL drain when
    /// 2+ source frames are queued so LAN bursts drain smoothly without overflowing `pending_frames`.
    pub fn beat_duration(&self) -> std::time::Duration {
        let base = (self.smoothed_interval / 2).clamp(
            std::time::Duration::from_micros(16_660),
            std::time::Duration::from_micros(16_690),
        );
        match self.pending_frames.len() {
            0 | 1 => base,
            2 => base.saturating_sub(std::time::Duration::from_micros(800)),
            _ => base.saturating_sub(std::time::Duration::from_micros(1_800)),
        }
    }

    fn update_work_ema(slot: &mut Option<std::time::Duration>, sample: std::time::Duration) {
        let clamped = sample.clamp(Self::MIN_WORK_EMA, Self::MAX_WORK_EMA);
        *slot = Some(match *slot {
            None => clamped,
            Some(prev) => {
                let ema_us = prev.as_micros() as f64 * 0.85 + clamped.as_micros() as f64 * 0.15;
                std::time::Duration::from_micros(ema_us.round() as u64)
                    .clamp(Self::MIN_WORK_EMA, Self::MAX_WORK_EMA)
            }
        });
    }

    /// Computes whether `pace_gui_frame()` should sleep before rendering this 60 Hz beat:
    /// - Detects true hardware 60 Hz VSync only when `>= 2` consecutive beats block `>= 14.5 ms`
    ///   (so even the lightweight `Real` beat is VSync-bound); never sleeps against 60 Hz VSync.
    /// - On desktop `vsync: false` / 120/144/240 Hz / VRR displays:
    ///   1. Preserves a locked 60 Hz phase grid (`grid + beat_dur`) across minor `<= 1`-beat
    ///      overruns so a fast `Real` beat immediately recovers a heavy `Interpolated` compute beat
    ///      (preventing permanent 58.5 FPS drift and staging-queue overflows).
    ///   2. Delays ONLY the lightweight `Real` beat by `clamp(ema_interp - ema_real, 0, margin)` so
    ///      actual `SwapBuffers` presentation cadence stays evenly spaced at `16.67 ms / 16.67 ms`
    ///      without waking `Interpolated` early or starving the `primed` jitter cushion.
    pub fn compute_beat_sleep_target(&mut self, entry_now: Instant) -> Option<Instant> {
        if let Some(prev_wake) = self.last_beat_wake {
            let wake_cycle = entry_now.saturating_duration_since(prev_wake);
            if wake_cycle >= Self::VSYNC_60HZ_MIN_CYCLE {
                self.consecutive_60hz_swaps = self.consecutive_60hz_swaps.saturating_add(1).min(4);
            } else {
                self.consecutive_60hz_swaps = 0;
            }
            self.is_60hz_vsync_locked = self.consecutive_60hz_swaps >= 2;
        } else {
            self.consecutive_60hz_swaps = 0;
            self.is_60hz_vsync_locked = false;
        }

        if !self.is_60hz_vsync_locked {
            if let (Some(render_start), Some(prev_phase)) =
                (self.last_render_start, self.last_phase_rendered)
            {
                let work = entry_now.saturating_duration_since(render_start);
                match prev_phase {
                    Ps5FrameGenPhase::Interpolated => {
                        Self::update_work_ema(&mut self.ema_interp_work, work);
                    }
                    Ps5FrameGenPhase::Real => {
                        Self::update_work_ema(&mut self.ema_real_work, work);
                    }
                }
            }
        }

        let beat_dur = self.beat_duration();
        if self.is_60hz_vsync_locked {
            self.next_beat_at = Some(entry_now + beat_dur);
            return None;
        }

        // Predict whether THIS beat will run `Interpolated` or `Real`:
        // - If the previous beat was `Interpolated`, this beat is unconditionally `Real`.
        // - If the previous beat was `Real` on a primed stream, this beat will run `Interpolated`.
        let will_be_interp =
            self.phase == Ps5FrameGenPhase::Real && self.primed && self.curr_captured_at.is_some();
        let real_phase_offset = if will_be_interp {
            std::time::Duration::ZERO
        } else if let (Some(ti), Some(tr)) = (self.ema_interp_work, self.ema_real_work) {
            let raw_lead = ti.saturating_sub(tr);
            let max_lead = beat_dur
                .saturating_sub(tr)
                .saturating_sub(std::time::Duration::from_micros(500));
            raw_lead.min(max_lead)
        } else {
            std::time::Duration::ZERO
        };

        match self.next_beat_at {
            None => {
                self.next_beat_at = Some(entry_now + beat_dur);
                None
            }
            Some(grid) => {
                let target_wake = grid + real_phase_offset;
                if entry_now >= target_wake {
                    // Minor overrun (<= 1 beat, e.g. heavy Interpolated compute pass): keep the
                    // locked phase grid `grid + beat_dur` so the immediately following fast Real
                    // beat recovers the overrun instead of permanently drifting slower.
                    if entry_now <= grid + beat_dur {
                        self.next_beat_at = Some(grid + beat_dur);
                    } else {
                        self.next_beat_at = Some(entry_now + beat_dur);
                    }
                    None
                } else {
                    let max_target = entry_now + beat_dur * 2;
                    let clamped = target_wake.min(max_target);
                    self.next_beat_at = if clamped < target_wake {
                        Some(clamped + beat_dur)
                    } else {
                        Some(grid + beat_dur)
                    };
                    Some(clamped)
                }
            }
        }
    }

    /// Records the post-sleep wake instant immediately before `recv_timeout`, rendering, and
    /// `SwapBuffers()` so the next beat measures the full wake-to-swap cycle for 60 Hz VSync detection.
    pub fn record_beat_wake(&mut self, wake_now: Instant) {
        self.last_beat_wake = Some(wake_now);
    }

    /// Records the post-`recv_timeout` render start instant and active phase immediately before
    /// `draw_video_player()` + `SwapBuffers()` so `ema_interp_work` / `ema_real_work` measure
    /// pure per-phase GPU/render duration without `recv_timeout` contamination.
    pub fn record_render_start(&mut self, render_start: Instant) {
        self.last_render_start = Some(render_start);
        self.last_phase_rendered = Some(self.phase);
    }

    /// Bounded `recv_timeout` budget when waiting for a slightly late PS5 packet right after `Real`.
    pub fn next_frame_wait_budget(&self) -> std::time::Duration {
        if self.is_60hz_vsync_locked {
            Self::WAIT_BUDGET_60HZ
        } else {
            Self::WAIT_BUDGET_HIGH_HZ
        }
    }

    /// Returns true when we just finished presenting `Real` and the staging queue is empty on an
    /// active stream — allowing `pace_gui_frame()` a short bounded wait to catch a slightly late
    /// PS5 LAN frame before painting.
    pub fn should_wait_for_next_frame(&self, now: Instant) -> bool {
        self.phase == Ps5FrameGenPhase::Real
            && self.primed
            && self.pending_frames.is_empty()
            && self.curr_captured_at.is_some()
            && self
                .last_source_arrival
                .is_some_and(|t| now.saturating_duration_since(t) <= Self::ACTIVE_STREAM_WINDOW)
    }

    /// Returns true while the PS5 stream is active or an interpolated/staged frame is pending,
    /// driving steady 60 Hz continuous repaint without `request_repaint_after` timer overshoot.
    pub fn should_request_continuous_repaint(&self, now: Instant) -> bool {
        self.show_interpolated()
            || !self.pending_frames.is_empty()
            || self
                .last_source_arrival
                .is_some_and(|t| now.saturating_duration_since(t) <= Self::ACTIVE_STREAM_WINDOW)
    }

    fn advance_beat_timestamp(&mut self, wall_now_us: i64, fallback_ts: i64) {
        let est_work_us = if self.is_60hz_vsync_locked {
            0i64
        } else {
            match self.phase {
                Ps5FrameGenPhase::Interpolated => {
                    self.ema_interp_work.map_or(0, |d| d.as_micros() as i64)
                }
                Ps5FrameGenPhase::Real => self.ema_real_work.map_or(0, |d| d.as_micros() as i64),
            }
        };
        let anchor = if wall_now_us > 0 {
            wall_now_us.saturating_add(est_work_us)
        } else {
            fallback_ts
        };
        let half_us = (self.smoothed_interval.as_micros() as i64 / 2).clamp(16_660, 16_690);
        let next_ts = if self.last_emitted_ts <= 0 {
            anchor
        } else {
            let ideal = self.last_emitted_ts + half_us;
            let err = anchor - ideal;
            // Snap directly on startup priming holds or genuine stalls (> 40 ms) so a one-time
            // offset never bleeds into artificial +350 us/beat Replay tick skips later.
            if err.abs() > 40_000 {
                anchor.max(self.last_emitted_ts + 1)
            } else {
                // Gentle PLL pull (+/-0.35 ms max/beat) keeps A/V sync tight while ensuring
                // consecutive beats advance 60 FPS Replay CFR ticks by exact +1.
                let pull = (err / 16).clamp(-350, 350);
                (ideal + pull).max(self.last_emitted_ts + 1)
            }
        };
        self.last_emitted_ts = next_ts;
        self.current_beat_ts = Some(next_ts);
    }

    /// Advances the presentation state machine by one 60 Hz beat BEFORE `draw_video_player()`.
    ///
    /// Guarantees strict alternation & centered jitter immunity:
    /// - From `Interpolated`: ALWAYS transitions to `Real` for the current frame (never skipped!).
    /// - From `Real`: holds the very first continuous follow-up frame for 1 beat (`16.67 ms`) when
    ///   only 1 frame is staged to prime a centered `+/-16.7 ms` LAN jitter cushion; thereafter
    ///   promotes the next staged PS5 frame `F(t)` into `Interpolated` (or `Real` on first frame /
    ///   resolution change / dropped source frame `> 58 ms` gap).
    pub fn step_beat(&mut self, wall_now_us: i64) -> Option<Arc<RawFrame>> {
        match self.phase {
            Ps5FrameGenPhase::Interpolated => {
                self.phase = Ps5FrameGenPhase::Real;
                let fallback = self.curr_captured_at.unwrap_or(wall_now_us);
                self.advance_beat_timestamp(wall_now_us, fallback);
                None
            }
            Ps5FrameGenPhase::Real => {
                let Some((_, peek_frame)) = self.pending_frames.front() else {
                    return None;
                };
                let ts_gap_ok = self.curr_captured_at.is_some_and(|prev_ts| {
                    if prev_ts > 0 && peek_frame.captured_at > prev_ts {
                        (peek_frame.captured_at - prev_ts)
                            <= Self::DISCONTINUITY_GAP.as_micros() as i64
                    } else {
                        true
                    }
                });
                let res = (peek_frame.width, peek_frame.height);
                let has_valid_prev = self.curr_captured_at.is_some()
                    && self.last_resolution == Some(res)
                    && ts_gap_ok;

                if has_valid_prev && !self.primed {
                    if self.pending_frames.len() >= 2 {
                        self.primed = true;
                    } else {
                        let age_us = if wall_now_us > 0 && peek_frame.captured_at > 0 {
                            wall_now_us.saturating_sub(peek_frame.captured_at)
                        } else {
                            0
                        };
                        if (Self::PRIME_ALREADY_CENTERED_MIN_AGE_US
                            ..=Self::PRIME_ALREADY_CENTERED_MAX_AGE_US)
                            .contains(&age_us)
                        {
                            // Already sat >= 8.33 ms in the channel -> centered inside the
                            // 33.33 ms PS5 arrival window; promote immediately with no extra beat.
                            self.primed = true;
                        } else {
                            // Fresh arrival (< 8.33 ms old): hold 1 single 60 Hz beat (16.67 ms)
                            // on startup/resync so subsequent 30 FPS arrivals sit centered in a
                            // +/-16.7 ms jitter window.
                            self.primed = true;
                            return None;
                        }
                    }
                }

                let (_, next_frame) = self.pending_frames.pop_front()?;

                if has_valid_prev {
                    self.prev_captured_at = self.curr_captured_at;
                    self.curr_captured_at = Some(next_frame.captured_at);
                    self.last_resolution = Some(res);
                    self.phase = Ps5FrameGenPhase::Interpolated;
                } else {
                    self.prev_captured_at = None;
                    self.curr_captured_at = Some(next_frame.captured_at);
                    self.last_resolution = Some(res);
                    self.phase = Ps5FrameGenPhase::Real;
                    self.primed = false;
                }
                self.advance_beat_timestamp(wall_now_us, next_frame.captured_at);
                Some(next_frame)
            }
        }
    }

    pub fn show_interpolated(&self) -> bool {
        matches!(self.phase, Ps5FrameGenPhase::Interpolated)
    }

    pub fn effective_captured_at(&self, real_captured_at: i64) -> i64 {
        self.current_beat_ts.unwrap_or(real_captured_at)
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
    pub halo_presets: BTreeMap<String, HaloSettings>,
    pub active_halo_preset: String,
    pub new_halo_preset_name: String,
    pub beam: BeamSettings,
    pub beam_defaults: BeamSettings,
    pub beam_presets: BTreeMap<String, BeamMaskPreset>,
    pub active_beam_preset: String,
    pub new_beam_preset_name: String,
    pub beam_lockstep: BeamLockstepState,
    pub ps5_framegen_pacer: Ps5FrameGenPacer,
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
    pub file_dialog_receiver: Option<crossbeam_channel::Receiver<AsyncFileDialogResult>>,
    pub usb_reset_in_progress: bool,
    pub usb_reset_receiver: Option<crossbeam_channel::Receiver<(bool, Result<(), String>)>>,
    pub capture_format_probe_gen: u64,
    pub webcam_format_probe_gen: u64,
    pub capture_format_probing: bool,
    pub webcam_format_probing: bool,
    pub v4l2_format_probe_tx: crossbeam_channel::Sender<V4l2FormatProbeResult>,
    pub v4l2_format_probe_rx: crossbeam_channel::Receiver<V4l2FormatProbeResult>,
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
    pub ps5_profile: config::Profile,
    pub previous_normal_profile: Option<String>,
    pub previous_gamescope_normal_profile: Option<String>,
    pub previous_pc98_normal_profile: Option<String>,
    pub previous_pc88_normal_profile: Option<String>,
    pub previous_ps5_normal_profile: Option<String>,
    pub previous_capture_resolution: (u32, u32),
    pub previous_capture_framerate: u32,
    pub gamescope_hook: Option<crate::gamescope::GamescopeHookState>,
    pub gamescope_launcher_script: String,
    pub steam_games: Vec<crate::gamescope::steam::SteamGame>,
    pub selected_steam_game: Option<String>,
    pub retro_pc_system: crate::pc88::RetroPcSystem,
    pub pc98: crate::pc98::Pc98State,
    pub pc88: crate::pc88::Pc88State,
    pub ps5: crate::ps5::Ps5State,

    pub fft_filter: Option<Arc<Mutex<crate::video::gpu::FftFilter>>>,
    pub fft_mask_data: Vec<u8>,
    pub fft_mask_dirty: bool,
    pub fft_mask_resolution: (u32, u32),
    pub fft_brush_radius: f32,
    pub fft_mask_threshold: f32,
    pub fft_black_threshold: f32,
    pub fft_mask_save_name: String,
    pub fft_available_masks: Vec<String>,
    pub fft_masks_scanned_res: Option<(u32, u32)>,
    pub ocr: crate::ocr::OcrState,
    pub dict: crate::dict::DictState,
    pub bank: crate::bank::BankState,
    pub twitch: crate::twitch::TwitchState,
    pub mic: MicState,
    pub webcam: WebcamState,
    pub whiteboard: crate::ui::whiteboard::WhiteboardState,
    pub screenshot: crate::screenshot::ScreenshotState,
    pub replay_scrub: crate::ui::replay_scrub::ReplayScrubState,
    pub loaded_config_base: Option<config::MichadameConfig>,
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
        assert!(state.fft_masks_scanned_res.is_none());
        assert!(!state.ui.is_file_dialog_open());
        assert!(!state.usb_reset_in_progress);
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
        assert_eq!(state.hardware.live_audio_db_peak, 0.0);
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

    #[test]
    fn test_halo_builtin_presets_validity_and_distinctness() {
        let list = HaloSettings::builtin_preset_list();
        assert_eq!(list.len(), 12);
        let map = HaloSettings::builtin_presets();
        assert_eq!(map.len(), 12);

        for (name, preset) in &list {
            assert!(
                HaloSettings::is_builtin_preset(name),
                "Preset '{}' must be recognized as builtin",
                name
            );
            assert_eq!(
                HaloSettings::builtin_preset(name).as_ref(),
                Some(preset),
                "builtin_preset('{}') must return matching preset",
                name
            );
            assert_eq!(
                *preset,
                preset.sanitized(),
                "Builtin preset '{}' must be within valid sanitized bounds",
                name
            );
        }
    }

    #[test]
    fn test_beam_builtin_presets_validity_and_distinctness() {
        let list = BeamMaskPreset::builtin_preset_list();
        assert_eq!(list.len(), 12);
        let map = BeamMaskPreset::builtin_presets();
        assert_eq!(map.len(), 12);

        for (name, preset) in &list {
            assert!(
                BeamMaskPreset::is_builtin_preset(name),
                "Beam preset '{}' must be recognized as builtin",
                name
            );
            assert_eq!(
                BeamMaskPreset::builtin_preset(name).as_ref(),
                Some(preset),
                "Beam builtin_preset('{}') must return matching preset",
                name
            );
            assert_eq!(
                *preset,
                preset.sanitized(),
                "Beam builtin preset '{}' must be within valid sanitized bounds",
                name
            );
        }

        // Verify apply_mask_preset only updates Stage 2 parameters and preserves Stage 1 beam simulator settings
        let mut beam = BeamSettings::default();
        beam.frames_per_hz = 4.0;
        beam.target_hz = 240;
        beam.gamma = 2.2;
        beam.gain_vs_blur = 0.60;
        beam.fps_divisor = 0.5;
        beam.scan_direction = 2;

        let toshiba = BeamMaskPreset::preset_toshiba_af();
        beam.apply_mask_preset(&toshiba);
        assert_eq!(beam.mask_preset(), toshiba);
        assert_eq!(beam.frames_per_hz, 4.0);
        assert_eq!(beam.target_hz, 240);
        assert_eq!(beam.gamma, 2.2);
        assert_eq!(beam.gain_vs_blur, 0.60);
        assert_eq!(beam.fps_divisor, 0.5);
        assert_eq!(beam.scan_direction, 2);
    }

    fn make_dummy_ps5_frame(captured_at: i64, width: u32, height: u32) -> Arc<RawFrame> {
        Arc::new(RawFrame {
            captured_at,
            rate: crate::replay::config::Rate { num: 30, den: 1 },
            width,
            height,
            data: Vec::new(),
            format: ffmpeg_next::format::Pixel::RGBA,
            color_range: crate::video::types::ColorRange::Full,
        })
    }

    #[test]
    fn ps5_framegen_pacer_alternates_strictly_without_skipping_real_under_jitter() {
        let mut pacer = Ps5FrameGenPacer::default();
        let t0 = Instant::now();

        // First frame displays Real immediately (no prior frame to interpolate from).
        pacer.enqueue_frame(t0, make_dummy_ps5_frame(100_000, 1920, 1080));
        let promoted0 = pacer.step_beat(100_000);
        assert_eq!(promoted0.as_ref().map(|f| f.captured_at), Some(100_000));
        assert!(!pacer.show_interpolated());
        let ts0 = pacer.effective_captured_at(100_000);
        assert_eq!(ts0, 100_000);

        // Fresh F1 arrival (< 8.33 ms old) holds 1 single 60 Hz beat on startup to prime the
        // centered +/-16.7 ms jitter cushion, then promotes F1 on the immediately following beat.
        let jitter_us: [i64; 8] = [-5_500, 4_200, -6_000, 5_800, -3_200, 2_900, -4_800, 6_000];
        let f1_ts = 100_000 + 33_333 + jitter_us[1];
        pacer.enqueue_frame(
            t0 + std::time::Duration::from_micros((f1_ts - 100_000) as u64),
            make_dummy_ps5_frame(f1_ts, 1920, 1080),
        );
        assert!(
            pacer.step_beat(f1_ts + 500).is_none(),
            "Fresh F1 (< 8.33 ms old) must hold 1 beat to center the jitter cushion"
        );
        assert!(pacer.primed);

        // Simulate 60 source 30 FPS PS5 frames arriving with realistic +/-6 ms LAN jitter
        // (including early arrivals while `Interpolated` is currently on screen!).
        // Verify:
        // 1. ZERO `Real` frames are ever skipped!
        // 2. Every source frame emits `(Interpolated, Real)` strictly in order.
        // 3. `ReplayGpu::should_capture` accepts 100% of all 120 paced 60 FPS beats!
        // 4. 60 Hz Replay CFR ticks advance by exact +1 every beat (ZERO sawtooth drops/doubles).
        let replay_gpu = crate::replay::gpu::Readback::default();
        assert!(replay_gpu.should_capture(ts0));

        let mut last_ts = ts0;
        let mut last_tick: Option<i64> = None;
        let mut sim_wall_us = 100_000i64;
        let mut accepted_beats = 0usize;

        for k in 1..=60i64 {
            let nominal_us = 100_000 + k * 33_333;
            let src_ts = nominal_us + jitter_us[(k as usize) % jitter_us.len()];
            if k > 1 {
                let arr_t = t0 + std::time::Duration::from_micros((src_ts - 100_000).max(0) as u64);
                pacer.enqueue_frame(arr_t, make_dummy_ps5_frame(src_ts, 1920, 1080));
            }

            // Beat 2k-1: promote F_k -> Interpolated.
            sim_wall_us += 16_667;
            let p_interp = pacer.step_beat(sim_wall_us);
            assert_eq!(
                p_interp.as_ref().map(|f| f.captured_at),
                Some(src_ts),
                "Beat should promote F_{k}"
            );
            assert!(
                pacer.show_interpolated(),
                "Continuous source frame F_{k} must enter Interpolated phase"
            );
            let ts_interp = pacer.effective_captured_at(src_ts);
            assert!(ts_interp > last_ts, "Timestamps must be strictly monotonic");
            assert!(
                replay_gpu.should_capture(ts_interp),
                "Replay should_capture must not drop paced Interp beat {k} (ts={ts_interp}, prev={last_ts})"
            );
            let tick_interp = ((ts_interp - ts0) as f64 * 60.0 / 1_000_000.0).round() as i64;
            if let Some(prev_tick) = last_tick {
                assert_eq!(
                    tick_interp,
                    prev_tick + 1,
                    "Replay CFR tick must advance by exact +1 on Interp beat {k}"
                );
            }
            last_tick = Some(tick_interp);
            last_ts = ts_interp;
            accepted_beats += 1;

            // Even if F_{k+1} arrives EARLY before Real F_k is painted, Real F_k MUST NOT be skipped!
            if k < 60 && (k % 3 == 0) {
                let next_nom = 100_000 + (k + 1) * 33_333;
                let next_ts = next_nom + jitter_us[((k + 1) as usize) % jitter_us.len()];
                let early_t =
                    t0 + std::time::Duration::from_micros((next_ts - 100_000).max(0) as u64);
                pacer.enqueue_frame(early_t, make_dummy_ps5_frame(next_ts, 1920, 1080));
            }

            // Beat 2k: unconditionally reveal Real F_k!
            sim_wall_us += 16_667;
            let p_real = pacer.step_beat(sim_wall_us);
            assert!(
                p_real.is_none(),
                "Real beat for F_{k} must keep current frame without clobbering it"
            );
            assert!(
                !pacer.show_interpolated(),
                "Beat after Interpolated must always present Real F_{k}"
            );
            let ts_real = pacer.effective_captured_at(src_ts);
            assert!(ts_real > last_ts, "Real timestamp must strictly advance");
            assert!(
                replay_gpu.should_capture(ts_real),
                "Replay should_capture must not drop paced Real beat {k} (ts={ts_real}, prev={last_ts})"
            );
            let tick_real = ((ts_real - ts0) as f64 * 60.0 / 1_000_000.0).round() as i64;
            assert_eq!(
                tick_real,
                last_tick.unwrap() + 1,
                "Replay CFR tick must advance by exact +1 on Real beat {k}"
            );
            last_tick = Some(tick_real);
            last_ts = ts_real;
            accepted_beats += 1;
        }
        assert_eq!(accepted_beats, 120);

        // Genuine multi-frame stream stall (> 80 ms gap) cleanly falls back to Real immediately
        // (without wasting a priming hold on a discontinuity frame) and resets `primed`.
        let drop_ts = pacer.curr_captured_at.unwrap() + 95_000;
        pacer.enqueue_frame(
            t0 + std::time::Duration::from_secs(3),
            make_dummy_ps5_frame(drop_ts, 1920, 1080),
        );
        let p_drop = pacer.step_beat(drop_ts + 500);
        assert_eq!(p_drop.as_ref().map(|f| f.captured_at), Some(drop_ts));
        assert!(!pacer.show_interpolated());
        assert!(!pacer.primed);

        // If the next continuous frame after a drop has already sat >= 8.33 ms in the channel,
        // it is already centered and promotes immediately into Interpolated without an extra beat.
        let centered_ts = drop_ts + 33_333;
        pacer.enqueue_frame(
            t0 + std::time::Duration::from_secs(3) + std::time::Duration::from_millis(33),
            make_dummy_ps5_frame(centered_ts, 1920, 1080),
        );
        let p_centered = pacer.step_beat(centered_ts + 12_000);
        assert_eq!(
            p_centered.as_ref().map(|f| f.captured_at),
            Some(centered_ts)
        );
        assert!(pacer.show_interpolated());
        assert!(pacer.primed);
        // Complete its Real beat before testing resolution change.
        assert!(pacer.step_beat(centered_ts + 28_667).is_none());
        assert!(!pacer.show_interpolated());

        // Resolution change also falls back cleanly to Real for 1 beat.
        let res_ts = centered_ts + 33_333;
        pacer.enqueue_frame(
            t0 + std::time::Duration::from_secs(3) + std::time::Duration::from_millis(66),
            make_dummy_ps5_frame(res_ts, 1280, 720),
        );
        let p_res = pacer.step_beat(res_ts + 500);
        assert_eq!(p_res.as_ref().map(|f| f.captured_at), Some(res_ts));
        assert!(!pacer.show_interpolated());

        // Queue depth cap: bursting 5 frames never retains more than MAX_PENDING_FRAMES (3).
        for i in 1..=5i64 {
            pacer.enqueue_frame(
                t0 + std::time::Duration::from_secs(4),
                make_dummy_ps5_frame(res_ts + i * 33_333, 1280, 720),
            );
        }
        assert_eq!(
            pacer.pending_frames.len(),
            Ps5FrameGenPacer::MAX_PENDING_FRAMES
        );
        // Proportional queue-pressure PLL engages smoothly: len=3 < len=2 < len=1.
        let burst3_beat = pacer.beat_duration();
        pacer.pending_frames.pop_front();
        let burst2_beat = pacer.beat_duration();
        pacer.pending_frames.pop_front();
        let normal_beat = pacer.beat_duration();
        assert!(burst3_beat < burst2_beat && burst2_beat < normal_beat);
    }

    #[test]
    fn ps5_framegen_pacer_prevents_smoothed_interval_sawtooth_runaway() {
        let mut pacer = Ps5FrameGenPacer::default();
        let t0 = Instant::now();

        // Feed 120 crystal-locked 30.00 Hz PS5 frames (`captured_at` += 33_333 us) while
        // artificially delaying the GUI drain instant (`arrived_at`) by +4 ms every frame
        // (reproducing the Round 8 `recv_timeout` positive-feedback loop).
        // `smoothed_interval` MUST measure strictly from `captured_at` and stay tightly locked!
        for k in 0..120i64 {
            let src_ts = 1_000_000 + k * 33_333;
            let delayed_gui_instant = t0 + std::time::Duration::from_micros((k as u64) * 37_333);
            pacer.enqueue_frame(
                delayed_gui_instant,
                make_dummy_ps5_frame(src_ts, 1920, 1080),
            );
        }
        assert!(
            pacer.smoothed_interval >= std::time::Duration::from_micros(33_320)
                && pacer.smoothed_interval <= std::time::Duration::from_micros(33_380),
            "smoothed_interval must never run away from GUI drain jitter: {:?}",
            pacer.smoothed_interval
        );
        assert!(
            (pacer.smoothed_interval.as_micros() as i64 - 33_333).abs() <= 50,
            "smoothed_interval should stay locked to true 33.33 ms source period: {:?}",
            pacer.smoothed_interval
        );

        // Even with a deliberately high worst-case clamped interval (33.38 ms -> beat_dur =
        // 16.690 ms > 16.667 ms monitor VSync), true hardware 60 Hz VSync (>= 2 consecutive
        // beats with swap_cycle >= 14.5 ms) must NEVER sleep or drift into sawtooth misses!
        pacer.smoothed_interval = std::time::Duration::from_micros(33_380);
        pacer.pending_frames.clear();
        assert_eq!(pacer.compute_beat_sleep_target(t0), None);
        pacer.record_beat_wake(t0);

        for beat in 1..=60u64 {
            let vsync_entry = t0 + std::time::Duration::from_micros(beat * 16_667);
            let sleep_target = pacer.compute_beat_sleep_target(vsync_entry);
            if beat >= 2 {
                assert_eq!(
                    sleep_target, None,
                    "60 Hz VSync beat {beat} must never sleep into a sawtooth miss"
                );
                assert!(pacer.is_60hz_vsync_locked);
                assert_eq!(
                    pacer.next_frame_wait_budget(),
                    Ps5FrameGenPacer::WAIT_BUDGET_60HZ
                );
            }
            pacer.record_beat_wake(vsync_entry);
        }
    }

    #[test]
    fn ps5_framegen_pacer_phase_locks_and_aligns_presentation_under_heavy_interp_gpu_asymmetry() {
        // Reproduces desktop `vsync: false` when `Interpolated` compute takes 17.5 ms (> 16.67 ms!)
        // while `Real` takes 3.5 ms (21.0 ms total << 33.33 ms):
        // 1. Must NOT false-trigger `is_60hz_vsync_locked` (which previously caused permanent
        //    58.5 FPS drift and periodic staging-queue overflow drops!).
        // 2. Must preserve the locked 60 Hz phase grid so `Real` recovers `Interpolated`'s overrun,
        //    emitting ZERO dropped source frames and ZERO steady-state Replay CFR tick skips.
        // 3. Must offset `Real` presentation so actual SwapBuffers intervals stay tightly centered
        //    around 16.67 ms (instead of 3.5 ms / 29.8 ms judder!).
        let mut pacer = Ps5FrameGenPacer::default();
        let t0 = Instant::now();
        let interp_work = std::time::Duration::from_micros(17_500);
        let real_work = std::time::Duration::from_micros(3_500);

        let jitter_us: [i64; 8] = [-5_000, 4_000, -5_500, 5_000, -3_000, 2_500, -4_500, 5_500];
        let mut src_arrivals_us = Vec::new();
        for k in 0..160i64 {
            let nom = k * 33_333;
            let t = if k == 0 {
                0
            } else {
                (nom + jitter_us[(k as usize) % jitter_us.len()]).max(0)
            };
            src_arrivals_us.push(t);
        }

        let replay_gpu = crate::replay::gpu::Readback::default();
        let mut src_idx = 0usize;
        let mut now_us = 0u64;
        let mut last_seen_at = 0i64;
        let mut epoch: Option<i64> = None;
        let mut last_tick: Option<i64> = None;
        let mut steady_tick_skips = 0usize;
        let mut prev_present_us: Option<u64> = None;
        let mut steady_disp_dts_ms = Vec::new();
        let mut prev_phase: Option<Ps5FrameGenPhase> = None;
        let mut steady_parity_flips = 0usize;
        let mut promoted_count = 0usize;

        while now_us < 4_500_000 {
            let entry = t0 + std::time::Duration::from_micros(now_us);
            if let Some(target) = pacer.compute_beat_sleep_target(entry) {
                let target_us = target.saturating_duration_since(t0).as_micros() as u64;
                if target_us > now_us {
                    now_us = target_us;
                }
            }
            assert!(
                !pacer.is_60hz_vsync_locked,
                "Alternating 17.5 ms Interp / 3.5 ms Real on desktop must never trip 60 Hz VSync lock"
            );

            let wake = t0 + std::time::Duration::from_micros(now_us);
            pacer.record_beat_wake(wake);

            while src_idx < src_arrivals_us.len() && (src_arrivals_us[src_idx] as u64) <= now_us {
                let ts = 1_000_000 + src_arrivals_us[src_idx];
                pacer.enqueue_frame(wake, make_dummy_ps5_frame(ts, 1920, 1080));
                src_idx += 1;
            }
            if pacer.should_wait_for_next_frame(wake) {
                let budget_us = pacer.next_frame_wait_budget().as_micros() as u64;
                if src_idx < src_arrivals_us.len()
                    && (src_arrivals_us[src_idx] as u64) <= now_us + budget_us
                {
                    now_us = src_arrivals_us[src_idx] as u64;
                    let arrived = t0 + std::time::Duration::from_micros(now_us);
                    while src_idx < src_arrivals_us.len()
                        && (src_arrivals_us[src_idx] as u64) <= now_us
                    {
                        let ts = 1_000_000 + src_arrivals_us[src_idx];
                        pacer.enqueue_frame(arrived, make_dummy_ps5_frame(ts, 1920, 1080));
                        src_idx += 1;
                    }
                } else {
                    now_us += budget_us;
                }
            }

            let render_start = t0 + std::time::Duration::from_micros(now_us);
            if pacer.step_beat(1_000_000 + now_us as i64).is_some() {
                promoted_count += 1;
            }
            pacer.record_render_start(render_start);

            let work = if pacer.show_interpolated() {
                interp_work
            } else {
                real_work
            };
            now_us += work.as_micros() as u64;

            if now_us > 400_000 {
                if let Some(pp) = prev_phase {
                    if pp == pacer.phase {
                        steady_parity_flips += 1;
                    }
                }
                prev_phase = Some(pacer.phase);

                if let Some(prev_p) = prev_present_us {
                    steady_disp_dts_ms.push((now_us - prev_p) as f64 / 1000.0);
                }
            }
            prev_present_us = Some(now_us);

            let at = pacer.effective_captured_at(1_000_000 + now_us as i64);
            if at > last_seen_at {
                last_seen_at = at;
                assert!(
                    replay_gpu.should_capture(at),
                    "Replay should_capture dropped beat at now={}ms",
                    now_us / 1000
                );
                let ep = *epoch.get_or_insert(at);
                let tick = ((at - ep) as f64 * 60.0 / 1_000_000.0).round() as i64;
                if let Some(prev_t) = last_tick {
                    if now_us > 400_000 && tick != prev_t + 1 {
                        steady_tick_skips += 1;
                    }
                }
                last_tick = Some(tick);
            }
        }

        // Every source frame that arrived (minus <= 2 still staged) must have been promoted (0 drops!).
        let dropped = src_idx.saturating_sub(promoted_count + pacer.pending_frames.len());
        assert_eq!(
            dropped, 0,
            "Heavy Interp pacer must drop ZERO source frames"
        );
        assert_eq!(
            steady_parity_flips, 0,
            "Heavy Interp pacer must have ZERO steady-state parity flips"
        );
        assert_eq!(
            steady_tick_skips, 0,
            "Heavy Interp pacer must have ZERO steady-state 60 Hz Replay CFR tick skips"
        );
        let min_dt = steady_disp_dts_ms
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let max_dt = steady_disp_dts_ms.iter().copied().fold(0.0_f64, f64::max);
        assert!(
            min_dt >= 14.0 && max_dt <= 20.0,
            "Presentation intervals must be balanced (min={min_dt:.2}ms, max={max_dt:.2}ms)"
        );
    }

    #[test]
    fn ps5_framegen_pacer_sleep_targets_handle_60hz_vsync_and_120hz_displays() {
        let mut pacer = Ps5FrameGenPacer::default();
        let t0 = Instant::now();

        // First beat never sleeps.
        assert_eq!(pacer.compute_beat_sleep_target(t0), None);
        pacer.record_beat_wake(t0);

        // 60 Hz VSync: SwapBuffers returns at +16.67 ms (>= 14.5 ms) -> ZERO sleep every beat
        // and `is_60hz_vsync_locked` engages on the 2nd consecutive 60 Hz swap!
        for i in 1..=10u64 {
            let vsync_t = t0 + std::time::Duration::from_micros(i * 16_667);
            assert_eq!(
                pacer.compute_beat_sleep_target(vsync_t),
                None,
                "60 Hz VSync beat {i} must not incur extra sleep"
            );
            if i >= 2 {
                assert!(pacer.is_60hz_vsync_locked);
            }
            pacer.record_beat_wake(vsync_t);
        }

        // 120 Hz VSync: SwapBuffers returns early at +8.33 ms (< 14.5 ms) -> sleeps to ~16.67 ms!
        let mut pacer120 = Ps5FrameGenPacer::default();
        assert_eq!(pacer120.compute_beat_sleep_target(t0), None);
        pacer120.record_beat_wake(t0);
        let early_return = t0 + std::time::Duration::from_micros(8_333);
        let sleep_to = pacer120
            .compute_beat_sleep_target(early_return)
            .expect("120 Hz early SwapBuffers must pace to next 60 Hz beat");
        assert!(!pacer120.is_60hz_vsync_locked);
        assert_eq!(
            pacer120.next_frame_wait_budget(),
            Ps5FrameGenPacer::WAIT_BUDGET_HIGH_HZ
        );
        let sleep_dur = sleep_to.saturating_duration_since(early_return);
        assert!(
            sleep_dur >= std::time::Duration::from_millis(7)
                && sleep_dur <= std::time::Duration::from_millis(10),
            "Unexpected 120 Hz pacing sleep duration: {:?}",
            sleep_dur
        );

        // Idle detection stops continuous repaint after ACTIVE_STREAM_WINDOW.
        pacer120.enqueue_frame(t0, make_dummy_ps5_frame(50_000, 1920, 1080));
        let _ = pacer120.step_beat(50_000);
        assert!(
            pacer120.should_request_continuous_repaint(t0 + std::time::Duration::from_millis(50))
        );
        assert!(
            !pacer120.should_request_continuous_repaint(t0 + std::time::Duration::from_millis(250))
        );
    }
}
