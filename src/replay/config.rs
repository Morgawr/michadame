use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplayConfig {
    pub history_seconds: u32,
    pub memory_mib: u32,
    pub quality: u32,
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
impl Codec {
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
            quality: 20,
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
            (1..=3600).contains(&self.history_seconds),
            "History must be 1–3600 seconds"
        );
        anyhow::ensure!(
            (256..=32768).contains(&self.memory_mib),
            "RAM budget must be 256–32768 MiB"
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
    pub fn tick(self, us: i64) -> i64 {
        ((us as i128 * self.num as i128 + 500_000 * self.den as i128)
            / (1_000_000 * self.den as i128)) as i64
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
/// Conservative CPU staging/codec allowance, separate from driver-owned VRAM.
pub fn overhead(width: u32, height: u32) -> Option<usize> {
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4 * 10)?
        .checked_add(64 * 1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_rate_does_not_accumulate_drift() {
        let r = Rate::new(60000, 1001);
        assert_eq!(r.us(36000), 600_600_000);
        assert_eq!(r.tick(r.us(36000)), 36000);
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
