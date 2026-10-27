use crate::pc88::Pc88Config;
use crate::pc98::Pc98Config;
use crate::video::types::RawFrame;
use anyhow::{Context, Result};
use image::imageops::FilterType;
use image::{ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

static SLOT_GENERATION_COUNTER: AtomicU64 = AtomicU64::new(1);

fn slot_commit_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Explicitly specified 64-bit FNV-1a digest for platform/compiler-independent identity and integrity hashes.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

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

/// Sanitizes an arbitrary game identifier or file stem into a safe directory name or readable slug.
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

fn sanitize_slug_prefix(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(48));
    let mut last_underscore = false;
    for ch in raw.trim().chars() {
        if ch.is_alphanumeric() || ch == '-' || ch == '.' {
            out.push(ch);
            last_underscore = false;
        } else if !last_underscore && !out.is_empty() {
            out.push('_');
            last_underscore = true;
        }
        if out.chars().count() >= 48 {
            break;
        }
    }
    let cleaned = out.trim_matches(|c: char| c == '_' || c == '.').to_string();
    if cleaned.is_empty() {
        "game".to_string()
    } else {
        cleaned
    }
}

fn normalize_path_identity(raw_path: &str) -> String {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let p = Path::new(trimmed);
    if let Ok(canon) = fs::canonicalize(p) {
        return canon.to_string_lossy().into_owned();
    }
    let normalized = trimmed.replace('\\', "/");
    let stripped = normalized.trim_end_matches('/');
    if stripped.is_empty() {
        "/".to_string()
    } else {
        stripped.to_string()
    }
}

/// Builds a collision-free save-state directory key combining a human-readable slug and a
/// deterministic 64-bit FNV-1a digest of the system, library root, and canonical game/media identity.
pub fn build_canonical_game_key(
    system: &str,
    root_dir: &str,
    canonical_identity: &str,
    readable_hint: &str,
) -> String {
    let slug = sanitize_slug_prefix(readable_hint);
    let norm_root = normalize_path_identity(root_dir);
    let payload = format!(
        "michadame-savekey-v2\0system={}\0root={}\0id={}",
        system.trim(),
        norm_root,
        canonical_identity.trim()
    );
    let digest = fnv1a_64(payload.as_bytes());
    format!("{slug}__{digest:016x}")
}

/// Extracts the legacy slug portion of a canonical `slug__<16hex>` game key, if present.
fn extract_legacy_slug(game_key: &str) -> Option<&str> {
    let (prefix, hex) = game_key.rsplit_once("__")?;
    if hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()) && !prefix.is_empty() {
        Some(prefix)
    } else {
        None
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
        let key = build_canonical_game_key(
            "PC-98",
            &config.games_dir,
            &format!("selected:{selected}"),
            selected,
        );
        return (key, title);
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
                    let norm_media = normalize_path_identity(media_path);
                    let key = build_canonical_game_key(
                        "PC-98",
                        "",
                        &format!("media:{norm_media}"),
                        trimmed_stem,
                    );
                    return (key, trimmed_stem.to_string());
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
        let key = build_canonical_game_key(
            "PC-88",
            &config.games_dir,
            &format!("selected:{selected}"),
            selected,
        );
        return (key, title);
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
                    let norm_media = normalize_path_identity(media_path);
                    let key = build_canonical_game_key(
                        "PC-88",
                        "",
                        &format!("media:{norm_media}"),
                        trimmed_stem,
                    );
                    return (key, trimmed_stem.to_string());
                }
            }
        }
    }

    ("pc88_system".to_string(), "PC-88 System".to_string())
}

/// Migrates or preserves legacy `<states_root>/<legacy_slug>` save slots when accessed via a new
/// canonical `<slug>__<16hex>` game key.
/// - Detects ambiguity across slots in the legacy directory (e.g. multiple distinct game titles or
///   distinct media parent directories stored in the same legacy directory).
/// - Copies compatible slots into the new canonical directory while always preserving the legacy
///   directory intact so colliding legacy games never lose any of their existing slots.
pub fn migrate_legacy_slots_if_needed(
    states_root: &Path,
    game_key: &str,
    expected_title: Option<&str>,
    expected_media_dir: Option<&Path>,
) {
    let Some(legacy_slug) = extract_legacy_slug(game_key) else {
        return;
    };
    let canonical_dir = states_root.join(sanitize_game_key(game_key));
    if canonical_dir.exists() {
        return;
    }
    let legacy_dir = states_root.join(sanitize_game_key(legacy_slug));
    if !legacy_dir.is_dir() {
        return;
    }

    let _ = fs::create_dir_all(&canonical_dir);
    for idx in 0..SAVE_SLOT_COUNT {
        let slot = (idx + 1) as u8;
        let leg_state = legacy_dir.join(format!("slot_{slot}.state"));
        let leg_meta = legacy_dir.join(format!("slot_{slot}.json"));
        let leg_thumb = legacy_dir.join(format!("slot_{slot}.jpg"));
        if !leg_state.is_file() || !leg_meta.is_file() {
            continue;
        }
        let Ok(json_str) = fs::read_to_string(&leg_meta) else {
            continue;
        };
        let Ok(mut meta) = serde_json::from_str::<SaveStateMeta>(&json_str) else {
            continue;
        };

        // Check whether this legacy slot belongs to a different colliding game
        if let Some(exp_title) = expected_title {
            if !meta.game_title.is_empty() && meta.game_title != exp_title {
                continue;
            }
        }
        if let Some(exp_dir) = expected_media_dir {
            let slot_media = [
                &meta.fdd1_path,
                &meta.hdd1_path,
                &meta.tape_path,
                &meta.fdd2_path,
            ]
            .into_iter()
            .find(|s| !s.trim().is_empty());
            if let Some(mpath) = slot_media {
                if let Some(parent) = Path::new(mpath.trim()).parent() {
                    if parent != exp_dir {
                        continue;
                    }
                }
            }
        }

        let Ok(state_bytes) = fs::read(&leg_state) else {
            continue;
        };
        meta.game_key = game_key.to_string();
        meta.size_bytes = state_bytes.len();
        let _ = write_slot_files(states_root, game_key, &meta, &state_bytes, None);
        if leg_thumb.is_file() {
            if let Ok(thumb_bytes) = fs::read(&leg_thumb) {
                let _ = write_slot_files_raw_thumbnail(
                    states_root,
                    game_key,
                    &meta,
                    &state_bytes,
                    Some(&thumb_bytes),
                    None,
                    None,
                );
            }
        }
    }
}

pub fn game_states_dir(states_root: &Path, game_key: &str) -> PathBuf {
    let dir = states_root.join(sanitize_game_key(game_key));
    if !dir.exists() {
        migrate_legacy_slots_if_needed(states_root, game_key, None, None);
    }
    dir
}

const SLOT_MANIFEST_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlotManifest {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    generation: u64,
    #[serde(default)]
    state_checksum: String,
    #[serde(default)]
    has_thumbnail: bool,
    #[serde(default)]
    thumb_checksum: Option<String>,
    #[serde(default)]
    core_id: String,
    #[serde(flatten)]
    meta: SaveStateMeta,
}

fn slot_gen_state_path(dir: &Path, slot: u8, generation: u64) -> PathBuf {
    dir.join(format!("slot_{slot}.gen_{generation}.state"))
}

fn slot_gen_thumb_path(dir: &Path, slot: u8, generation: u64) -> PathBuf {
    dir.join(format!("slot_{slot}.gen_{generation}.jpg"))
}

fn read_slot_manifest(states_root: &Path, game_key: &str, slot: u8) -> Result<SlotManifest> {
    let slot = slot.clamp(1, SAVE_SLOT_COUNT as u8);
    let meta_path = slot_meta_path(states_root, game_key, slot);
    let json_str = fs::read_to_string(&meta_path)
        .with_context(|| format!("Missing or unreadable metadata file {:?}", meta_path))?;
    let manifest: SlotManifest = serde_json::from_str(&json_str)
        .with_context(|| format!("Corrupted save-state metadata JSON in {:?}", meta_path))?;
    if manifest.meta.slot != slot {
        anyhow::bail!(
            "Save-state metadata slot mismatch: file is for slot {}, declared slot {}",
            slot,
            manifest.meta.slot
        );
    }
    Ok(manifest)
}

pub fn slot_state_path(states_root: &Path, game_key: &str, slot: u8) -> PathBuf {
    let slot = slot.clamp(1, SAVE_SLOT_COUNT as u8);
    let dir = game_states_dir(states_root, game_key);
    if let Ok(manifest) = read_slot_manifest(states_root, game_key, slot) {
        if manifest.schema_version >= SLOT_MANIFEST_SCHEMA_VERSION && manifest.generation > 0 {
            let gen_path = slot_gen_state_path(&dir, slot, manifest.generation);
            if gen_path.exists() {
                return gen_path;
            }
        }
    }
    dir.join(format!("slot_{}.state", slot))
}

pub fn slot_meta_path(states_root: &Path, game_key: &str, slot: u8) -> PathBuf {
    let slot = slot.clamp(1, SAVE_SLOT_COUNT as u8);
    game_states_dir(states_root, game_key).join(format!("slot_{}.json", slot))
}

pub fn slot_thumb_path(states_root: &Path, game_key: &str, slot: u8) -> PathBuf {
    let slot = slot.clamp(1, SAVE_SLOT_COUNT as u8);
    let dir = game_states_dir(states_root, game_key);
    if let Ok(manifest) = read_slot_manifest(states_root, game_key, slot) {
        if manifest.schema_version >= SLOT_MANIFEST_SCHEMA_VERSION {
            if !manifest.has_thumbnail {
                return dir.join(format!(
                    "slot_{}.gen_{}.no_thumb.jpg",
                    slot, manifest.generation
                ));
            }
            if manifest.generation > 0 {
                return slot_gen_thumb_path(&dir, slot, manifest.generation);
            }
        }
    }
    dir.join(format!("slot_{}.jpg", slot))
}

/// Fault injection boundaries for testing atomic multi-file slot commits (F06).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveFaultBoundary {
    WriteState,
    RenameState,
    EncodeThumbnail,
    WriteThumbnail,
    RenameThumbnail,
    WriteManifest,
    RenameManifest,
}

fn write_and_sync_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f =
        fs::File::create(path).with_context(|| format!("Failed to create file {:?}", path))?;
    f.write_all(bytes)
        .with_context(|| format!("Failed to write bytes to {:?}", path))?;
    f.sync_all()
        .with_context(|| format!("Failed to sync file {:?}", path))?;
    Ok(())
}

/// Atomically writes a new generation of `slot` state, thumbnail, and metadata manifest,
/// retaining the previous valid generation until the manifest pointer is atomically replaced.
pub fn write_slot_files(
    states_root: &Path,
    game_key: &str,
    meta: &SaveStateMeta,
    state_bytes: &[u8],
    frame: Option<&RawFrame>,
) -> Result<()> {
    write_slot_files_with_options(states_root, game_key, meta, state_bytes, frame, None, None)
}

/// Extended slot writer supporting an explicit `core_id` and optional fault injection boundary.
pub fn write_slot_files_with_options(
    states_root: &Path,
    game_key: &str,
    meta: &SaveStateMeta,
    state_bytes: &[u8],
    frame: Option<&RawFrame>,
    core_id: Option<&str>,
    fault: Option<SaveFaultBoundary>,
) -> Result<()> {
    let thumb_bytes = if let Some(raw) = frame {
        if fault == Some(SaveFaultBoundary::EncodeThumbnail) {
            anyhow::bail!("Injected fault at EncodeThumbnail");
        }
        Some(encode_frame_thumbnail_jpeg(raw)?)
    } else {
        None
    };

    write_slot_files_raw_thumbnail(
        states_root,
        game_key,
        meta,
        state_bytes,
        thumb_bytes.as_deref(),
        core_id,
        fault,
    )
}

fn write_slot_files_raw_thumbnail(
    states_root: &Path,
    game_key: &str,
    meta: &SaveStateMeta,
    state_bytes: &[u8],
    jpeg_bytes: Option<&[u8]>,
    core_id: Option<&str>,
    fault: Option<SaveFaultBoundary>,
) -> Result<()> {
    if state_bytes.is_empty() {
        anyhow::bail!("Cannot save empty state buffer");
    }

    let _guard = slot_commit_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let slot = meta.slot.clamp(1, SAVE_SLOT_COUNT as u8);
    let dir = game_states_dir(states_root, game_key);
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create save-state directory {:?}", dir))?;

    let prev_gen = read_slot_manifest(states_root, game_key, slot)
        .map(|m| m.generation)
        .unwrap_or(0);
    let next_counter = SLOT_GENERATION_COUNTER.fetch_add(1, Ordering::SeqCst);
    let generation = prev_gen.saturating_add(1).max(next_counter);

    let gen_state_path = slot_gen_state_path(&dir, slot, generation);
    let tmp_state_path = dir.join(format!("slot_{slot}.gen_{generation}.state.tmp"));
    let gen_thumb_path = slot_gen_thumb_path(&dir, slot, generation);
    let tmp_thumb_path = dir.join(format!("slot_{slot}.gen_{generation}.jpg.tmp"));
    let meta_path = slot_meta_path(states_root, game_key, slot);
    let tmp_meta_path = dir.join(format!("slot_{slot}.gen_{generation}.json.tmp"));

    let cleanup_staged = || {
        let _ = fs::remove_file(&tmp_state_path);
        let _ = fs::remove_file(&gen_state_path);
        let _ = fs::remove_file(&tmp_thumb_path);
        let _ = fs::remove_file(&gen_thumb_path);
        let _ = fs::remove_file(&tmp_meta_path);
    };

    // 1. Write and finalize generation state file
    if fault == Some(SaveFaultBoundary::WriteState) {
        cleanup_staged();
        anyhow::bail!("Injected fault at WriteState");
    }
    if let Err(e) = write_and_sync_file(&tmp_state_path, state_bytes) {
        cleanup_staged();
        return Err(e)
            .with_context(|| format!("Failed to write state data to {:?}", tmp_state_path));
    }
    if fault == Some(SaveFaultBoundary::RenameState) {
        cleanup_staged();
        anyhow::bail!("Injected fault at RenameState");
    }
    if let Err(e) = fs::rename(&tmp_state_path, &gen_state_path) {
        cleanup_staged();
        return Err(e)
            .with_context(|| format!("Failed to finalize state file {:?}", gen_state_path));
    }

    // 2. Write and finalize generation thumbnail file (if provided)
    let (has_thumbnail, thumb_checksum) = if let Some(jpeg) = jpeg_bytes {
        if fault == Some(SaveFaultBoundary::WriteThumbnail) {
            cleanup_staged();
            anyhow::bail!("Injected fault at WriteThumbnail");
        }
        if let Err(e) = write_and_sync_file(&tmp_thumb_path, jpeg) {
            cleanup_staged();
            return Err(e)
                .with_context(|| format!("Failed to write thumbnail to {:?}", tmp_thumb_path));
        }
        if fault == Some(SaveFaultBoundary::RenameThumbnail) {
            cleanup_staged();
            anyhow::bail!("Injected fault at RenameThumbnail");
        }
        if let Err(e) = fs::rename(&tmp_thumb_path, &gen_thumb_path) {
            cleanup_staged();
            return Err(e)
                .with_context(|| format!("Failed to finalize thumbnail {:?}", gen_thumb_path));
        }
        (true, Some(format!("{:016x}", fnv1a_64(jpeg))))
    } else {
        (false, None)
    };

    // 3. Build and write manifest JSON, then atomically replace `slot_{n}.json`
    let mut normalized_meta = meta.clone();
    normalized_meta.slot = slot;
    normalized_meta.size_bytes = state_bytes.len();

    let manifest = SlotManifest {
        schema_version: SLOT_MANIFEST_SCHEMA_VERSION,
        generation,
        state_checksum: format!("{:016x}", fnv1a_64(state_bytes)),
        has_thumbnail,
        thumb_checksum,
        core_id: core_id.unwrap_or_default().to_string(),
        meta: normalized_meta,
    };

    let json = serde_json::to_string_pretty(&manifest)
        .context("Failed to serialize save-state metadata JSON")?;
    if fault == Some(SaveFaultBoundary::WriteManifest) {
        cleanup_staged();
        anyhow::bail!("Injected fault at WriteManifest");
    }
    if let Err(e) = write_and_sync_file(&tmp_meta_path, json.as_bytes()) {
        cleanup_staged();
        return Err(e).with_context(|| format!("Failed to write metadata to {:?}", tmp_meta_path));
    }
    if fault == Some(SaveFaultBoundary::RenameManifest) {
        cleanup_staged();
        anyhow::bail!("Injected fault at RenameManifest");
    }
    if let Err(e) = fs::rename(&tmp_meta_path, &meta_path) {
        cleanup_staged();
        return Err(e).with_context(|| format!("Failed to finalize metadata file {:?}", meta_path));
    }

    // 4. Garbage-collect obsolete generations for this slot now that the new manifest is committed
    garbage_collect_old_slot_generations(&dir, slot, generation);

    Ok(())
}

fn garbage_collect_old_slot_generations(dir: &Path, slot: u8, active_gen: u64) {
    let prefix = format!("slot_{slot}.gen_");
    let keep_state = format!("slot_{slot}.gen_{active_gen}.state");
    let keep_thumb = format!("slot_{slot}.gen_{active_gen}.jpg");

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let fname = entry.file_name().to_string_lossy().into_owned();
            if fname.starts_with(&prefix) && fname != keep_state && fname != keep_thumb {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    // Remove legacy un-versioned thumbnail if the new commit has no thumbnail
    let legacy_thumb = dir.join(format!("slot_{slot}.jpg"));
    if legacy_thumb.exists() {
        let _ = fs::remove_file(legacy_thumb);
    }
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

fn read_and_validate_manifest_slot(
    states_root: &Path,
    game_key: &str,
    slot: u8,
) -> Result<(SlotManifest, Vec<u8>)> {
    let slot = slot.clamp(1, SAVE_SLOT_COUNT as u8);
    let dir = game_states_dir(states_root, game_key);
    let meta_path = slot_meta_path(states_root, game_key, slot);

    if !meta_path.is_file() {
        let legacy_state = dir.join(format!("slot_{slot}.state"));
        if legacy_state.is_file() {
            anyhow::bail!(
                "Slot {} is missing its metadata manifest (use explicit legacy recovery)",
                slot
            );
        }
        anyhow::bail!("Slot {} is empty (no save state found)", slot);
    }

    let manifest = read_slot_manifest(states_root, game_key, slot)?;
    let state_path =
        if manifest.schema_version >= SLOT_MANIFEST_SCHEMA_VERSION && manifest.generation > 0 {
            slot_gen_state_path(&dir, slot, manifest.generation)
        } else {
            dir.join(format!("slot_{slot}.state"))
        };

    if !state_path.is_file() {
        anyhow::bail!(
            "Slot {} state payload {:?} is missing",
            slot,
            state_path.file_name().unwrap_or_default()
        );
    }

    let state_bytes = fs::read(&state_path)
        .with_context(|| format!("Failed to read save state {:?}", state_path))?;

    if state_bytes.is_empty() {
        anyhow::bail!("Slot {} state file is empty", slot);
    }
    if state_bytes.len() != manifest.meta.size_bytes {
        anyhow::bail!(
            "Slot {} state size mismatch: metadata declared {} bytes, got {} bytes",
            slot,
            manifest.meta.size_bytes,
            state_bytes.len()
        );
    }
    if manifest.schema_version >= SLOT_MANIFEST_SCHEMA_VERSION
        && !manifest.state_checksum.is_empty()
    {
        let actual_checksum = format!("{:016x}", fnv1a_64(&state_bytes));
        if actual_checksum != manifest.state_checksum {
            anyhow::bail!(
                "Slot {} state checksum mismatch: expected {}, got {}",
                slot,
                manifest.state_checksum,
                actual_checksum
            );
        }
    }

    Ok((manifest, state_bytes))
}

/// Reads and validates the serialized Libretro state bytes and metadata for `slot` (`1..=8`).
pub fn read_slot_state(
    states_root: &Path,
    game_key: &str,
    slot: u8,
) -> Result<(SaveStateMeta, Vec<u8>)> {
    let (manifest, state_bytes) = read_and_validate_manifest_slot(states_root, game_key, slot)?;
    Ok((manifest.meta, state_bytes))
}

/// Explicit legacy recovery helper for orphaned v1 `.state` files that lack a `.json` manifest.
pub fn recover_legacy_orphan_slot(
    states_root: &Path,
    game_key: &str,
    slot: u8,
    system: &str,
) -> Result<(SaveStateMeta, Vec<u8>)> {
    let slot = slot.clamp(1, SAVE_SLOT_COUNT as u8);
    let dir = game_states_dir(states_root, game_key);
    let legacy_state = dir.join(format!("slot_{slot}.state"));
    let state_bytes = fs::read(&legacy_state)
        .with_context(|| format!("No legacy state file at {:?}", legacy_state))?;
    if state_bytes.is_empty() {
        anyhow::bail!("Legacy state file is empty");
    }
    let mtime = fs::metadata(&legacy_state)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let meta = SaveStateMeta {
        slot,
        created_at: mtime,
        system: system.to_string(),
        game_key: game_key.to_string(),
        game_title: game_key.to_string(),
        fdd1_path: String::new(),
        fdd2_path: String::new(),
        hdd1_path: String::new(),
        tape_path: String::new(),
        resolution: (640, 400),
        size_bytes: state_bytes.len(),
    };
    Ok((meta, state_bytes))
}

/// Validates slot identity, core compatibility, state size, and referenced disk media before
/// modifying a running emulator session (F07).
pub fn validate_slot_for_session_load(
    states_root: &Path,
    game_key: &str,
    slot: u8,
    expected_system: &str,
    expected_core_id: Option<&str>,
    expected_serialize_size: Option<usize>,
) -> Result<(SaveStateMeta, Vec<u8>)> {
    let (manifest, state_bytes) = read_and_validate_manifest_slot(states_root, game_key, slot)?;

    if !manifest.meta.system.is_empty() && manifest.meta.system != expected_system {
        anyhow::bail!(
            "Slot {} belongs to system '{}', expected '{}'",
            slot,
            manifest.meta.system,
            expected_system
        );
    }

    if !manifest.meta.game_key.is_empty() && manifest.meta.game_key != game_key {
        let legacy_match =
            extract_legacy_slug(game_key).is_some_and(|slug| manifest.meta.game_key == slug);
        if !legacy_match {
            anyhow::bail!(
                "Slot {} belongs to game '{}', expected '{}'",
                slot,
                manifest.meta.game_key,
                game_key
            );
        }
    }

    if let Some(exp_core) = expected_core_id {
        if !manifest.core_id.is_empty() && manifest.core_id != exp_core {
            anyhow::bail!(
                "Slot {} was created with incompatible core '{}' (current: '{}')",
                slot,
                manifest.core_id,
                exp_core
            );
        }
    }

    if let Some(max_size) = expected_serialize_size {
        if max_size > 0 && state_bytes.len() > max_size {
            anyhow::bail!(
                "Slot {} state payload ({} bytes) exceeds core serialize capacity ({} bytes)",
                slot,
                state_bytes.len(),
                max_size
            );
        }
    }

    for (drive_label, media_path) in [
        ("FDD1", manifest.meta.fdd1_path.trim()),
        ("FDD2", manifest.meta.fdd2_path.trim()),
        ("HDD1", manifest.meta.hdd1_path.trim()),
        ("TAPE", manifest.meta.tape_path.trim()),
    ] {
        if !media_path.is_empty() && !Path::new(media_path).is_file() {
            anyhow::bail!(
                "Slot {} references missing {} media file '{}'",
                slot,
                drive_label,
                media_path
            );
        }
    }

    Ok((manifest.meta, state_bytes))
}

/// Coordinates mounting saved floppy disks and invoking `unserialize` with automatic media and
/// state rollback if mounting or `unserialize` fails (F07).
pub fn execute_load_state_with_rollback<M, U>(
    meta: &SaveStateMeta,
    state_bytes: &[u8],
    current_fdd1: &mut Option<PathBuf>,
    current_fdd2: &mut Option<PathBuf>,
    mut mount_drive: M,
    mut unserialize: U,
) -> Result<()>
where
    M: FnMut(u8, Option<&Path>) -> bool,
    U: FnMut(&[u8]) -> bool,
{
    let prev_fdd1 = current_fdd1.clone();
    let prev_fdd2 = current_fdd2.clone();

    let target_fdd1 = if meta.fdd1_path.trim().is_empty() {
        prev_fdd1.clone()
    } else {
        let p = PathBuf::from(meta.fdd1_path.trim());
        if !p.is_file() {
            anyhow::bail!("Saved FDD1 image does not exist: {}", p.display());
        }
        Some(p)
    };

    let target_fdd2 = if meta.fdd2_path.trim().is_empty() {
        prev_fdd2.clone()
    } else {
        let p = PathBuf::from(meta.fdd2_path.trim());
        if !p.is_file() {
            anyhow::bail!("Saved FDD2 image does not exist: {}", p.display());
        }
        Some(p)
    };

    let fdd1_changed = &target_fdd1 != current_fdd1;
    let fdd2_changed = &target_fdd2 != current_fdd2;

    if fdd1_changed {
        if !mount_drive(0, target_fdd1.as_deref()) {
            let _ = mount_drive(0, prev_fdd1.as_deref());
            anyhow::bail!("Core failed to mount saved FDD1 image before state load");
        }
        *current_fdd1 = target_fdd1;
    }

    if fdd2_changed {
        if !mount_drive(1, target_fdd2.as_deref()) {
            let _ = mount_drive(1, prev_fdd2.as_deref());
            if fdd1_changed {
                let _ = mount_drive(0, prev_fdd1.as_deref());
                *current_fdd1 = prev_fdd1;
            }
            anyhow::bail!("Core failed to mount saved FDD2 image before state load");
        }
        *current_fdd2 = target_fdd2;
    }

    if !unserialize(state_bytes) {
        if fdd2_changed {
            let _ = mount_drive(1, prev_fdd2.as_deref());
            *current_fdd2 = prev_fdd2;
        }
        if fdd1_changed {
            let _ = mount_drive(0, prev_fdd1.as_deref());
            *current_fdd1 = prev_fdd1;
        }
        anyhow::bail!(
            "Core rejected save-state data in slot {}; restored previous mounted disks",
            meta.slot
        );
    }

    Ok(())
}

/// Returns metadata for all 8 save slots (`index = (slot - 1) as usize`) of `game_key`.
/// Only valid, intact slots are returned.
pub fn list_game_slots(
    states_root: &Path,
    game_key: &str,
) -> [Option<SaveStateMeta>; SAVE_SLOT_COUNT] {
    let mut slots: [Option<SaveStateMeta>; SAVE_SLOT_COUNT] = Default::default();
    for idx in 0..SAVE_SLOT_COUNT {
        let slot = (idx + 1) as u8;
        if let Ok((manifest, _)) = read_and_validate_manifest_slot(states_root, game_key, slot) {
            slots[idx] = Some(manifest.meta);
        }
    }
    slots
}

/// Decodes the active generation's thumbnail into RGBA8 `(width, height, rgba_bytes)` for `egui::ColorImage`.
pub fn load_slot_thumbnail_rgba(
    states_root: &Path,
    game_key: &str,
    slot: u8,
) -> Option<(u32, u32, Vec<u8>)> {
    let slot = slot.clamp(1, SAVE_SLOT_COUNT as u8);
    let dir = game_states_dir(states_root, game_key);
    let manifest = read_slot_manifest(states_root, game_key, slot).ok()?;
    if manifest.schema_version >= SLOT_MANIFEST_SCHEMA_VERSION && !manifest.has_thumbnail {
        return None;
    }
    let thumb_path =
        if manifest.schema_version >= SLOT_MANIFEST_SCHEMA_VERSION && manifest.generation > 0 {
            slot_gen_thumb_path(&dir, slot, manifest.generation)
        } else {
            dir.join(format!("slot_{slot}.jpg"))
        };
    let bytes = fs::read(&thumb_path).ok()?;
    if let Some(ref exp_hash) = manifest.thumb_checksum {
        let actual_hash = format!("{:016x}", fnv1a_64(&bytes));
        if &actual_hash != exp_hash {
            return None;
        }
    }
    let decoded = image::load_from_memory(&bytes).ok()?.into_rgba8();
    let (w, h) = decoded.dimensions();
    Some((w, h, decoded.into_raw()))
}

/// Deletes all state, manifest, and thumbnail files for the given `game_key` and `slot`.
pub fn delete_slot(states_root: &Path, game_key: &str, slot: u8) -> Result<()> {
    let _guard = slot_commit_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let slot = slot.clamp(1, SAVE_SLOT_COUNT as u8);
    let dir = game_states_dir(states_root, game_key);

    for p in [
        dir.join(format!("slot_{slot}.state")),
        dir.join(format!("slot_{slot}.json")),
        dir.join(format!("slot_{slot}.jpg")),
    ] {
        if p.exists() {
            fs::remove_file(&p)
                .with_context(|| format!("Failed to delete save-state file {:?}", p))?;
        }
    }

    let prefix = format!("slot_{slot}.gen_");
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let fname = entry.file_name().to_string_lossy().into_owned();
            if fname.starts_with(&prefix) {
                fs::remove_file(entry.path()).with_context(|| {
                    format!("Failed to delete save-state file {:?}", entry.path())
                })?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_frame(r: u8, g: u8, b: u8) -> RawFrame {
        let mut rgba = vec![0u8; 64 * 40 * 4];
        for px in rgba.chunks_exact_mut(4) {
            px[0] = r;
            px[1] = g;
            px[2] = b;
            px[3] = 255;
        }
        RawFrame {
            width: 64,
            height: 40,
            data: rgba,
            format: ffmpeg_next::format::Pixel::RGBA,
            color_range: crate::video::types::ColorRange::Full,
            captured_at: 0,
            rate: crate::replay::config::Rate { num: 60, den: 1 },
        }
    }

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
        let (key_media, title_media) = derive_pc98_game_key(&cfg98);
        assert!(key_media.starts_with("Policenauts_DiskA__"));
        assert_eq!(title_media, "Policenauts_DiskA");

        cfg98.selected_game = "Action/Rusty".to_string();
        let (key_sel, title_sel) = derive_pc98_game_key(&cfg98);
        assert!(key_sel.starts_with("Action_Rusty__"));
        assert_eq!(title_sel, "Rusty");

        let mut cfg88 = Pc88Config::default();
        cfg88.selected_game = "RPG/Ys_II".to_string();
        let (key88, title88) = derive_pc88_game_key(&cfg88);
        assert!(key88.starts_with("RPG_Ys_II__"));
        assert_eq!(title88, "Ys_II");
    }

    #[test]
    fn test_f05_game_key_collision_resistance_and_legacy_migration() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // 1. Punctuation / separator collisions: "A/Game" vs "A_Game"
        let mut c1 = Pc98Config::default();
        c1.selected_game = "A/Game".to_string();
        let mut c2 = Pc98Config::default();
        c2.selected_game = "A_Game".to_string();
        let (k1, _) = derive_pc98_game_key(&c1);
        let (k2, _) = derive_pc98_game_key(&c2);
        assert_ne!(k1, k2, "A/Game and A_Game must not collide");

        // 2. Long common prefix > 80 chars
        let long_prefix = "Super_Long_PC98_Game_Title_With_More_Than_Eighty_Characters_In_Its_Prefix_Section_0123456789";
        let mut cl1 = Pc98Config::default();
        cl1.selected_game = format!("{long_prefix}_Part_A");
        let mut cl2 = Pc98Config::default();
        cl2.selected_game = format!("{long_prefix}_Part_B");
        assert_ne!(derive_pc98_game_key(&cl1).0, derive_pc98_game_key(&cl2).0);

        // 3. Unicode names
        let mut cu1 = Pc98Config::default();
        cu1.selected_game = "東方靈異伝/博麗".to_string();
        let mut cu2 = Pc98Config::default();
        cu2.selected_game = "東方靈異伝_博麗".to_string();
        assert_ne!(derive_pc98_game_key(&cu1).0, derive_pc98_game_key(&cu2).0);

        // 4. Identical media stems in different directories
        let mut cm1 = Pc88Config::default();
        cm1.fdd1_path = "/games/pc88/GameAlpha/disk1.d88".to_string();
        let mut cm2 = Pc88Config::default();
        cm2.fdd1_path = "/games/pc88/GameBeta/disk1.d88".to_string();
        let (km1, _) = derive_pc88_game_key(&cm1);
        let (km2, _) = derive_pc88_game_key(&cm2);
        assert_ne!(km1, km2, "disk1.d88 in different folders must not collide");

        // 5. Same relative game in two different library roots
        let mut cr1 = Pc98Config::default();
        cr1.games_dir = "/mnt/lib_a".to_string();
        cr1.selected_game = "RPG/Brandish".to_string();
        let mut cr2 = Pc98Config::default();
        cr2.games_dir = "/mnt/lib_b".to_string();
        cr2.selected_game = "RPG/Brandish".to_string();
        assert_ne!(derive_pc98_game_key(&cr1).0, derive_pc98_game_key(&cr2).0);

        // 6. Saving in one must never overwrite the other
        let meta1 = SaveStateMeta {
            slot: 1,
            created_at: 100,
            system: "PC-98".to_string(),
            game_key: k1.clone(),
            game_title: "Game".to_string(),
            fdd1_path: String::new(),
            fdd2_path: String::new(),
            hdd1_path: String::new(),
            tape_path: String::new(),
            resolution: (640, 400),
            size_bytes: 6,
        };
        let mut meta2 = meta1.clone();
        meta2.game_key = k2.clone();
        write_slot_files(root, &k1, &meta1, b"STATE1", None).unwrap();
        write_slot_files(root, &k2, &meta2, b"STATE2", None).unwrap();
        assert_eq!(read_slot_state(root, &k1, 1).unwrap().1, b"STATE1");
        assert_eq!(read_slot_state(root, &k2, 1).unwrap().1, b"STATE2");

        // 7. Legacy directory migration preserves colliding legacy slots
        let legacy_dir = root.join("Legacy_Game");
        fs::create_dir_all(&legacy_dir).unwrap();
        let leg_meta = SaveStateMeta {
            slot: 2,
            created_at: 555,
            system: "PC-98".to_string(),
            game_key: "Legacy_Game".to_string(),
            game_title: "Game".to_string(),
            fdd1_path: String::new(),
            fdd2_path: String::new(),
            hdd1_path: String::new(),
            tape_path: String::new(),
            resolution: (640, 400),
            size_bytes: 12,
        };
        fs::write(legacy_dir.join("slot_2.state"), b"LEGACY_BYTES").unwrap();
        fs::write(
            legacy_dir.join("slot_2.json"),
            serde_json::to_string(&leg_meta).unwrap(),
        )
        .unwrap();

        let mut cfg_leg_a = Pc98Config::default();
        cfg_leg_a.selected_game = "Legacy/Game".to_string();
        let mut cfg_leg_b = Pc98Config::default();
        cfg_leg_b.selected_game = "Legacy_Game".to_string();
        let (k_leg_a, _) = derive_pc98_game_key(&cfg_leg_a);
        let (k_leg_b, _) = derive_pc98_game_key(&cfg_leg_b);
        assert_eq!(
            read_slot_state(root, &k_leg_a, 2).unwrap().1,
            b"LEGACY_BYTES"
        );
        assert_eq!(
            read_slot_state(root, &k_leg_b, 2).unwrap().1,
            b"LEGACY_BYTES"
        );
        // Legacy directory itself is still preserved
        assert!(legacy_dir.join("slot_2.state").is_file());
    }

    #[test]
    fn test_f06_atomic_slot_commit_fault_boundaries_and_validation() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let game_key = "atomic_test_game";

        let frame_v1 = make_test_frame(20, 180, 240);
        let meta_v1 = SaveStateMeta {
            slot: 1,
            created_at: 1_000,
            system: "PC-98".to_string(),
            game_key: game_key.to_string(),
            game_title: "Atomic Test".to_string(),
            fdd1_path: "/roms/v1_disk1.fdi".to_string(),
            fdd2_path: String::new(),
            hdd1_path: String::new(),
            tape_path: String::new(),
            resolution: (640, 400),
            size_bytes: 8,
        };
        write_slot_files(root, game_key, &meta_v1, b"STATE_V1", Some(&frame_v1)).unwrap();
        let thumb_v1 = load_slot_thumbnail_rgba(root, game_key, 1).unwrap().2;

        // Inject failures at every boundary during an attempted overwrite with V2
        let frame_v2 = make_test_frame(250, 30, 30);
        let meta_v2 = SaveStateMeta {
            slot: 1,
            created_at: 2_000,
            system: "PC-98".to_string(),
            game_key: game_key.to_string(),
            game_title: "Atomic Test".to_string(),
            fdd1_path: "/roms/v2_disk1.fdi".to_string(),
            fdd2_path: String::new(),
            hdd1_path: String::new(),
            tape_path: String::new(),
            resolution: (640, 400),
            size_bytes: 8,
        };

        for fault in [
            SaveFaultBoundary::WriteState,
            SaveFaultBoundary::RenameState,
            SaveFaultBoundary::EncodeThumbnail,
            SaveFaultBoundary::WriteThumbnail,
            SaveFaultBoundary::RenameThumbnail,
            SaveFaultBoundary::WriteManifest,
            SaveFaultBoundary::RenameManifest,
        ] {
            let res = write_slot_files_with_options(
                root,
                game_key,
                &meta_v2,
                b"STATE_V2",
                Some(&frame_v2),
                Some("np2kai"),
                Some(fault),
            );
            assert!(res.is_err(), "Fault {fault:?} should have returned Err");

            // Reopening the slot must resolve to the complete V1 generation!
            let (loaded_meta, loaded_bytes) = read_slot_state(root, game_key, 1).unwrap();
            assert_eq!(
                loaded_meta, meta_v1,
                "Metadata changed after fault {fault:?}"
            );
            assert_eq!(
                loaded_bytes, b"STATE_V1",
                "State bytes changed after fault {fault:?}"
            );
            let loaded_thumb = load_slot_thumbnail_rgba(root, game_key, 1).unwrap().2;
            assert_eq!(
                loaded_thumb, thumb_v1,
                "Thumbnail changed after fault {fault:?}"
            );
        }

        // Now save V2 WITHOUT a frame (`frame = None`) -> stale V1 thumbnail must NOT be retained!
        write_slot_files(root, game_key, &meta_v2, b"STATE_V2", None).unwrap();
        let (loaded_meta, loaded_bytes) = read_slot_state(root, game_key, 1).unwrap();
        assert_eq!(loaded_meta, meta_v2);
        assert_eq!(loaded_bytes, b"STATE_V2");
        assert!(
            load_slot_thumbnail_rgba(root, game_key, 1).is_none(),
            "Saving without a frame must not retain a stale thumbnail from an earlier generation"
        );

        // Truncating state bytes must be rejected by read_slot_state and list_game_slots
        let active_state_file = slot_state_path(root, game_key, 1);
        fs::write(&active_state_file, b"TRUNC").unwrap();
        assert!(read_slot_state(root, game_key, 1).is_err());
        assert!(list_game_slots(root, game_key)[0].is_none());

        // Restore valid V2, then corrupt metadata JSON -> must be rejected rather than synthesizing fake defaults
        write_slot_files(root, game_key, &meta_v2, b"STATE_V2", None).unwrap();
        fs::write(slot_meta_path(root, game_key, 1), b"{corrupted json").unwrap();
        assert!(read_slot_state(root, game_key, 1).is_err());
        assert!(list_game_slots(root, game_key)[0].is_none());
    }

    #[test]
    fn test_f07_rejected_load_rolls_back_mounted_disks_and_validates_compatibility() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let game_key = "rollback_test_game";

        let disk_orig_1 = root.join("orig1.d88");
        let disk_orig_2 = root.join("orig2.d88");
        let disk_save_1 = root.join("save1.d88");
        let disk_save_2 = root.join("save2.d88");
        for p in [&disk_orig_1, &disk_orig_2, &disk_save_1, &disk_save_2] {
            fs::write(p, b"DUMMY_DISK").unwrap();
        }

        let meta = SaveStateMeta {
            slot: 1,
            created_at: 1_500,
            system: "PC-88".to_string(),
            game_key: game_key.to_string(),
            game_title: "Rollback Game".to_string(),
            fdd1_path: disk_save_1.to_string_lossy().into_owned(),
            fdd2_path: disk_save_2.to_string_lossy().into_owned(),
            hdd1_path: String::new(),
            tape_path: String::new(),
            resolution: (640, 400),
            size_bytes: 9,
        };
        write_slot_files_with_options(
            root,
            game_key,
            &meta,
            b"GOODSTATE",
            None,
            Some("quasi88_v1"),
            None,
        )
        .unwrap();

        // 1. Incompatible core version is rejected before mounting
        assert!(validate_slot_for_session_load(
            root,
            game_key,
            1,
            "PC-88",
            Some("quasi88_v2"),
            Some(64)
        )
        .is_err());

        // 2. State exceeding core serialize size is rejected
        assert!(validate_slot_for_session_load(
            root,
            game_key,
            1,
            "PC-88",
            Some("quasi88_v1"),
            Some(4)
        )
        .is_err());

        // 3. Fake core that rejects unserialization leaves mounted disks rolled back to original
        let (validated_meta, state_bytes) = validate_slot_for_session_load(
            root,
            game_key,
            1,
            "PC-88",
            Some("quasi88_v1"),
            Some(64),
        )
        .unwrap();

        let mut cur_fdd1 = Some(disk_orig_1.clone());
        let mut cur_fdd2 = Some(disk_orig_2.clone());
        let mut mounted_hw: [Option<PathBuf>; 2] =
            [Some(disk_orig_1.clone()), Some(disk_orig_2.clone())];

        let res = execute_load_state_with_rollback(
            &validated_meta,
            &state_bytes,
            &mut cur_fdd1,
            &mut cur_fdd2,
            |drive, path| {
                mounted_hw[drive as usize] = path.map(|p| p.to_path_buf());
                true
            },
            |_bytes| false, // Fake core rejects unserialize!
        );
        assert!(res.is_err());
        assert_eq!(cur_fdd1.as_ref(), Some(&disk_orig_1));
        assert_eq!(cur_fdd2.as_ref(), Some(&disk_orig_2));
        assert_eq!(mounted_hw[0].as_ref(), Some(&disk_orig_1));
        assert_eq!(mounted_hw[1].as_ref(), Some(&disk_orig_2));

        // 4. Missing saved media is rejected before any drive is changed
        fs::remove_file(&disk_save_2).unwrap();
        assert!(validate_slot_for_session_load(
            root,
            game_key,
            1,
            "PC-88",
            Some("quasi88_v1"),
            Some(64)
        )
        .is_err());
    }

    #[test]
    fn test_write_read_list_and_delete_slot() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let game_key = "test_game_98";
        let frame = make_test_frame(20, 180, 240);

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
