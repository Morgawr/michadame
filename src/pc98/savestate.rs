use crate::pc88::Pc88Config;
use crate::pc98::Pc98Config;
use crate::video::types::RawFrame;
use anyhow::{Context, Result};
use image::imageops::FilterType;
use image::{ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Returns the current Unix epoch timestamp in seconds.
pub fn current_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Number of save-state slots per game (slots `1..=8`).
pub const SAVE_SLOT_COUNT: usize = 8;

/// Standard thumbnail width for save-state preview cards (16:10 CRT aspect).
pub const THUMBNAIL_WIDTH: u32 = 320;
/// Standard thumbnail height for save-state preview cards (16:10 CRT aspect).
pub const THUMBNAIL_HEIGHT: u32 = 200;

/// Metadata persisted alongside each Libretro save-state slot (`slot_{n}.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveStateMeta {
    pub slot: u8,
    pub created_at: i64,
    pub system: String,
    pub game_key: String,
    pub game_title: String,
    #[serde(default)]
    pub fdd1_path: String,
    #[serde(default)]
    pub fdd2_path: String,
    #[serde(default)]
    pub hdd1_path: String,
    #[serde(default)]
    pub tape_path: String,
    pub resolution: (u32, u32),
    pub size_bytes: usize,
}

impl SaveStateMeta {
    /// Formats `created_at` (Unix epoch seconds) into local `YYYY-MM-DD HH:MM:SS`.
    pub fn formatted_timestamp(&self) -> String {
        if self.created_at <= 0 {
            return "Unknown time".to_string();
        }
        unsafe {
            let t = self.created_at as libc::time_t;
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&t, &mut tm).is_null() {
                return "Unknown time".to_string();
            }
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                tm.tm_year + 1900,
                tm.tm_mon + 1,
                tm.tm_mday,
                tm.tm_hour,
                tm.tm_min,
                tm.tm_sec
            )
        }
    }

    /// Formats `size_bytes` into a compact human-readable string (`KB` / `MB`).
    pub fn formatted_size(&self) -> String {
        if self.size_bytes >= 1024 * 1024 {
            format!("{:.2} MB", self.size_bytes as f64 / (1024.0 * 1024.0))
        } else if self.size_bytes >= 1024 {
            format!("{} KB", (self.size_bytes + 512) / 1024)
        } else {
            format!("{} B", self.size_bytes)
        }
    }

    /// Returns a short label of the primary mounted media when the state was saved.
    pub fn media_label(&self) -> Option<String> {
        for candidate in [
            &self.fdd1_path,
            &self.tape_path,
            &self.hdd1_path,
            &self.fdd2_path,
        ] {
            let trimmed = candidate.trim();
            if !trimmed.is_empty() {
                if let Some(name) = Path::new(trimmed).file_name().and_then(|s| s.to_str()) {
                    return Some(name.to_string());
                }
            }
        }
        None
    }
}

/// Root directory for PC-98 save states (`~/.local/share/michadame/pc98/states`).
pub fn pc98_states_root() -> PathBuf {
    super::core_sync::pc98_base_dir().join("states")
}

/// Root directory for PC-88 save states (`~/.local/share/michadame/pc88/states`).
pub fn pc88_states_root() -> PathBuf {
    crate::pc88::core_sync::pc88_base_dir().join("states")
}

/// Sanitizes an arbitrary game identifier or file stem into a safe directory name.
pub fn sanitize_game_key(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_underscore = false;

    for ch in raw.trim().chars() {
        if ch.is_alphanumeric() || ch == '-' || ch == '.' {
            out.push(ch);
            last_underscore = false;
        } else if !last_underscore && !out.is_empty() {
            out.push('_');
            last_underscore = true;
        }
        if out.chars().count() >= 80 {
            break;
        }
    }

    let cleaned = out.trim_matches(|c: char| c == '_' || c == '.').to_string();
    if cleaned.is_empty() {
        "default_system".to_string()
    } else {
        cleaned
    }
}

/// Derives `(game_key, game_title)` from a `Pc98Config`.
pub fn derive_pc98_game_key(config: &Pc98Config) -> (String, String) {
    let selected = config.selected_game.trim();
    if !selected.is_empty() {
        let title = Path::new(selected)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(selected)
            .to_string();
        return (sanitize_game_key(selected), title);
    }

    for media_path in [
        config.fdd1_path.trim(),
        config.hdd1_path.trim(),
        config.cdrom_path.trim(),
        config.fdd2_path.trim(),
    ] {
        if !media_path.is_empty() {
            let p = Path::new(media_path);
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                let trimmed_stem = stem.trim();
                if !trimmed_stem.is_empty() {
                    return (sanitize_game_key(trimmed_stem), trimmed_stem.to_string());
                }
            }
        }
    }

    ("pc98_system".to_string(), "PC-98 System".to_string())
}

/// Derives `(game_key, game_title)` from a `Pc88Config`.
pub fn derive_pc88_game_key(config: &Pc88Config) -> (String, String) {
    let selected = config.selected_game.trim();
    if !selected.is_empty() {
        let title = Path::new(selected)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(selected)
            .to_string();
        return (sanitize_game_key(selected), title);
    }

    for media_path in [
        config.fdd1_path.trim(),
        config.tape_path.trim(),
        config.fdd2_path.trim(),
    ] {
        if !media_path.is_empty() {
            let p = Path::new(media_path);
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                let trimmed_stem = stem.trim();
                if !trimmed_stem.is_empty() {
                    return (sanitize_game_key(trimmed_stem), trimmed_stem.to_string());
                }
            }
        }
    }

    ("pc88_system".to_string(), "PC-88 System".to_string())
}

pub fn game_states_dir(states_root: &Path, game_key: &str) -> PathBuf {
    states_root.join(sanitize_game_key(game_key))
}

pub fn slot_state_path(states_root: &Path, game_key: &str, slot: u8) -> PathBuf {
    game_states_dir(states_root, game_key).join(format!("slot_{}.state", slot))
}

pub fn slot_meta_path(states_root: &Path, game_key: &str, slot: u8) -> PathBuf {
    game_states_dir(states_root, game_key).join(format!("slot_{}.json", slot))
}

pub fn slot_thumb_path(states_root: &Path, game_key: &str, slot: u8) -> PathBuf {
    game_states_dir(states_root, game_key).join(format!("slot_{}.jpg", slot))
}

/// Atomically writes `slot_{n}.state`, `slot_{n}.json`, and (if `frame` is provided) `slot_{n}.jpg`.
pub fn write_slot_files(
    states_root: &Path,
    game_key: &str,
    meta: &SaveStateMeta,
    state_bytes: &[u8],
    frame: Option<&RawFrame>,
) -> Result<()> {
    let slot = meta.slot.clamp(1, SAVE_SLOT_COUNT as u8);
    let dir = game_states_dir(states_root, game_key);
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create save-state directory {:?}", dir))?;

    let state_path = slot_state_path(states_root, game_key, slot);
    let tmp_state_path = state_path.with_extension("state.tmp");
    fs::write(&tmp_state_path, state_bytes)
        .with_context(|| format!("Failed to write state data to {:?}", tmp_state_path))?;
    fs::rename(&tmp_state_path, &state_path)
        .with_context(|| format!("Failed to finalize state file {:?}", state_path))?;

    if let Some(raw) = frame {
        if let Ok(jpeg_bytes) = encode_frame_thumbnail_jpeg(raw) {
            let thumb_path = slot_thumb_path(states_root, game_key, slot);
            let tmp_thumb = thumb_path.with_extension("jpg.tmp");
            if fs::write(&tmp_thumb, &jpeg_bytes).is_ok() {
                let _ = fs::rename(&tmp_thumb, &thumb_path);
            }
        }
    }

    let meta_path = slot_meta_path(states_root, game_key, slot);
    let tmp_meta_path = meta_path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(meta)
        .context("Failed to serialize save-state metadata JSON")?;
    fs::write(&tmp_meta_path, json)
        .with_context(|| format!("Failed to write metadata to {:?}", tmp_meta_path))?;
    fs::rename(&tmp_meta_path, &meta_path)
        .with_context(|| format!("Failed to finalize metadata file {:?}", meta_path))?;

    Ok(())
}

/// Downscales an emulator `RawFrame` (RGBA8) to `320x200` and encodes it as a JPEG thumbnail.
pub fn encode_frame_thumbnail_jpeg(raw: &RawFrame) -> Result<Vec<u8>> {
    if raw.width == 0 || raw.height == 0 {
        anyhow::bail!("Empty frame dimensions");
    }
    let expected_len = (raw.width as usize)
        .saturating_mul(raw.height as usize)
        .saturating_mul(4);
    if raw.data.len() < expected_len {
        anyhow::bail!("RawFrame data buffer is smaller than width*height*4");
    }

    let img = RgbaImage::from_raw(raw.width, raw.height, raw.data[..expected_len].to_vec())
        .context("Failed to construct RgbaImage from RawFrame")?;
    let resized = image::imageops::resize(
        &img,
        THUMBNAIL_WIDTH,
        THUMBNAIL_HEIGHT,
        FilterType::Triangle,
    );
    let rgb = image::DynamicImage::ImageRgba8(resized).into_rgb8();

    let mut buf = Vec::with_capacity(24 * 1024);
    rgb.write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)
        .context("Failed to encode save-state thumbnail as JPEG")?;
    Ok(buf)
}

/// Reads the serialized Libretro state bytes and metadata for `slot` (`1..=8`).
pub fn read_slot_state(
    states_root: &Path,
    game_key: &str,
    slot: u8,
) -> Result<(SaveStateMeta, Vec<u8>)> {
    let slot = slot.clamp(1, SAVE_SLOT_COUNT as u8);
    let state_path = slot_state_path(states_root, game_key, slot);
    if !state_path.is_file() {
        anyhow::bail!("Slot {} is empty (no save state found)", slot);
    }

    let state_bytes = fs::read(&state_path)
        .with_context(|| format!("Failed to read save state {:?}", state_path))?;

    let meta_path = slot_meta_path(states_root, game_key, slot);
    let meta = if let Ok(json_str) = fs::read_to_string(&meta_path) {
        serde_json::from_str::<SaveStateMeta>(&json_str).unwrap_or_else(|_| SaveStateMeta {
            slot,
            created_at: 0,
            system: "PC".to_string(),
            game_key: game_key.to_string(),
            game_title: game_key.to_string(),
            fdd1_path: String::new(),
            fdd2_path: String::new(),
            hdd1_path: String::new(),
            tape_path: String::new(),
            resolution: (640, 400),
            size_bytes: state_bytes.len(),
        })
    } else {
        SaveStateMeta {
            slot,
            created_at: 0,
            system: "PC".to_string(),
            game_key: game_key.to_string(),
            game_title: game_key.to_string(),
            fdd1_path: String::new(),
            fdd2_path: String::new(),
            hdd1_path: String::new(),
            tape_path: String::new(),
            resolution: (640, 400),
            size_bytes: state_bytes.len(),
        }
    };

    Ok((meta, state_bytes))
}

/// Returns metadata for all 8 save slots (`index = (slot - 1) as usize`) of `game_key`.
pub fn list_game_slots(
    states_root: &Path,
    game_key: &str,
) -> [Option<SaveStateMeta>; SAVE_SLOT_COUNT] {
    let mut slots: [Option<SaveStateMeta>; SAVE_SLOT_COUNT] = Default::default();
    for idx in 0..SAVE_SLOT_COUNT {
        let slot = (idx + 1) as u8;
        let state_path = slot_state_path(states_root, game_key, slot);
        if !state_path.is_file() {
            continue;
        }
        let meta_path = slot_meta_path(states_root, game_key, slot);
        if let Ok(json_str) = fs::read_to_string(&meta_path) {
            if let Ok(meta) = serde_json::from_str::<SaveStateMeta>(&json_str) {
                slots[idx] = Some(meta);
                continue;
            }
        }
        if let Ok(fs_meta) = fs::metadata(&state_path) {
            let mtime = fs_meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            slots[idx] = Some(SaveStateMeta {
                slot,
                created_at: mtime,
                system: "PC".to_string(),
                game_key: game_key.to_string(),
                game_title: game_key.to_string(),
                fdd1_path: String::new(),
                fdd2_path: String::new(),
                hdd1_path: String::new(),
                tape_path: String::new(),
                resolution: (640, 400),
                size_bytes: fs_meta.len() as usize,
            });
        }
    }
    slots
}

/// Decodes `slot_{n}.jpg` into RGBA8 `(width, height, rgba_bytes)` for `egui::ColorImage`.
pub fn load_slot_thumbnail_rgba(
    states_root: &Path,
    game_key: &str,
    slot: u8,
) -> Option<(u32, u32, Vec<u8>)> {
    let slot = slot.clamp(1, SAVE_SLOT_COUNT as u8);
    let thumb_path = slot_thumb_path(states_root, game_key, slot);
    let bytes = fs::read(&thumb_path).ok()?;
    let decoded = image::load_from_memory(&bytes).ok()?.into_rgba8();
    let (w, h) = decoded.dimensions();
    Some((w, h, decoded.into_raw()))
}

/// Deletes `slot_{n}.state`, `slot_{n}.json`, and `slot_{n}.jpg` for the given `game_key` and `slot`.
pub fn delete_slot(states_root: &Path, game_key: &str, slot: u8) -> Result<()> {
    let slot = slot.clamp(1, SAVE_SLOT_COUNT as u8);
    for p in [
        slot_state_path(states_root, game_key, slot),
        slot_meta_path(states_root, game_key, slot),
        slot_thumb_path(states_root, game_key, slot),
    ] {
        if p.exists() {
            fs::remove_file(&p)
                .with_context(|| format!("Failed to delete save-state file {:?}", p))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_and_derive_game_keys() {
        assert_eq!(
            sanitize_game_key("Touhou 02 - Story of Eastern Wonderland/disk1"),
            "Touhou_02_-_Story_of_Eastern_Wonderland_disk1"
        );
        assert_eq!(sanitize_game_key("   ///   "), "default_system");

        let mut cfg98 = Pc98Config::default();
        assert_eq!(
            derive_pc98_game_key(&cfg98),
            ("pc98_system".to_string(), "PC-98 System".to_string())
        );
        cfg98.fdd1_path = "/games/pc98/Policenauts_DiskA.fdi".to_string();
        assert_eq!(
            derive_pc98_game_key(&cfg98),
            (
                "Policenauts_DiskA".to_string(),
                "Policenauts_DiskA".to_string()
            )
        );
        cfg98.selected_game = "Action/Rusty".to_string();
        assert_eq!(
            derive_pc98_game_key(&cfg98),
            ("Action_Rusty".to_string(), "Rusty".to_string())
        );

        let mut cfg88 = Pc88Config::default();
        cfg88.selected_game = "RPG/Ys_II".to_string();
        assert_eq!(
            derive_pc88_game_key(&cfg88),
            ("RPG_Ys_II".to_string(), "Ys_II".to_string())
        );
    }

    #[test]
    fn test_write_read_list_and_delete_slot() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let game_key = "test_game_98";

        // Create a synthetic 64x40 RGBA frame
        let mut rgba = vec![0u8; 64 * 40 * 4];
        for px in rgba.chunks_exact_mut(4) {
            px[0] = 20;
            px[1] = 180;
            px[2] = 240;
            px[3] = 255;
        }
        let frame = RawFrame {
            width: 64,
            height: 40,
            data: rgba,
            format: ffmpeg_next::format::Pixel::RGBA,
            color_range: crate::video::types::ColorRange::Full,
            captured_at: 0,
            rate: crate::replay::config::Rate { num: 60, den: 1 },
        };

        let meta = SaveStateMeta {
            slot: 3,
            created_at: 1_700_000_000,
            system: "PC-98".to_string(),
            game_key: game_key.to_string(),
            game_title: "Test Game 98".to_string(),
            fdd1_path: "/roms/disk1.fdi".to_string(),
            fdd2_path: "/roms/disk2.fdi".to_string(),
            hdd1_path: String::new(),
            tape_path: String::new(),
            resolution: (640, 400),
            size_bytes: 5,
        };

        write_slot_files(root, game_key, &meta, b"STATE", Some(&frame)).unwrap();

        let slots = list_game_slots(root, game_key);
        assert!(slots[0].is_none());
        assert!(slots[1].is_none());
        assert_eq!(slots[2].as_ref(), Some(&meta));
        assert_eq!(
            slots[2].as_ref().unwrap().media_label().as_deref(),
            Some("disk1.fdi")
        );

        let (loaded_meta, loaded_bytes) = read_slot_state(root, game_key, 3).unwrap();
        assert_eq!(loaded_meta, meta);
        assert_eq!(loaded_bytes, b"STATE");

        let (tw, th, thumb_rgba) = load_slot_thumbnail_rgba(root, game_key, 3).unwrap();
        assert_eq!(tw, THUMBNAIL_WIDTH);
        assert_eq!(th, THUMBNAIL_HEIGHT);
        assert_eq!(
            thumb_rgba.len(),
            (THUMBNAIL_WIDTH * THUMBNAIL_HEIGHT * 4) as usize
        );

        delete_slot(root, game_key, 3).unwrap();
        let slots_after = list_game_slots(root, game_key);
        assert!(slots_after[2].is_none());
        assert!(load_slot_thumbnail_rgba(root, game_key, 3).is_none());
    }

    #[test]
    fn test_cycle_slot_wraparound_and_boot_load_dispatch() {
        let mut pc98 = crate::pc98::Pc98State::default();
        assert_eq!(pc98.active_slot, 1);
        assert_eq!(pc98.cycle_slot(-1), 8);
        assert_eq!(pc98.cycle_slot(1), 1);
        assert_eq!(pc98.cycle_slot(2), 3);

        let mut pc88 = crate::pc88::Pc88State::default();
        assert_eq!(pc88.active_slot, 1);
        assert_eq!(pc88.cycle_slot(-1), 8);
        assert_eq!(pc88.cycle_slot(1), 1);
    }
}
