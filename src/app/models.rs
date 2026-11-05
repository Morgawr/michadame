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
    pub is_window_focused: bool,
    /// Resolution-aware UI scale (desktop only).
    pub scale: crate::ui::scaling::UiScaleState,
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
    pub halo_presets: BTreeMap<String, HaloSettings>,
    pub active_halo_preset: String,
    pub new_halo_preset_name: String,
    pub beam: BeamSettings,
    pub beam_defaults: BeamSettings,
    pub beam_presets: BTreeMap<String, BeamMaskPreset>,
    pub active_beam_preset: String,
    pub new_beam_preset_name: String,
    pub beam_lockstep: BeamLockstepState,
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
    pub screenshot: crate::screenshot::ScreenshotState,
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
}
