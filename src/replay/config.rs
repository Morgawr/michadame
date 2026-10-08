use serde::{Deserialize, Serialize};

pub const VIDEO_ENCODER: &str = "h264_vaapi";
pub const VIDEO_BITRATE: u64 = 8_000_000;
pub const VIDEO_MAXRATE: u64 = 10_000_000;
pub const VIDEO_BUFSIZE: u64 = 20_000_000;
pub const OUTPUT_FPS: u32 = 60;
pub const AUDIO_BITRATE: usize = 160_000;
pub const AUDIO_FRAME_SIZE: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DenoiseBackend {
    #[default]
    Afftdn,       // Realtime adaptive FFT spectral tracking (lapped windows, ideal for game audio)
    Rnnoise,      // Realtime neural network noise suppression (nnnoiseless)
    Disabled,     // No spectral denoiser (only notch filters / gate)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum HumFilterMode {
    #[default]
    SingleNotch60Hz, // Single notch at 60.0 Hz (NTSC console / 60 Hz mains)
    SingleNotch50Hz, // Single notch at 50.0 Hz (PAL console / 50 Hz mains)
    Custom,          // Single notch at custom frequency
    Disabled,        // No hum notch
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FilterPreset {
    #[default]
    DreamcastNtsc,
    ConsolePal,
    MainsHum60,
    VoiceChat,
    Custom,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioFilterConfig {
    pub enabled: bool,
    pub preset: FilterPreset,
    pub denoise_backend: DenoiseBackend,
    pub denoise_reduction_db: f32, // for afftdn (e.g. 25.0)
    pub hum_filter: HumFilterMode,
    pub hum_freq: f32,             // for Custom hum notch (default 60.0)
    pub crt_notch: bool,           // 15.734 kHz flyback notch
    pub crt_freq: f32,             // default 15734.26
    pub rumble_filter: bool,       // 40 Hz highpass
    pub treble_bypass: bool,       // Retain high frequencies above crossover (prevents muffled/underwater sound)
    pub treble_crossover_hz: f32,  // Cutoff frequency for treble bypass (default 4000.0)
    pub gate: bool,                // downward expander
    pub gate_threshold_db: f32,    // default -50.0
}

impl Default for AudioFilterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            preset: FilterPreset::DreamcastNtsc,
            denoise_backend: DenoiseBackend::Afftdn,
            denoise_reduction_db: 25.0,
            hum_filter: HumFilterMode::SingleNotch60Hz,
            hum_freq: 60.0,
            crt_notch: true,
            crt_freq: 15734.26,
            rumble_filter: true,
            treble_bypass: true,
            treble_crossover_hz: 4000.0,
            gate: true,
            gate_threshold_db: -50.0,
        }
    }
}

impl AudioFilterConfig {
    pub fn apply_preset(&mut self, preset: FilterPreset) {
        self.preset = preset;
        match preset {
            FilterPreset::DreamcastNtsc => {
                self.denoise_backend = DenoiseBackend::Afftdn;
                self.hum_filter = HumFilterMode::SingleNotch60Hz;
                self.crt_notch = true;
                self.crt_freq = 15734.26;
                self.rumble_filter = true;
                self.treble_bypass = true;
                self.treble_crossover_hz = 4000.0;
                self.gate = true;
            }
            FilterPreset::ConsolePal => {
                self.denoise_backend = DenoiseBackend::Afftdn;
                self.hum_filter = HumFilterMode::SingleNotch50Hz;
                self.crt_notch = true;
                self.crt_freq = 15625.0;
                self.rumble_filter = true;
                self.treble_bypass = true;
                self.treble_crossover_hz = 4000.0;
                self.gate = true;
            }
            FilterPreset::MainsHum60 => {
                self.denoise_backend = DenoiseBackend::Disabled;
                self.hum_filter = HumFilterMode::SingleNotch60Hz;
                self.crt_notch = false;
                self.rumble_filter = true;
                self.treble_bypass = false;
                self.gate = false;
            }
            FilterPreset::VoiceChat => {
                self.denoise_backend = DenoiseBackend::Rnnoise;
                self.hum_filter = HumFilterMode::Disabled;
                self.crt_notch = false;
                self.rumble_filter = true;
                self.treble_bypass = false;
                self.gate = true;
            }
            FilterPreset::Custom => {}
        }
    }

    pub fn build_filter_spec(&self) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let mut stages = Vec::new();
        if self.rumble_filter {
            stages.push("40 Hz HPF".to_string());
        }
        match self.hum_filter {
            HumFilterMode::SingleNotch60Hz => stages.push("60 Hz Single Notch".to_string()),
            HumFilterMode::SingleNotch50Hz => stages.push("50 Hz Single Notch".to_string()),
            HumFilterMode::Custom => stages.push(format!("{:.1} Hz Single Notch", self.hum_freq)),
            HumFilterMode::Disabled => {}
        }
        if self.crt_notch {
            stages.push(format!("{:.0} Hz CRT Notch", self.crt_freq));
        }
        match self.denoise_backend {
            DenoiseBackend::Afftdn => {
                if self.treble_bypass {
                    stages.push(format!("FFmpeg afftdn (-{:.0}dB, <{:.0}Hz)", self.denoise_reduction_db, self.treble_crossover_hz));
                } else {
                    stages.push(format!("FFmpeg afftdn (-{:.0}dB)", self.denoise_reduction_db));
                }
            }
            DenoiseBackend::Rnnoise => {
                if self.treble_bypass {
                    stages.push(format!("RNNoise (<{:.0}Hz)", self.treble_crossover_hz));
                } else {
                    stages.push("RNNoise (nnnoiseless)".to_string());
                }
            }
            DenoiseBackend::Disabled => {}
        }
        if self.treble_bypass && self.denoise_backend != DenoiseBackend::Disabled {
            stages.push(format!("Treble Bypass (>{:.0}Hz Retained)", self.treble_crossover_hz));
        }
        if self.gate {
            stages.push(format!("Gate ({:.0} dBFS)", self.gate_threshold_db));
        }
        if stages.is_empty() {
            Some("Bypass".to_string())
        } else {
            Some(stages.join(" + "))
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplayConfig {
    pub history_seconds: u32,
    pub memory_mib: u32,
    /// Raw GPU/CPU work buffers, within the total RAM budget (capped at half).
    pub work_queue_mib: u32,
    pub render_device: String,
    pub directory: String,
    pub custom_seconds: u32,
    /// Function key numbers; zero disables a binding.
    pub keys: [u8; 6],
    pub capture_overlays: bool,
    pub audio_filter: AudioFilterConfig,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            history_seconds: 300,
            memory_mib: 1024,
            work_queue_mib: 512,
            render_device: "/dev/dri/renderD128".into(),
            directory: std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| ".".into())
                .join("Videos/Michadame")
                .to_string_lossy()
                .into_owned(),
            custom_seconds: 120,
            keys: [5, 6, 7, 8, 9, 10],
            capture_overlays: false,
            audio_filter: AudioFilterConfig::default(),
        }
    }
}
impl ReplayConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=3600).contains(&self.history_seconds),
            "History must be 1–3600 seconds"
        );
        anyhow::ensure!(
            (256..=32768).contains(&self.memory_mib),
            "RAM budget must be 256–32768 MiB"
        );
        anyhow::ensure!(
            (32..=8192).contains(&self.work_queue_mib),
            "Work queue budget must be 32–8192 MiB"
        );
        anyhow::ensure!(!self.directory.trim().is_empty(), "Choose a replay folder");
        anyhow::ensure!(self.keys.iter().all(|k| *k <= 12), "Use F1–F12 or Disabled");
        for (i, key) in self.keys.iter().enumerate() {
            anyhow::ensure!(
                *key == 0 || !self.keys[..i].contains(key),
                "Replay shortcuts must be different"
            );
        }
        Ok(())
    }
    pub fn durations(&self) -> [u32; 6] {
        [30, 60, 180, 300, 600, self.custom_seconds.clamp(1, 3600)]
    }
    pub fn budget(&self) -> usize {
        self.memory_mib as usize * 1024 * 1024
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    pub num: u32,
    pub den: u32,
}
impl Rate {
    pub fn new(num: u32, den: u32) -> Self {
        Self {
            num: num.max(1),
            den: den.max(1),
        }
    }
    pub fn us(self, tick: i64) -> i64 {
        (tick as i128 * 1_000_000 * self.den as i128 / self.num as i128) as i64
    }
}

pub fn available_memory() -> Option<usize> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_available(&text)
}
fn parse_available(text: &str) -> Option<usize> {
    text.lines().find_map(|line| {
        line.strip_prefix("MemAvailable:")?
            .split_whitespace()
            .next()?
            .parse::<usize>()
            .ok()?
            .checked_mul(1024)
    })
}
pub const SAFETY_RESERVE: usize = 512 * 1024 * 1024;
pub const MAX_WORK_FRAMES: usize = 120;
#[derive(Clone, Copy, Debug)]
pub struct QueuePlan {
    pub frame_bytes: usize,
    pub slots: usize,
    pub queue_bytes: usize,
    pub overhead: usize,
}
impl ReplayConfig {
    pub fn queue_plan(&self, width: u32, height: u32) -> Option<QueuePlan> {
        let frame_bytes = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        if frame_bytes == 0 || frame_bytes > 256 * 1024 * 1024 {
            return None;
        }
        let allowance = (self.work_queue_mib as usize * 1024 * 1024).min(self.budget() / 2);
        // Equal bounded pools for pending GPU transfers and reusable CPU frames.
        // The CPU pool includes the frame currently being encoded.
        let slots = (allowance / (2 * frame_bytes)).min(MAX_WORK_FRAMES);
        if slots < 2 {
            return None;
        }
        let queue_bytes = slots * 2 * frame_bytes;
        let overhead = queue_bytes
            .checked_add(frame_bytes.checked_mul(4)?)?
            .checked_add(64 * 1024 * 1024)?;
        (overhead < self.budget()).then_some(QueuePlan {
            frame_bytes,
            slots,
            queue_bytes,
            overhead,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_config_validates() {
        let config = ReplayConfig::default();
        assert!(config.validate().is_ok());
    }
    #[test]
    fn fractional_rate_does_not_accumulate_drift() {
        let r = Rate::new(60000, 1001);
        assert_eq!(r.us(36000), 600_600_000);
        assert_eq!(r.us(36001) - r.us(36000), 16_683);
    }
    #[test]
    fn duplicate_keys_are_rejected_but_disabled_keys_are_allowed() {
        let mut c = ReplayConfig {
            keys: [5; 6],
            ..Default::default()
        };
        assert!(c.validate().is_err());
        c.keys = [0; 6];
        assert!(c.validate().is_ok());
    }
    #[test]
    fn memory_uses_available_not_free_or_swap() {
        assert_eq!(
            parse_available("MemFree: 12 kB\nMemAvailable: 2048 kB\nSwapFree: 999999 kB"),
            Some(2097152)
        );
        assert_eq!(parse_available("MemFree: 12 kB"), None);
    }
    #[test]
    fn audio_filter_presets() {
        let mut filter = AudioFilterConfig::default();
        assert_eq!(filter.preset, FilterPreset::DreamcastNtsc);
        assert_eq!(filter.hum_filter, HumFilterMode::SingleNotch60Hz);
        assert_eq!(filter.denoise_backend, DenoiseBackend::Afftdn);
        assert!(filter.crt_notch);

        filter.apply_preset(FilterPreset::ConsolePal);
        assert_eq!(filter.hum_filter, HumFilterMode::SingleNotch50Hz);
        assert_eq!(filter.crt_freq, 15625.0);

        filter.apply_preset(FilterPreset::MainsHum60);
        assert_eq!(filter.hum_filter, HumFilterMode::SingleNotch60Hz);
        assert!(!filter.crt_notch);
        assert_eq!(filter.denoise_backend, DenoiseBackend::Disabled);

        filter.apply_preset(FilterPreset::VoiceChat);
        assert_eq!(filter.denoise_backend, DenoiseBackend::Rnnoise);
        assert_eq!(filter.hum_filter, HumFilterMode::Disabled);
    }
}
