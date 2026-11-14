use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StreamingVideoEncoder {
    #[default]
    SoftwareX264,
    HardwareVaapi,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StreamingAudioEncoder {
    #[default]
    Aac,
}

/// Global Twitch integration settings (not tied to a profile).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct TwitchConfig {
    /// Normalized channel login (lowercase, no `#`).
    pub channel: String,
    /// Show the live chat overlay on the video window.
    pub chat_overlay_enabled: bool,
    /// Overlay width as a fraction of the video window width.
    pub overlay_width_pct: f32,
    /// Overlay background opacity (0..1).
    pub overlay_opacity: f32,
    pub font_size: f32,
    /// Seconds before a chat message disappears. 0 = never.
    pub message_lifetime_secs: u32,
    pub max_messages: u32,
    /// Optional Twitch application Client ID override. Empty = built-in.
    pub client_id: String,
    /// Fire each chat message across the video, niconico style.
    pub niconico_enabled: bool,
    /// Comment text height as a fraction of the video height.
    pub niconico_size_pct: f32,
    /// Average seconds a comment takes to cross the screen.
    pub niconico_duration_secs: f32,
    /// Most comments on screen at once (oldest dropped first). 0 = unlimited.
    pub niconico_max_comments: u32,

    // Twitch Live Streaming Output settings
    pub video_bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
    pub video_encoder: StreamingVideoEncoder,
    pub encoder_preset: String,
    pub audio_encoder: StreamingAudioEncoder,
    pub ingest_server: String,
    pub stream_key: String,

    // Video alerts (followers, subs, bits, raids)
    pub alerts_enabled: bool,
    pub alerts_duration_secs: f32,
    pub alert_follows: bool,
    pub alert_subs: bool,
    pub alert_bits: bool,
    pub alert_raids: bool,
    pub alert_min_bits: u32,

    /// Automatically share newly mined words to Twitch chat while live streaming.
    pub share_mined_words_to_chat: bool,

    /// Play a pre-roll video when going live on Twitch.
    pub preroll_enabled: bool,
    /// Absolute or relative path to the pre-roll video file on disk.
    pub preroll_video_path: String,

    /// Play an outro video when ending a Twitch stream.
    pub outro_enabled: bool,
    /// Absolute or relative path to the outro video file on disk.
    pub outro_video_path: String,
}

impl Default for TwitchConfig {
    fn default() -> Self {
        Self {
            channel: String::new(),
            chat_overlay_enabled: false,
            overlay_width_pct: 0.22,
            overlay_opacity: 0.65,
            font_size: 14.0,
            message_lifetime_secs: 120,
            max_messages: 150,
            client_id: String::new(),
            niconico_enabled: false,
            niconico_size_pct: 0.06,
            niconico_duration_secs: 7.0,
            niconico_max_comments: 0,

            video_bitrate_kbps: 6000,
            audio_bitrate_kbps: 160,
            video_encoder: StreamingVideoEncoder::SoftwareX264,
            encoder_preset: "veryfast".into(),
            audio_encoder: StreamingAudioEncoder::Aac,
            ingest_server: "rtmps://live.twitch.tv:443/app/".into(),
            stream_key: String::new(),

            alerts_enabled: false,
            alerts_duration_secs: 5.0,
            alert_follows: true,
            alert_subs: true,
            alert_bits: true,
            alert_raids: true,
            alert_min_bits: 1,

            share_mined_words_to_chat: false,

            preroll_enabled: false,
            preroll_video_path: String::new(),

            outro_enabled: false,
            outro_video_path: String::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct NoiseGateConfig {
    pub enabled: bool,
    pub close_threshold_db: f32,
    pub open_threshold_db: f32,
    pub attack_time_ms: f32,
    pub hold_time_ms: f32,
    pub release_time_ms: f32,
}

impl Default for NoiseGateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            close_threshold_db: -52.0,
            open_threshold_db: -42.0,
            attack_time_ms: 0.0,
            hold_time_ms: 400.0,
            release_time_ms: 0.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct GainConfig {
    pub enabled: bool,
    pub gain_db: f32,
}

impl Default for GainConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            gain_db: 6.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct CompressorConfig {
    pub enabled: bool,
    pub ratio: f32,
    pub threshold_db: f32,
    pub attack_time_ms: f32,
    pub release_time_ms: f32,
    pub output_gain_db: f32,
}

impl Default for CompressorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ratio: 4.0,
            threshold_db: -18.0,
            attack_time_ms: 6.0,
            release_time_ms: 60.0,
            output_gain_db: -4.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum VoiceModelId {
    #[default]
    TsukuyomiChan,
    Amitaro,
    KikotoKurage,
    KikotoMahiro,
    TokinaShigure,
}

impl VoiceModelId {
    pub const ALL: [Self; 5] = [
        Self::TsukuyomiChan,
        Self::Amitaro,
        Self::KikotoKurage,
        Self::KikotoMahiro,
        Self::TokinaShigure,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::TsukuyomiChan => "Tsukuyomi-chan (Girl — Calm / Clear)",
            Self::Amitaro => "Amitaro (Girl — Cute / Bright Anime)",
            Self::KikotoKurage => "Kikoto Kurage (Girl — Soft / Gentle)",
            Self::KikotoMahiro => "Kikoto Mahiro (Boyish / Androgynous)",
            Self::TokinaShigure => "Tokina Shigure (Cool / Mature Male)",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::TsukuyomiChan => "Tsukuyomi-chan",
            Self::Amitaro => "Amitaro",
            Self::KikotoKurage => "Kikoto Kurage",
            Self::KikotoMahiro => "Kikoto Mahiro",
            Self::TokinaShigure => "Tokina Shigure",
        }
    }

    pub fn filename(self) -> &'static str {
        match self {
            Self::TsukuyomiChan => "tsukuyomi_v2_40k_e100_float.onnx",
            Self::Amitaro => "amitaro_v2_40k_e100_float.onnx",
            Self::KikotoKurage => "kikoto_kurage_v2_40k_e100_float.onnx",
            Self::KikotoMahiro => "kikoto_mahiro_v2_40k_float.onnx",
            Self::TokinaShigure => "tokina_shigure_v2_40k_e100_float.onnx",
        }
    }

    pub fn credit_notice(self) -> &'static str {
        match self {
            Self::TsukuyomiChan => {
                "CV: Rei Yumesaki (Tsukuyomi-chan Corpus / Project Tsukuyomi-chan). Follow character terms when publishing."
            }
            Self::Amitaro => {
                "Voice Material: Amitaro's Voice Material Workshop (amitaro.net). Follow workshop credit terms when publishing."
            }
            Self::KikotoKurage => {
                "Character Voice: Kikoto Kurage (istoria). Follow character usage guidelines when streaming or publishing."
            }
            Self::KikotoMahiro => {
                "Character Voice: Kikoto Mahiro (istoria). Follow character usage guidelines when streaming or publishing."
            }
            Self::TokinaShigure => {
                "Character Voice: Tokina Shigure. Follow character usage guidelines when streaming or publishing."
            }
        }
    }

    pub fn default_pitch_shift(self) -> f32 {
        match self {
            Self::TsukuyomiChan | Self::Amitaro | Self::KikotoKurage => 12.0,
            Self::KikotoMahiro => 6.0,
            Self::TokinaShigure => -2.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VoiceModelKind {
    #[default]
    BuiltIn,
    CustomOnnx,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VcExecutionProvider {
    #[default]
    AutoGpu,
    WebGpuVulkan,
    RocmMiGraphX,
    Cuda,
    CpuOnly,
}

impl VcExecutionProvider {
    pub const ALL: [Self; 5] = [
        Self::AutoGpu,
        Self::WebGpuVulkan,
        Self::RocmMiGraphX,
        Self::Cuda,
        Self::CpuOnly,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::AutoGpu => "Auto (GPU -> CPU Fallback)",
            Self::WebGpuVulkan => "GPU — Vulkan / WebGPU",
            Self::RocmMiGraphX => "GPU — AMD ROCm / MIGraphX",
            Self::Cuda => "GPU — NVIDIA CUDA",
            Self::CpuOnly => "CPU Only (Multi-Thread)",
        }
    }
}

pub const VOICE_CHANGER_DEFAULT_PRESET: &str = "Girl — Calm / Clear (Tsukuyomi)";
pub const VALID_VC_CHUNK_MS: [u32; 4] = [40, 60, 80, 120];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct VoiceChangerPreset {
    // Keep all fields scalar for clean TOML table serialization.
    pub model_kind: VoiceModelKind,
    pub builtin_model: VoiceModelId,
    pub custom_model_path: String,
    pub pitch_shift_semitones: f32,
    pub formant_shift_semitones: f32,
    pub protect: f32,
    pub silence_threshold_db: f32,
    pub pre_denoise: bool,
    pub chunk_ms: u32,
    pub extra_context_ms: u32,
    pub output_gain_db: f32,
    pub dry_wet: f32,
}

impl Default for VoiceChangerPreset {
    fn default() -> Self {
        Self::preset_girl_calm_tsukuyomi()
    }
}

fn sanitize_vc_f32(v: f32, default: f32, min: f32, max: f32) -> f32 {
    if v.is_finite() {
        v.clamp(min, max)
    } else {
        default
    }
}

fn snap_vc_chunk_ms(ms: u32) -> u32 {
    VALID_VC_CHUNK_MS
        .into_iter()
        .min_by_key(|&cand| cand.abs_diff(ms))
        .unwrap_or(80)
}

impl VoiceChangerPreset {
    pub fn preset_girl_calm_tsukuyomi() -> Self {
        Self {
            model_kind: VoiceModelKind::BuiltIn,
            builtin_model: VoiceModelId::TsukuyomiChan,
            custom_model_path: String::new(),
            pitch_shift_semitones: VoiceModelId::TsukuyomiChan.default_pitch_shift(),
            formant_shift_semitones: 0.3,
            protect: 0.33,
            silence_threshold_db: -52.0,
            pre_denoise: true,
            chunk_ms: 80,
            extra_context_ms: 400,
            output_gain_db: 0.0,
            dry_wet: 1.0,
        }
    }

    pub fn preset_girl_cute_amitaro() -> Self {
        Self {
            builtin_model: VoiceModelId::Amitaro,
            pitch_shift_semitones: 12.0,
            formant_shift_semitones: 0.8,
            protect: 0.35,
            ..Self::preset_girl_calm_tsukuyomi()
        }
    }

    pub fn preset_girl_soft_kurage() -> Self {
        Self {
            builtin_model: VoiceModelId::KikotoKurage,
            pitch_shift_semitones: 12.0,
            formant_shift_semitones: 0.4,
            protect: 0.36,
            ..Self::preset_girl_calm_tsukuyomi()
        }
    }

    pub fn preset_boyish_mahiro() -> Self {
        Self {
            builtin_model: VoiceModelId::KikotoMahiro,
            pitch_shift_semitones: 6.0,
            formant_shift_semitones: 0.0,
            protect: 0.30,
            ..Self::preset_girl_calm_tsukuyomi()
        }
    }

    pub fn preset_cool_male_shigure() -> Self {
        Self {
            builtin_model: VoiceModelId::TokinaShigure,
            pitch_shift_semitones: -2.0,
            formant_shift_semitones: -0.4,
            protect: 0.25,
            ..Self::preset_girl_calm_tsukuyomi()
        }
    }

    pub fn sanitized(&self) -> Self {
        let extra = ((self.extra_context_ms.clamp(160, 800) + 10) / 20) * 20;
        Self {
            model_kind: self.model_kind,
            builtin_model: self.builtin_model,
            custom_model_path: self.custom_model_path.trim().to_string(),
            pitch_shift_semitones: sanitize_vc_f32(self.pitch_shift_semitones, 12.0, -24.0, 24.0),
            formant_shift_semitones: sanitize_vc_f32(self.formant_shift_semitones, 0.3, -4.0, 4.0),
            protect: sanitize_vc_f32(self.protect, 0.33, 0.0, 0.5),
            silence_threshold_db: sanitize_vc_f32(self.silence_threshold_db, -52.0, -70.0, -20.0),
            pre_denoise: self.pre_denoise,
            chunk_ms: snap_vc_chunk_ms(self.chunk_ms),
            extra_context_ms: extra.clamp(160, 800),
            output_gain_db: sanitize_vc_f32(self.output_gain_db, 0.0, -18.0, 18.0),
            dry_wet: sanitize_vc_f32(self.dry_wet, 1.0, 0.0, 1.0),
        }
    }

    pub fn builtin_preset_list() -> Vec<(&'static str, Self)> {
        vec![
            (
                VOICE_CHANGER_DEFAULT_PRESET,
                Self::preset_girl_calm_tsukuyomi(),
            ),
            (
                "Girl — Cute Anime (Amitaro)",
                Self::preset_girl_cute_amitaro(),
            ),
            (
                "Girl — Soft / Gentle (Kurage)",
                Self::preset_girl_soft_kurage(),
            ),
            (
                "Boyish / Androgynous (Mahiro)",
                Self::preset_boyish_mahiro(),
            ),
            (
                "Cool / Mature Male (Shigure)",
                Self::preset_cool_male_shigure(),
            ),
        ]
    }

    pub fn builtin_presets() -> BTreeMap<String, Self> {
        Self::builtin_preset_list()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    pub fn builtin_preset(name: &str) -> Option<Self> {
        Self::builtin_preset_list()
            .into_iter()
            .find_map(|(k, v)| (k == name).then_some(v))
    }

    pub fn is_builtin_preset(name: &str) -> bool {
        matches!(
            name,
            VOICE_CHANGER_DEFAULT_PRESET
                | "Girl — Cute Anime (Amitaro)"
                | "Girl — Soft / Gentle (Kurage)"
                | "Boyish / Androgynous (Mahiro)"
                | "Cool / Mature Male (Shigure)"
        )
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct VoiceChangerConfig {
    // Scalar fields MUST precede nested tables/maps for confy TOML serialization.
    pub enabled: bool,
    pub execution_provider: VcExecutionProvider,
    pub active_preset: String,
    pub current: VoiceChangerPreset,
    pub presets: BTreeMap<String, VoiceChangerPreset>,
}

impl Default for VoiceChangerConfig {
    fn default() -> Self {
        let presets = VoiceChangerPreset::builtin_presets();
        let active_preset = VOICE_CHANGER_DEFAULT_PRESET.to_string();
        let current = presets.get(&active_preset).cloned().unwrap_or_default();
        Self {
            enabled: false,
            execution_provider: VcExecutionProvider::AutoGpu,
            active_preset,
            current,
            presets,
        }
    }
}

impl VoiceChangerConfig {
    pub fn ensure_builtin_presets(&mut self) {
        for (name, preset) in VoiceChangerPreset::builtin_presets() {
            match self.presets.get_mut(&name) {
                Some(existing) => {
                    // Migrate unmodified initial v1 defaults (60ms chunk / 240ms context)
                    let mut legacy_v1 = preset.clone();
                    legacy_v1.chunk_ms = 60;
                    legacy_v1.extra_context_ms = 240;
                    if *existing == legacy_v1 {
                        *existing = preset;
                    }
                }
                None => {
                    self.presets.insert(name, preset);
                }
            }
        }
        for preset in self.presets.values_mut() {
            *preset = preset.sanitized();
        }
        if let Some(builtin) = VoiceChangerPreset::builtin_preset(&self.active_preset) {
            let mut legacy_cur = builtin.clone();
            legacy_cur.chunk_ms = 60;
            legacy_cur.extra_context_ms = 240;
            if self.current == legacy_cur {
                self.current = builtin;
            }
        }
        self.current = self.current.sanitized();
        if self.active_preset.trim().is_empty() || !self.presets.contains_key(&self.active_preset) {
            self.active_preset = VOICE_CHANGER_DEFAULT_PRESET.to_string();
        }
    }

    pub fn apply_preset(&mut self, name: &str) -> bool {
        self.ensure_builtin_presets();
        if let Some(preset) = self.presets.get(name).cloned() {
            let clean = preset.sanitized();
            self.active_preset = name.to_string();
            self.current = clean;
            true
        } else {
            false
        }
    }

    pub fn sanitized(&self) -> Self {
        let mut copy = self.clone();
        copy.ensure_builtin_presets();
        copy
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct MicConfig {
    pub enabled: bool,
    pub device_name: Option<String>,
    pub volume: f32,
    pub monitor: bool,
    pub noise_gate: NoiseGateConfig,
    pub gain: GainConfig,
    pub compressor: CompressorConfig,
    pub voice_changer: VoiceChangerConfig,
}

impl Default for MicConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            device_name: None,
            volume: 1.0,
            monitor: false,
            noise_gate: NoiseGateConfig::default(),
            gain: GainConfig::default(),
            compressor: CompressorConfig::default(),
            voice_changer: VoiceChangerConfig::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct WebcamCutout {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

impl Default for WebcamCutout {
    fn default() -> Self {
        Self {
            left: 0,
            right: 0,
            top: 0,
            bottom: 0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WebcamSourceMode {
    #[default]
    LocalCamera,
    VtuberAvatar,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VtuberDisplayStyle {
    #[default]
    FramedWebcam,
    BorderlessCutout,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VtuberBackgroundKind {
    #[default]
    #[serde(alias = "RetroBackdrop")]
    SolidColor,
    StaticImage,
    Transparent,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VtuberAspectPreset {
    #[default]
    Wide16x9,
    Classic4x3,
    Square1x1,
    Portrait3x4,
}

impl VtuberAspectPreset {
    pub const ALL: [Self; 4] = [
        Self::Wide16x9,
        Self::Classic4x3,
        Self::Square1x1,
        Self::Portrait3x4,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Wide16x9 => "16:9 Wide",
            Self::Classic4x3 => "4:3 Classic",
            Self::Square1x1 => "1:1 Square",
            Self::Portrait3x4 => "3:4 Bust",
        }
    }

    pub fn virtual_dimensions(self) -> (u32, u32) {
        match self {
            Self::Wide16x9 => (960, 540),
            Self::Classic4x3 => (768, 576),
            Self::Square1x1 => (640, 640),
            Self::Portrait3x4 => (576, 768),
        }
    }
}

pub const DEFAULT_LOCAL_WEBCAM_PROFILE: &str = "Default";
pub const VTUBER_PROFILE_FRAMED: &str = "Framed Webcam";
pub const VTUBER_PROFILE_CUTOUT: &str = "Borderless Cutout";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct LocalWebcamProfile {
    pub device: Option<String>,
    pub format_fourcc: Option<String>,
    pub resolution: Option<(u32, u32)>,
    pub framerate: Option<u32>,
    pub pos_x_pct: f32,
    pub pos_y_pct: f32,
    pub width_pct: f32,
    // Keep nested table after scalar fields for confy/TOML serialization.
    pub cutout: WebcamCutout,
}

impl Default for LocalWebcamProfile {
    fn default() -> Self {
        Self {
            device: None,
            format_fourcc: None,
            resolution: None,
            framerate: None,
            pos_x_pct: 0.72,
            pos_y_pct: 0.65,
            width_pct: 0.24,
            cutout: WebcamCutout::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct VtuberOverlayLayout {
    pub pos_x_pct: f32,
    pub pos_y_pct: f32,
    pub width_pct: f32,
    pub aspect_preset: VtuberAspectPreset,
    // Keep nested table after scalar fields for confy/TOML serialization.
    pub cutout: WebcamCutout,
}

impl Default for VtuberOverlayLayout {
    fn default() -> Self {
        Self::default_framed()
    }
}

impl VtuberOverlayLayout {
    pub fn default_framed() -> Self {
        Self {
            pos_x_pct: 0.72,
            pos_y_pct: 0.65,
            width_pct: 0.24,
            aspect_preset: VtuberAspectPreset::Wide16x9,
            cutout: WebcamCutout::default(),
        }
    }

    pub fn default_cutout() -> Self {
        Self {
            pos_x_pct: 0.76,
            pos_y_pct: 0.48,
            width_pct: 0.20,
            aspect_preset: VtuberAspectPreset::Portrait3x4,
            cutout: WebcamCutout::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct VtuberProfile {
    pub model_path: Option<String>,
    pub tracking_device: Option<String>,
    pub tracking_format_fourcc: Option<String>,
    pub tracking_resolution: Option<(u32, u32)>,
    pub tracking_framerate: Option<u32>,
    pub display_style: VtuberDisplayStyle,
    pub background_kind: VtuberBackgroundKind,
    pub background_color: [u8; 4],
    pub background_image_path: Option<String>,
    pub camera_zoom: f32,
    pub camera_offset_y: f32,
    pub smoothing: f32,
    pub blink_sensitivity: f32,
    pub mouth_sensitivity: f32,
    pub movement_scale: f32,
    pub mirror_tracking: bool,
    pub mic_lipsync_assist: bool,
    pub neutral_head_quat: Option<[f32; 4]>,
    // Keep nested tables after scalar fields for confy/TOML serialization.
    pub framed_layout: VtuberOverlayLayout,
    pub cutout_layout: VtuberOverlayLayout,
}

impl Default for VtuberProfile {
    fn default() -> Self {
        Self::default_framed()
    }
}

impl VtuberProfile {
    pub fn default_framed() -> Self {
        Self {
            model_path: None,
            tracking_device: None,
            tracking_format_fourcc: None,
            tracking_resolution: None,
            tracking_framerate: None,
            display_style: VtuberDisplayStyle::FramedWebcam,
            background_kind: VtuberBackgroundKind::SolidColor,
            background_color: [26, 24, 42, 255],
            background_image_path: None,
            camera_zoom: 1.0,
            camera_offset_y: 0.0,
            smoothing: 0.55,
            blink_sensitivity: 1.0,
            mouth_sensitivity: 1.15,
            movement_scale: 1.0,
            mirror_tracking: true,
            mic_lipsync_assist: false,
            neutral_head_quat: None,
            framed_layout: VtuberOverlayLayout::default_framed(),
            cutout_layout: VtuberOverlayLayout::default_cutout(),
        }
    }

    pub fn default_cutout() -> Self {
        Self {
            display_style: VtuberDisplayStyle::BorderlessCutout,
            camera_zoom: 1.05,
            camera_offset_y: -0.02,
            ..Self::default_framed()
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct VtuberConfig {
    pub active_profile: String,
    pub model_path: Option<String>,
    pub tracking_device: Option<String>,
    pub tracking_format_fourcc: Option<String>,
    pub tracking_resolution: Option<(u32, u32)>,
    pub tracking_framerate: Option<u32>,
    pub display_style: VtuberDisplayStyle,
    pub background_kind: VtuberBackgroundKind,
    pub background_color: [u8; 4],
    pub background_image_path: Option<String>,
    pub camera_zoom: f32,
    pub camera_offset_y: f32,
    pub smoothing: f32,
    pub blink_sensitivity: f32,
    pub mouth_sensitivity: f32,
    pub movement_scale: f32,
    pub mirror_tracking: bool,
    pub mic_lipsync_assist: bool,
    pub neutral_head_quat: Option<[f32; 4]>,
    // Keep nested tables/maps after scalar fields for confy/TOML serialization.
    pub framed_layout: VtuberOverlayLayout,
    pub cutout_layout: VtuberOverlayLayout,
    pub profiles: BTreeMap<String, VtuberProfile>,
}

impl Default for VtuberConfig {
    fn default() -> Self {
        Self {
            active_profile: VTUBER_PROFILE_FRAMED.to_string(),
            model_path: None,
            tracking_device: None,
            tracking_format_fourcc: None,
            tracking_resolution: None,
            tracking_framerate: None,
            display_style: VtuberDisplayStyle::FramedWebcam,
            background_kind: VtuberBackgroundKind::SolidColor,
            background_color: [26, 24, 42, 255],
            background_image_path: None,
            camera_zoom: 1.0,
            camera_offset_y: 0.0,
            smoothing: 0.55,
            blink_sensitivity: 1.0,
            mouth_sensitivity: 1.15,
            movement_scale: 1.0,
            mirror_tracking: true,
            mic_lipsync_assist: false,
            neutral_head_quat: None,
            framed_layout: VtuberOverlayLayout::default_framed(),
            cutout_layout: VtuberOverlayLayout::default_cutout(),
            profiles: BTreeMap::new(),
        }
    }
}

impl VtuberConfig {
    pub fn active_layout(&self) -> &VtuberOverlayLayout {
        match self.display_style {
            VtuberDisplayStyle::FramedWebcam => &self.framed_layout,
            VtuberDisplayStyle::BorderlessCutout => &self.cutout_layout,
        }
    }

    pub fn active_layout_mut(&mut self) -> &mut VtuberOverlayLayout {
        match self.display_style {
            VtuberDisplayStyle::FramedWebcam => &mut self.framed_layout,
            VtuberDisplayStyle::BorderlessCutout => &mut self.cutout_layout,
        }
    }

    pub fn snapshot_profile(&self) -> VtuberProfile {
        VtuberProfile {
            model_path: self.model_path.clone(),
            tracking_device: self.tracking_device.clone(),
            tracking_format_fourcc: self.tracking_format_fourcc.clone(),
            tracking_resolution: self.tracking_resolution,
            tracking_framerate: self.tracking_framerate,
            display_style: self.display_style,
            background_kind: self.background_kind,
            background_color: self.background_color,
            background_image_path: self.background_image_path.clone(),
            camera_zoom: self.camera_zoom,
            camera_offset_y: self.camera_offset_y,
            smoothing: self.smoothing,
            blink_sensitivity: self.blink_sensitivity,
            mouth_sensitivity: self.mouth_sensitivity,
            movement_scale: self.movement_scale,
            mirror_tracking: self.mirror_tracking,
            mic_lipsync_assist: self.mic_lipsync_assist,
            neutral_head_quat: self.neutral_head_quat,
            framed_layout: self.framed_layout.clone(),
            cutout_layout: self.cutout_layout.clone(),
        }
    }

    pub fn apply_profile(&mut self, p: &VtuberProfile) {
        self.model_path = p.model_path.clone();
        self.tracking_device = p.tracking_device.clone();
        self.tracking_format_fourcc = p.tracking_format_fourcc.clone();
        self.tracking_resolution = p.tracking_resolution;
        self.tracking_framerate = p.tracking_framerate;
        self.display_style = p.display_style;
        self.background_kind = p.background_kind;
        self.background_color = p.background_color;
        self.background_image_path = p.background_image_path.clone();
        self.camera_zoom = p.camera_zoom;
        self.camera_offset_y = p.camera_offset_y;
        self.smoothing = p.smoothing;
        self.blink_sensitivity = p.blink_sensitivity;
        self.mouth_sensitivity = p.mouth_sensitivity;
        self.movement_scale = p.movement_scale;
        self.mirror_tracking = p.mirror_tracking;
        self.mic_lipsync_assist = p.mic_lipsync_assist;
        self.neutral_head_quat = p.neutral_head_quat;
        self.framed_layout = p.framed_layout.clone();
        self.cutout_layout = p.cutout_layout.clone();
    }

    pub fn ensure_profiles(&mut self) {
        if self.active_profile.trim().is_empty() {
            self.active_profile = VTUBER_PROFILE_FRAMED.to_string();
        }
        if self.profiles.is_empty() {
            let current = self.snapshot_profile();
            self.profiles
                .insert(self.active_profile.clone(), current.clone());
            if !self.profiles.contains_key(VTUBER_PROFILE_FRAMED) {
                let mut framed = VtuberProfile::default_framed();
                framed.model_path = current.model_path.clone();
                framed.tracking_device = current.tracking_device.clone();
                framed.tracking_format_fourcc = current.tracking_format_fourcc.clone();
                framed.tracking_resolution = current.tracking_resolution;
                framed.tracking_framerate = current.tracking_framerate;
                self.profiles
                    .insert(VTUBER_PROFILE_FRAMED.to_string(), framed);
            }
            if !self.profiles.contains_key(VTUBER_PROFILE_CUTOUT) {
                let mut cutout = VtuberProfile::default_cutout();
                cutout.model_path = current.model_path;
                cutout.tracking_device = current.tracking_device;
                cutout.tracking_format_fourcc = current.tracking_format_fourcc;
                cutout.tracking_resolution = current.tracking_resolution;
                cutout.tracking_framerate = current.tracking_framerate;
                self.profiles
                    .insert(VTUBER_PROFILE_CUTOUT.to_string(), cutout);
            }
        } else if !self.profiles.contains_key(&self.active_profile) {
            let fallback = if self.profiles.contains_key(VTUBER_PROFILE_FRAMED) {
                VTUBER_PROFILE_FRAMED.to_string()
            } else {
                self.profiles.keys().next().unwrap().clone()
            };
            self.active_profile = fallback.clone();
            if let Some(p) = self.profiles.get(&fallback).cloned() {
                self.apply_profile(&p);
            }
        }
    }

    pub fn sync_active_profile(&mut self) {
        self.ensure_profiles();
        let snap = self.snapshot_profile();
        self.profiles.insert(self.active_profile.clone(), snap);
    }

    pub fn switch_to_profile(&mut self, target_name: &str) -> bool {
        self.ensure_profiles();
        if self.active_profile == target_name {
            return false;
        }
        let Some(target) = self.profiles.get(target_name).cloned() else {
            return false;
        };
        self.sync_active_profile();
        self.active_profile = target_name.to_string();
        self.apply_profile(&target);
        true
    }

    pub fn propagate_initial_model_path(&mut self, path: &str) {
        self.ensure_profiles();
        self.model_path = Some(path.to_string());
        self.sync_active_profile();
        for profile in self.profiles.values_mut() {
            if profile.model_path.is_none() {
                profile.model_path = Some(path.to_string());
            }
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct WebcamConfig {
    pub enabled: bool,
    pub device: Option<String>,
    pub format_fourcc: Option<String>,
    pub resolution: Option<(u32, u32)>,
    pub framerate: Option<u32>,
    pub pos_x_pct: f32,
    pub pos_y_pct: f32,
    pub width_pct: f32,
    #[serde(default)]
    pub source_mode: WebcamSourceMode,
    #[serde(default = "default_local_webcam_profile_name")]
    pub local_active_profile: String,
    // Keep nested tables/maps after scalar fields for confy/TOML serialization.
    pub cutout: WebcamCutout,
    #[serde(default)]
    pub local_profiles: BTreeMap<String, LocalWebcamProfile>,
    #[serde(default)]
    pub vtuber: VtuberConfig,
}

fn default_local_webcam_profile_name() -> String {
    DEFAULT_LOCAL_WEBCAM_PROFILE.to_string()
}

impl Default for WebcamConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            device: None,
            format_fourcc: None,
            resolution: None,
            framerate: None,
            pos_x_pct: 0.72,
            pos_y_pct: 0.65,
            width_pct: 0.24,
            source_mode: WebcamSourceMode::LocalCamera,
            local_active_profile: DEFAULT_LOCAL_WEBCAM_PROFILE.to_string(),
            cutout: WebcamCutout::default(),
            local_profiles: BTreeMap::new(),
            vtuber: VtuberConfig::default(),
        }
    }
}

impl WebcamConfig {
    pub fn snapshot_local_profile(&self) -> LocalWebcamProfile {
        LocalWebcamProfile {
            device: self.device.clone(),
            format_fourcc: self.format_fourcc.clone(),
            resolution: self.resolution,
            framerate: self.framerate,
            pos_x_pct: self.pos_x_pct,
            pos_y_pct: self.pos_y_pct,
            width_pct: self.width_pct,
            cutout: self.cutout.clone(),
        }
    }

    pub fn apply_local_profile(&mut self, p: &LocalWebcamProfile) {
        self.device = p.device.clone();
        self.format_fourcc = p.format_fourcc.clone();
        self.resolution = p.resolution;
        self.framerate = p.framerate;
        self.pos_x_pct = p.pos_x_pct;
        self.pos_y_pct = p.pos_y_pct;
        self.width_pct = p.width_pct;
        self.cutout = p.cutout.clone();
    }

    pub fn ensure_profiles(&mut self) {
        if self.local_active_profile.trim().is_empty() {
            self.local_active_profile = DEFAULT_LOCAL_WEBCAM_PROFILE.to_string();
        }
        if self.local_profiles.is_empty() {
            self.local_profiles.insert(
                self.local_active_profile.clone(),
                self.snapshot_local_profile(),
            );
        } else if !self.local_profiles.contains_key(&self.local_active_profile) {
            let fallback = if self
                .local_profiles
                .contains_key(DEFAULT_LOCAL_WEBCAM_PROFILE)
            {
                DEFAULT_LOCAL_WEBCAM_PROFILE.to_string()
            } else {
                self.local_profiles.keys().next().unwrap().clone()
            };
            self.local_active_profile = fallback.clone();
            if let Some(p) = self.local_profiles.get(&fallback).cloned() {
                self.apply_local_profile(&p);
            }
        }
        self.vtuber.ensure_profiles();
    }

    pub fn sync_active_local_profile(&mut self) {
        self.ensure_profiles();
        let snap = self.snapshot_local_profile();
        self.local_profiles
            .insert(self.local_active_profile.clone(), snap);
    }

    pub fn switch_to_local_profile(&mut self, target_name: &str) -> bool {
        self.ensure_profiles();
        if self.local_active_profile == target_name {
            return false;
        }
        let Some(target) = self.local_profiles.get(target_name).cloned() else {
            return false;
        };
        self.sync_active_local_profile();
        self.local_active_profile = target_name.to_string();
        self.apply_local_profile(&target);
        true
    }

    pub fn synced_for_save(&self) -> Self {
        let mut copy = self.clone();
        copy.sync_active_local_profile();
        copy.vtuber.sync_active_profile();
        copy
    }
}

#[derive(Default, Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Profile {
    pub video_format_fourcc: Option<String>,
    pub crt_filter: Option<u8>,
    pub scaler_filter: Option<u8>,
    pub color_range: Option<u8>,
    pub pixelate_filter_enabled: Option<bool>,

    // Lottes params
    pub crt_hard_scan: Option<f32>,
    pub crt_warp_x: Option<f32>,
    pub crt_warp_y: Option<f32>,
    pub crt_shadow_mask: Option<f32>,
    pub crt_brightboost: Option<f32>,
    pub crt_hard_bloom_pix: Option<f32>,
    pub crt_hard_bloom_scan: Option<f32>,
    pub crt_bloom_amount: Option<f32>,
    pub crt_shape: Option<f32>,
    pub crt_hard_pix: Option<f32>,
    pub use_magenta_background: Option<bool>,
    pub retro_pc_frame: Option<bool>,
    pub retro_pc_frame_dark_mode: Option<bool>,
    pub retro_pc_ambient_glow: Option<f32>,
    pub retro_software_mouse: Option<bool>,
    pub lights_off_night_mode: Option<bool>,
    pub realistic_light_switch: Option<bool>,
    pub night_mode_glow_intensity: Option<f32>,
    pub crt_glass_enabled: Option<bool>,
    pub crt_glass_intensity: Option<f32>,
    pub crt_glass_glossiness: Option<f32>,
    pub crt_glass_ceiling_light_enabled: Option<bool>,
    pub crt_glass_photographer_enabled: Option<bool>,
    pub crt_glass_photographer_intensity: Option<f32>,
    pub crt_glass_flash_enabled: Option<bool>,
    pub crt_glass_flash_intensity: Option<f32>,
    pub horizontal_stretch: Option<f32>,
    pub median_filter_enabled: Option<bool>,
    pub median_mix: Option<f32>,
    pub deinterlace_filter_enabled: Option<bool>,
    pub deinterlace_mode: Option<u8>,
    pub deinterlace_blend: Option<f32>,
    pub deinterlace_motion_threshold: Option<f32>,
    pub deinterlace_line_spacing: Option<f32>,
    pub deinterlace_spatial_mix: Option<f32>,
    pub vibrance: Option<f32>,
    pub brightness: Option<f32>,
    pub gamma: Option<f32>,
    pub overscan_x: Option<f32>,
    pub overscan_y: Option<f32>,
    pub underscan_x: Option<f32>,
    pub underscan_y: Option<f32>,
    pub border_crop_left: Option<f32>,
    pub border_crop_right: Option<f32>,
    pub border_crop_top: Option<f32>,
    pub border_crop_bottom: Option<f32>,

    pub selected_crt_filter: Option<u8>,
    pub active_halo_preset: Option<String>,
    pub active_beam_preset: Option<String>,

    // Halo params
    pub halo_brightboost: Option<f32>,
    pub halo_brightboost1: Option<f32>,
    pub halo_beam_min: Option<f32>,
    pub halo_beam_max: Option<f32>,
    pub halo_beam_size: Option<f32>,
    pub halo_h_sharp: Option<f32>,
    pub halo_glow: Option<f32>,
    pub halo_bloom: Option<f32>,
    pub halo_halation: Option<f32>,
    pub halo_shadow_mask: Option<f32>,
    pub halo_masksize: Option<f32>,
    pub halo_maskstr: Option<f32>,
    pub halo_mcut: Option<f32>,
    pub halo_slotmask: Option<f32>,
    pub halo_slotmask1: Option<f32>,
    pub halo_double_slot: Option<f32>,
    pub halo_smoothmask: Option<f32>,
    pub halo_zoom: Option<f32>,
    pub halo_intensity: Option<f32>,
    pub halo_corner_size: Option<f32>,
    pub halo_curvature: Option<bool>,

    // Beam CRT simulator params (Stage 1; Stage 2 mask params are stored in beam_presets)
    pub beam_frames_per_hz: Option<f32>,
    pub beam_target_hz: Option<u32>,
    pub beam_gamma: Option<f32>,
    pub beam_gain_vs_blur: Option<f32>,
    pub beam_fps_divisor: Option<f32>,
    pub beam_lcd_anti_retention: Option<bool>,
    pub beam_lcd_inversion_compensation_slew: Option<f32>,
    pub beam_scan_direction: Option<u8>,
    pub beam_splitscreen: Option<bool>,
    pub beam_splitscreen_x: Option<f32>,
    pub beam_splitscreen_y: Option<f32>,
    pub beam_splitscreen_border_px: Option<u32>,
    pub beam_splitscreen_match_brightness: Option<bool>,

    // Cathode interference params
    pub cathode_interference_enabled: Option<bool>,
    pub cathode_intensity: Option<f32>,
    pub cathode_frequency: Option<f32>,
    pub cathode_randomization: Option<f32>,
    pub cathode_electricity_glow: Option<f32>,
    pub cathode_flicker_depth: Option<f32>,
    pub cathode_interference: Option<f32>,
    pub cathode_lightbulb_effect: Option<f32>,

    // Video resolution & framerate
    pub video_resolution: Option<(u32, u32)>,
    pub video_framerate: Option<u32>,

    // FFT mask filter
    pub fft_filter_enabled: Option<bool>,
    pub fft_mask_save_name: Option<String>,

    // Audio device settings
    pub audio_source: Option<String>,
    pub audio_buffer_size: Option<u32>,
    pub audio_sample_rate: Option<u32>,
    pub audio_sample_format: Option<String>,
    pub master_volume: Option<f32>,

    pub popup_under_crt: Option<bool>,

    // Audio filter
    pub audio_filter: Option<crate::replay::config::AudioFilterConfig>,
}

pub fn default_pc98_profile() -> Profile {
    Profile {
        crt_filter: Some(2),
        selected_crt_filter: Some(2),
        scaler_filter: Some(crate::video::types::ScalerFilter::Bilinear as u8),
        retro_pc_frame: Some(true),
        retro_pc_ambient_glow: Some(0.55),
        horizontal_stretch: Some(1.0),
        ..Default::default()
    }
}

pub fn default_pc88_profile() -> Profile {
    Profile {
        crt_filter: Some(2),
        selected_crt_filter: Some(2),
        scaler_filter: Some(crate::video::types::ScalerFilter::Bilinear as u8),
        retro_pc_frame: Some(true),
        retro_pc_ambient_glow: Some(0.55),
        horizontal_stretch: Some(1.0),
        ..Default::default()
    }
}

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct LegacyConfig {
    #[serde(default)]
    pub replay: crate::replay::config::ReplayConfig,
    pub video_device: Option<String>,
    pub usb_device: Option<String>,
    pub video_resolution: Option<(u32, u32)>,
    pub video_framerate: Option<u32>,
    #[serde(default)]
    pub video_hwdec: Option<bool>,
    pub reset_usb_on_startup: Option<bool>,
    pub has_shown_first_run_warning: Option<bool>,

    #[serde(default = "default_active_profile")]
    pub active_profile: String,
    #[serde(default = "default_active_halo_preset")]
    pub active_halo_preset: String,
    #[serde(default = "default_active_beam_preset")]
    pub active_beam_preset: String,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub halo_presets: BTreeMap<String, crate::app::models::HaloSettings>,
    #[serde(default)]
    pub beam_presets: BTreeMap<String, crate::app::models::BeamMaskPreset>,

    pub audio_source: Option<String>,
    pub video_format_fourcc: Option<String>,
    pub crt_filter: Option<u8>,
    pub selected_crt_filter: Option<u8>,
    pub scaler_filter: Option<u8>,
    pub color_range: Option<u8>,
    pub pixelate_filter_enabled: Option<bool>,
    pub audio_buffer_size: Option<u32>,
    pub audio_sample_rate: Option<u32>,
    pub audio_sample_format: Option<String>,
    pub master_volume: Option<f32>,
    pub crt_hard_scan: Option<f32>,
    pub crt_warp_x: Option<f32>,
    pub crt_warp_y: Option<f32>,
    pub crt_shadow_mask: Option<f32>,
    pub crt_brightboost: Option<f32>,
    pub crt_hard_bloom_pix: Option<f32>,
    pub crt_hard_bloom_scan: Option<f32>,
    pub crt_bloom_amount: Option<f32>,
    pub crt_shape: Option<f32>,
    pub crt_hard_pix: Option<f32>,
    pub use_magenta_background: Option<bool>,
    pub retro_pc_frame: Option<bool>,
    pub retro_pc_frame_dark_mode: Option<bool>,
    pub retro_pc_ambient_glow: Option<f32>,
    pub retro_software_mouse: Option<bool>,
    pub lights_off_night_mode: Option<bool>,
    pub realistic_light_switch: Option<bool>,
    pub night_mode_glow_intensity: Option<f32>,
    pub crt_glass_enabled: Option<bool>,
    pub crt_glass_intensity: Option<f32>,
    pub crt_glass_glossiness: Option<f32>,
    pub crt_glass_ceiling_light_enabled: Option<bool>,
    pub crt_glass_photographer_enabled: Option<bool>,
    pub crt_glass_photographer_intensity: Option<f32>,
    pub crt_glass_flash_enabled: Option<bool>,
    pub crt_glass_flash_intensity: Option<f32>,
    pub horizontal_stretch: Option<f32>,
    pub median_filter_enabled: Option<bool>,
    pub median_mix: Option<f32>,
    pub deinterlace_filter_enabled: Option<bool>,
    pub deinterlace_mode: Option<u8>,
    pub deinterlace_blend: Option<f32>,
    pub deinterlace_motion_threshold: Option<f32>,
    pub deinterlace_line_spacing: Option<f32>,
    pub deinterlace_spatial_mix: Option<f32>,
    pub vibrance: Option<f32>,
    pub brightness: Option<f32>,
    pub gamma: Option<f32>,
    pub overscan_x: Option<f32>,
    pub overscan_y: Option<f32>,
    pub underscan_x: Option<f32>,
    pub underscan_y: Option<f32>,

    // Halo params
    pub halo_brightboost: Option<f32>,
    pub halo_brightboost1: Option<f32>,
    pub halo_beam_min: Option<f32>,
    pub halo_beam_max: Option<f32>,
    pub halo_beam_size: Option<f32>,
    pub halo_h_sharp: Option<f32>,
    pub halo_glow: Option<f32>,
    pub halo_bloom: Option<f32>,
    pub halo_halation: Option<f32>,
    pub halo_shadow_mask: Option<f32>,
    pub halo_masksize: Option<f32>,
    pub halo_maskstr: Option<f32>,
    pub halo_mcut: Option<f32>,
    pub halo_slotmask: Option<f32>,
    pub halo_slotmask1: Option<f32>,
    pub halo_double_slot: Option<f32>,
    pub halo_smoothmask: Option<f32>,
    pub halo_zoom: Option<f32>,
    pub halo_intensity: Option<f32>,
    pub halo_corner_size: Option<f32>,
    pub halo_curvature: Option<bool>,
    pub ocr_backend: Option<crate::ocr::OcrBackend>,
    pub ocr_sticky_distance: Option<f32>,
    pub ocr_hide_overlay: Option<bool>,
    pub ocr_timeout_seconds: Option<u32>,
    pub ocr_constant_scanning: Option<bool>,
    pub ocr_scan_interval_secs: Option<f32>,
    pub ocr_max_dropoff_secs: Option<f32>,
    pub ocr_jitter_tolerance: Option<f32>,
    pub default_halo: Option<crate::app::models::HaloSettings>,
    pub default_beam: Option<crate::app::models::BeamSettings>,
    pub cathode_interference_enabled: Option<bool>,
    pub cathode_intensity: Option<f32>,
    pub cathode_frequency: Option<f32>,
    pub cathode_randomization: Option<f32>,
    pub cathode_electricity_glow: Option<f32>,
    pub cathode_flicker_depth: Option<f32>,
    pub cathode_interference: Option<f32>,
    pub cathode_lightbulb_effect: Option<f32>,
    pub default_cathode_interference: Option<crate::app::models::CathodeInterferenceSettings>,
    pub bank_current_tag: Option<String>,
    pub bank_compact_mode: Option<bool>,
    pub ui_scale: Option<f32>,
    pub popup_under_crt: Option<bool>,
    #[serde(default)]
    pub retro_pc_system: crate::pc88::RetroPcSystem,
    pub gamescope_launcher_script: Option<String>,
    pub selected_steam_game: Option<String>,
    #[serde(default)]
    pub video_profile: Option<Profile>,
    #[serde(default)]
    pub gamescope_profile: Option<Profile>,
    #[serde(default)]
    pub pc98_profile: Option<Profile>,
    #[serde(default)]
    pub pc88_profile: Option<Profile>,
    #[serde(default)]
    pub pc98: crate::pc98::Pc98Config,
    #[serde(default)]
    pub pc88: crate::pc88::Pc88Config,
    #[serde(default)]
    pub twitch: TwitchConfig,
    #[serde(default)]
    pub mic: MicConfig,
    #[serde(default)]
    pub webcam: WebcamConfig,
    #[serde(default)]
    pub audio_reactive: crate::app::models::AudioReactiveConfig,
}

impl Default for LegacyConfig {
    fn default() -> Self {
        Self {
            replay: Default::default(),
            video_device: None,
            usb_device: None,
            video_resolution: None,
            video_framerate: None,
            video_hwdec: None,
            reset_usb_on_startup: None,
            has_shown_first_run_warning: None,
            active_profile: default_active_profile(),
            active_halo_preset: default_active_halo_preset(),
            active_beam_preset: default_active_beam_preset(),
            profiles: BTreeMap::new(),
            halo_presets: BTreeMap::new(),
            beam_presets: BTreeMap::new(),
            audio_source: None,
            video_format_fourcc: None,
            crt_filter: None,
            selected_crt_filter: None,
            scaler_filter: None,
            color_range: None,
            pixelate_filter_enabled: None,
            audio_buffer_size: None,
            audio_sample_rate: None,
            audio_sample_format: None,
            master_volume: None,
            crt_hard_scan: None,
            crt_warp_x: None,
            crt_warp_y: None,
            crt_shadow_mask: None,
            crt_brightboost: None,
            crt_hard_bloom_pix: None,
            crt_hard_bloom_scan: None,
            crt_bloom_amount: None,
            crt_shape: None,
            crt_hard_pix: None,
            use_magenta_background: None,
            retro_pc_frame: None,
            retro_pc_frame_dark_mode: None,
            retro_pc_ambient_glow: None,
            retro_software_mouse: None,
            lights_off_night_mode: None,
            realistic_light_switch: None,
            night_mode_glow_intensity: None,
            crt_glass_enabled: None,
            crt_glass_intensity: None,
            crt_glass_glossiness: None,
            crt_glass_ceiling_light_enabled: None,
            crt_glass_photographer_enabled: None,
            crt_glass_photographer_intensity: None,
            crt_glass_flash_enabled: None,
            crt_glass_flash_intensity: None,
            horizontal_stretch: None,
            median_filter_enabled: None,
            median_mix: None,
            deinterlace_filter_enabled: None,
            deinterlace_mode: None,
            deinterlace_blend: None,
            deinterlace_motion_threshold: None,
            deinterlace_line_spacing: None,
            deinterlace_spatial_mix: None,
            vibrance: None,
            brightness: None,
            gamma: None,
            overscan_x: None,
            overscan_y: None,
            underscan_x: None,
            underscan_y: None,
            halo_brightboost: None,
            halo_brightboost1: None,
            halo_beam_min: None,
            halo_beam_max: None,
            halo_beam_size: None,
            halo_h_sharp: None,
            halo_glow: None,
            halo_bloom: None,
            halo_halation: None,
            halo_shadow_mask: None,
            halo_masksize: None,
            halo_maskstr: None,
            halo_mcut: None,
            halo_slotmask: None,
            halo_slotmask1: None,
            halo_double_slot: None,
            halo_smoothmask: None,
            halo_zoom: None,
            halo_intensity: None,
            halo_corner_size: None,
            halo_curvature: None,
            ocr_backend: None,
            ocr_sticky_distance: None,
            ocr_hide_overlay: None,
            ocr_timeout_seconds: None,
            ocr_constant_scanning: None,
            ocr_scan_interval_secs: None,
            ocr_max_dropoff_secs: None,
            ocr_jitter_tolerance: None,
            default_halo: None,
            default_beam: None,
            cathode_interference_enabled: None,
            cathode_intensity: None,
            cathode_frequency: None,
            cathode_randomization: None,
            cathode_electricity_glow: None,
            cathode_flicker_depth: None,
            cathode_interference: None,
            cathode_lightbulb_effect: None,
            default_cathode_interference: None,
            bank_current_tag: None,
            bank_compact_mode: None,
            ui_scale: None,
            popup_under_crt: None,
            retro_pc_system: crate::pc88::RetroPcSystem::default(),
            gamescope_launcher_script: None,
            selected_steam_game: None,
            video_profile: None,
            gamescope_profile: None,
            pc98_profile: None,
            pc88_profile: None,
            pc98: crate::pc98::Pc98Config::default(),
            pc88: crate::pc88::Pc88Config::default(),
            twitch: TwitchConfig::default(),
            mic: MicConfig::default(),
            webcam: WebcamConfig::default(),
            audio_reactive: crate::app::models::AudioReactiveConfig::default(),
        }
    }
}

pub fn default_active_profile() -> String {
    "Default".to_string()
}

pub fn default_active_halo_preset() -> String {
    crate::app::models::HALO_DEFAULT_PRESET.to_string()
}

pub fn default_halo_presets() -> BTreeMap<String, crate::app::models::HaloSettings> {
    crate::app::models::HaloSettings::builtin_presets()
}

pub fn default_active_beam_preset() -> String {
    crate::app::models::BEAM_DEFAULT_PRESET.to_string()
}

pub fn default_beam_presets() -> BTreeMap<String, crate::app::models::BeamMaskPreset> {
    crate::app::models::BeamMaskPreset::builtin_presets()
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(from = "LegacyConfig")]
pub struct MichadameConfig {
    pub video_device: Option<String>,
    pub usb_device: Option<String>,
    pub video_resolution: Option<(u32, u32)>,
    pub video_framerate: Option<u32>,
    #[serde(default)]
    pub video_hwdec: Option<bool>,
    pub reset_usb_on_startup: Option<bool>,
    pub has_shown_first_run_warning: Option<bool>,
    pub audio_source: Option<String>,
    pub audio_buffer_size: Option<u32>,
    pub audio_sample_rate: Option<u32>,
    pub audio_sample_format: Option<String>,
    pub master_volume: Option<f32>,
    pub active_profile: String,
    #[serde(default = "default_active_halo_preset")]
    pub active_halo_preset: String,
    #[serde(default = "default_active_beam_preset")]
    pub active_beam_preset: String,
    pub ocr_backend: Option<crate::ocr::OcrBackend>,
    pub ocr_sticky_distance: Option<f32>,
    pub ocr_hide_overlay: Option<bool>,
    pub ocr_timeout_seconds: Option<u32>,
    pub ocr_constant_scanning: Option<bool>,
    pub ocr_scan_interval_secs: Option<f32>,
    pub ocr_max_dropoff_secs: Option<f32>,
    pub ocr_jitter_tolerance: Option<f32>,
    /// Tag applied to newly mined words in the mining bank.
    pub bank_current_tag: Option<String>,
    /// Whether the mining bank window starts in compact mode.
    pub bank_compact_mode: Option<bool>,
    /// Manual desktop UI scale override; `None` scales automatically by monitor resolution.
    pub ui_scale: Option<f32>,
    pub popup_under_crt: Option<bool>,
    #[serde(default)]
    pub retro_pc_system: crate::pc88::RetroPcSystem,
    pub gamescope_launcher_script: Option<String>,
    pub selected_steam_game: Option<String>,
    pub default_halo: Option<crate::app::models::HaloSettings>,
    pub default_beam: Option<crate::app::models::BeamSettings>,
    pub default_cathode_interference: Option<crate::app::models::CathodeInterferenceSettings>,
    pub twitch: TwitchConfig,
    pub mic: MicConfig,
    pub webcam: WebcamConfig,
    #[serde(default)]
    pub pc98: crate::pc98::Pc98Config,
    #[serde(default)]
    pub pc88: crate::pc88::Pc88Config,
    // confy's TOML serializer requires scalar fields before nested tables.
    pub replay: crate::replay::config::ReplayConfig,
    #[serde(default)]
    pub video_profile: Option<Profile>,
    #[serde(default)]
    pub gamescope_profile: Option<Profile>,
    #[serde(default)]
    pub pc98_profile: Option<Profile>,
    #[serde(default)]
    pub pc88_profile: Option<Profile>,
    #[serde(default)]
    pub audio_reactive: crate::app::models::AudioReactiveConfig,
    #[serde(default = "default_halo_presets")]
    pub halo_presets: BTreeMap<String, crate::app::models::HaloSettings>,
    #[serde(default = "default_beam_presets")]
    pub beam_presets: BTreeMap<String, crate::app::models::BeamMaskPreset>,
    pub profiles: BTreeMap<String, Profile>,
}

impl Default for MichadameConfig {
    fn default() -> Self {
        let mut profiles = BTreeMap::new();
        profiles.insert("Default".to_string(), Profile::default());
        Self {
            replay: Default::default(),
            video_device: None,
            usb_device: None,
            video_resolution: None,
            video_framerate: None,
            video_hwdec: None,
            reset_usb_on_startup: None,
            has_shown_first_run_warning: None,
            audio_source: None,
            audio_buffer_size: None,
            audio_sample_rate: None,
            audio_sample_format: None,
            master_volume: None,
            active_profile: "Default".to_string(),
            active_halo_preset: default_active_halo_preset(),
            active_beam_preset: default_active_beam_preset(),
            ocr_backend: Some(crate::ocr::OcrBackend::default()),
            ocr_sticky_distance: Some(0.6),
            ocr_hide_overlay: Some(false),
            ocr_timeout_seconds: Some(45),
            ocr_constant_scanning: Some(false),
            ocr_scan_interval_secs: Some(crate::ocr::models::DEFAULT_SCAN_INTERVAL_SECS),
            ocr_max_dropoff_secs: Some(crate::ocr::models::DEFAULT_MAX_DROPOFF_SECS),
            ocr_jitter_tolerance: Some(crate::ocr::models::DEFAULT_JITTER_TOLERANCE),
            default_halo: None,
            default_beam: None,
            default_cathode_interference: None,
            twitch: TwitchConfig::default(),
            mic: MicConfig::default(),
            webcam: WebcamConfig::default(),
            pc98: crate::pc98::Pc98Config::default(),
            pc88: crate::pc88::Pc88Config::default(),
            bank_current_tag: None,
            bank_compact_mode: None,
            ui_scale: None,
            popup_under_crt: Some(false),
            retro_pc_system: crate::pc88::RetroPcSystem::default(),
            gamescope_launcher_script: None,
            selected_steam_game: None,
            video_profile: None,
            gamescope_profile: None,
            pc98_profile: None,
            pc88_profile: None,
            audio_reactive: crate::app::models::AudioReactiveConfig::default(),
            halo_presets: default_halo_presets(),
            beam_presets: default_beam_presets(),
            profiles,
        }
    }
}
