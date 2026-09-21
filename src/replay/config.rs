use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplayConfig {
    pub history_seconds: u32,
    pub memory_mib: u32,
    /// Raw GPU/CPU work buffers, within the total RAM budget (capped at half).
    pub work_queue_mib: u32,
    pub quality: u32,
    pub rate_control: RateControl,
    pub max_bitrate_mbps: u32,
    pub codec: Codec,
    pub render_device: String,
    pub directory: String,
    pub custom_seconds: u32,
    /// Function key numbers; zero disables a binding.
    pub keys: [u8; 6],
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Codec {
    #[default]
    Av1,
    Hevc,
    H264,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RateControl {
    #[default]
    Bitrate,
    Quality,
}
impl Codec {
    /// A normalized UI scale, not a promise of equivalent quality across codecs.
    /// AV1 uses q_idx 0..255; H.264/HEVC use QP 0..51.
    pub fn quantizer(self, quality: u32) -> u32 {
        quality.clamp(1, 51) * if self == Self::Av1 { 5 } else { 1 }
    }
    pub fn encoder(self) -> &'static str {
        match self {
            Self::Av1 => "av1_vaapi",
            Self::Hevc => "hevc_vaapi",
            Self::H264 => "h264_vaapi",
        }
    }
}
impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            history_seconds: 300,
            memory_mib: 1024,
            work_queue_mib: 512,
            quality: 20,
            rate_control: RateControl::Bitrate,
            max_bitrate_mbps: 40,
            codec: Codec::Av1,
            render_device: "/dev/dri/renderD128".into(),
            directory: std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| ".".into())
                .join("Videos/Michadame")
                .to_string_lossy()
                .into_owned(),
            custom_seconds: 120,
            keys: [5, 6, 7, 8, 9, 10],
        }
    }
}
impl ReplayConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=200).contains(&self.max_bitrate_mbps),
            "Video bitrate limit must be 1–200 Mbit/s"
        );
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
        anyhow::ensure!(
            (1..=51).contains(&self.quality),
            "Quality must be 1–51 (lower is better)"
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
    fn normalized_quality_uses_each_codecs_native_quantizer_range() {
        assert_eq!(Codec::Av1.quantizer(20), 100);
        assert_eq!(Codec::Av1.quantizer(51), 255);
        for codec in [Codec::Hevc, Codec::H264] {
            assert_eq!(codec.quantizer(20), 20);
            assert_eq!(codec.quantizer(51), 51);
        }
        let mut config = ReplayConfig::default();
        for bitrate in [0, 201, u32::MAX] {
            config.max_bitrate_mbps = bitrate;
            assert!(config.validate().is_err());
        }
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
}
