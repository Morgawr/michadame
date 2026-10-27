use crate::pc98::search::{match_search_index_score, Pc98SearchIndex};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

/// Supported PC-88 floppy image extensions (lowercase).
/// Also handles fullwidth `.ｄ88` via `normalized_extension`.
pub const PC88_FLOPPY_EXTENSIONS: &[&str] = &["d88", "88d", "2hd", "2dd", "fdi", "dsk", "dup"];

/// Supported PC-88 cassette tape image extensions (lowercase).
pub const PC88_TAPE_EXTENSIONS: &[&str] = &["t88", "cmt"];

/// Describes how a discovered PC-88 game folder boots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pc88BootMode {
    /// Standard floppy-based game (`.d88`, `.88d`, `.2hd`, `.fdi`, `.dsk`).
    Floppy,
    /// Cassette tape game (`.t88`, `.cmt`).
    Tape,
}

impl Pc88BootMode {
    pub fn badge_label(&self) -> &'static str {
        match self {
            Pc88BootMode::Floppy => "FDD",
            Pc88BootMode::Tape => "TAPE",
        }
    }
}

/// Represents a discovered PC-88 game folder inside the user's games directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pc88GameEntry {
    /// Relative path from the root games directory (used as unique key in config).
    pub id: String,
    /// Display title (derived from the game folder name).
    pub title: String,
    /// Optional parent category folder (e.g. `"A"`, `"あ"`, `"0-9"`, `"PC-8001"`).
    pub category: Option<String>,
    /// Absolute path to the game directory.
    pub folder_path: PathBuf,
    /// Sorted list of floppy disk files (`.d88`, `.88d`, etc.) in the game folder.
    pub floppies: Vec<PathBuf>,
    /// Sorted list of cassette tape files (`.t88`, `.cmt`) in the game folder.
    pub tapes: Vec<PathBuf>,
    /// Detected boot mode.
    pub boot_mode: Pc88BootMode,
    /// Precomputed search index for instant Kanji/Kana/Romaji/English fuzzy filtering.
    pub search_index: Pc98SearchIndex,
}

impl Pc88GameEntry {
    /// Formatted label for the UI combo box.
    pub fn display_label(&self) -> String {
        let cat_prefix = match &self.category {
            Some(c) if !c.is_empty() => format!("[{}] ", c),
            _ => String::new(),
        };
        let media_badge = self.media_summary();
        format!("{}{} ({})", cat_prefix, self.title, media_badge)
    }

    pub fn media_summary(&self) -> String {
        self.media_summary_with_resolved_count(self.floppies.len())
    }

    pub fn media_summary_with_resolved_count(&self, resolved_fdd_count: usize) -> String {
        let mut parts = Vec::new();
        let fdd_count = resolved_fdd_count.max(self.floppies.len());
        if fdd_count > 0 {
            if fdd_count == 1 {
                parts.push("1 FDD".to_string());
            } else {
                parts.push(format!("{} FDDs", fdd_count));
            }
        }
        if !self.tapes.is_empty() {
            if self.tapes.len() == 1 {
                parts.push("1 Tape".to_string());
            } else {
                parts.push(format!("{} Tapes", self.tapes.len()));
            }
        }
        parts.join(" + ")
    }
}

/// Minimum valid size of a single `.d88` disk image (header length `0x2b0` = 688 bytes).
pub const D88_MIN_IMAGE_SIZE: u64 = 0x2b0;

/// Maximum number of disk images supported inside a single `.d88` container for automatic unpacking.
pub const D88_MAX_CONTAINER_IMAGES: usize = 32;

/// Schema version for `.source_d88_manifest` files written in the PC-88 disk cache.
pub const D88_MANIFEST_VERSION: u32 = 2;

const D88_MANIFEST_FILE_NAME: &str = ".source_d88_manifest";
const D88_BACKUP_FILE_NAME: &str = ".last_source_backup.d88";

/// Metadata for a single disk image slice inside a `.d88` container file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct D88ImageSlice {
    /// Internal disk label from the 17-byte `.d88` header (decoded from Shift-JIS / ASCII).
    pub name: String,
    /// Byte offset of this disk image inside the `.d88` file.
    pub offset: u64,
    /// Total byte length of this disk image (including its `0x2b0` header).
    pub size: u64,
}

/// Result of parsing and validating a `.d88` / `.88d` container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum D88ParseResult {
    /// The container consists of `1..=32` valid `.d88` images that consume the entire file with no trailing bytes.
    Complete(Vec<D88ImageSlice>),
    /// The container is structurally valid through EOF but contains more than `D88_MAX_CONTAINER_IMAGES` images.
    UnsupportedTooManyImages {
        parsed_prefix: Vec<D88ImageSlice>,
        total_found: usize,
    },
    /// The file is missing, unreadable, truncated, has a corrupt header/size, or has trailing unconsumed bytes.
    InvalidOrIncomplete { reason: String },
}

/// Per-slice entry recorded inside a versioned `.source_d88_manifest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct D88ManifestSlice {
    pub file_name: String,
    pub label: String,
    pub original_fingerprint: String,
}

/// Versioned manifest linking unpacked cache slices to their canonical multi-image `.d88` source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct D88SourceManifest {
    pub version: u32,
    pub source_path: String,
    pub source_fingerprint: String,
    pub slices: Vec<D88ManifestSlice>,
}

/// Computes a deterministic fingerprint string (`v1:<len_hex>:<fnv1a_hex>:<mix_hex>`) for `bytes`.
pub fn fingerprint_bytes(bytes: &[u8]) -> String {
    let (h1, h2) = deterministic_digest128(bytes);
    format!("v1:{:016x}:{:016x}:{:016x}", bytes.len() as u64, h1, h2)
}

fn deterministic_digest128(bytes: &[u8]) -> (u64, u64) {
    let mut h1: u64 = 0xcbf29ce484222325;
    let mut h2: u64 = 0x84222325cbf29ce4 ^ (bytes.len() as u64).wrapping_mul(0x9e3779b97f4a7c15);
    for (idx, &b) in bytes.iter().enumerate() {
        h1 ^= b as u64;
        h1 = h1.wrapping_mul(0x00000100000001b3);

        let mixed = (b as u64).wrapping_add((idx as u64) & 0xff);
        h2 ^= mixed.rotate_left((idx as u32) & 63);
        h2 = h2.wrapping_mul(0x9e3779b97f4a7c15).rotate_left(17);
    }
    (h1, h2)
}

fn cache_entry_dir_name(canonical_source: &Path) -> String {
    let (h1, h2) = deterministic_digest128(canonical_source.to_string_lossy().as_bytes());
    format!("{:016x}{:016x}", h1, h2)
}

/// Normalizes a file extension (handling fullwidth characters like `.ｄ88` -> `d88`).
pub fn normalized_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    let mut out = String::with_capacity(ext.len());
    for ch in ext.chars() {
        let mapped = match ch {
            '\u{FF21}'..='\u{FF3A}' => {
                char::from_u32(ch as u32 - 0xFF21 + b'a' as u32).unwrap_or(ch)
            }
            '\u{FF41}'..='\u{FF5A}' => {
                char::from_u32(ch as u32 - 0xFF41 + b'a' as u32).unwrap_or(ch)
            }
            '\u{FF10}'..='\u{FF19}' => {
                char::from_u32(ch as u32 - 0xFF10 + b'0' as u32).unwrap_or(ch)
            }
            _ => ch.to_ascii_lowercase(),
        };
        out.push(mapped);
    }
    Some(out)
}

pub fn is_pc88_floppy_path(path: &Path) -> bool {
    normalized_extension(path).is_some_and(|ext| PC88_FLOPPY_EXTENSIONS.contains(&ext.as_str()))
}

pub fn is_pc88_tape_path(path: &Path) -> bool {
    normalized_extension(path).is_some_and(|ext| PC88_TAPE_EXTENSIONS.contains(&ext.as_str()))
}

/// Parses and validates a `.d88` / `.88d` stream of known total byte length `file_len`.
/// Requires every image slice to have `size >= 0x2b0` and requires the slices to consume
/// the entire `file_len` with no trailing unconsumed bytes.
pub fn inspect_d88_reader<R: Read + Seek>(reader: &mut R, file_len: u64) -> D88ParseResult {
    if file_len < D88_MIN_IMAGE_SIZE {
        return D88ParseResult::InvalidOrIncomplete {
            reason: format!(
                "File length {} is smaller than minimum D88 header size {}",
                file_len, D88_MIN_IMAGE_SIZE
            ),
        };
    }

    let mut slices = Vec::new();
    let mut total_found: usize = 0;
    let mut offset: u64 = 0;
    let mut hdr = [0u8; 32];

    while offset < file_len {
        if offset.checked_add(32).is_none_or(|end| end > file_len) {
            return D88ParseResult::InvalidOrIncomplete {
                reason: format!(
                    "Trailing unconsumed bytes at offset {} (file length {})",
                    offset, file_len
                ),
            };
        }
        if let Err(err) = reader.seek(SeekFrom::Start(offset)) {
            return D88ParseResult::InvalidOrIncomplete {
                reason: format!("Failed to seek to D88 header at offset {}: {}", offset, err),
            };
        }
        if let Err(err) = reader.read_exact(&mut hdr) {
            return D88ParseResult::InvalidOrIncomplete {
                reason: format!("Failed to read D88 header at offset {}: {}", offset, err),
            };
        }
        let dsize = u32::from_le_bytes([hdr[0x1c], hdr[0x1d], hdr[0x1e], hdr[0x1f]]) as u64;
        if dsize < D88_MIN_IMAGE_SIZE {
            return D88ParseResult::InvalidOrIncomplete {
                reason: format!(
                    "Invalid D88 image size {} (< {}) at offset {}",
                    dsize, D88_MIN_IMAGE_SIZE, offset
                ),
            };
        }
        let Some(next_offset) = offset.checked_add(dsize) else {
            return D88ParseResult::InvalidOrIncomplete {
                reason: format!(
                    "D88 image size overflow at offset {} (size {})",
                    offset, dsize
                ),
            };
        };
        if next_offset > file_len {
            return D88ParseResult::InvalidOrIncomplete {
                reason: format!(
                    "Truncated D88 image at offset {}: size {} exceeds file length {}",
                    offset, dsize, file_len
                ),
            };
        }

        let raw_name = hdr[..17].split(|&b| b == 0).next().unwrap_or(&[]);
        let name = decode_d88_ank_name(raw_name);
        if slices.len() < D88_MAX_CONTAINER_IMAGES {
            slices.push(D88ImageSlice {
                name,
                offset,
                size: dsize,
            });
        }
        total_found += 1;
        offset = next_offset;
    }

    if total_found == 0 {
        D88ParseResult::InvalidOrIncomplete {
            reason: "No valid D88 images found".to_string(),
        }
    } else if total_found > D88_MAX_CONTAINER_IMAGES {
        D88ParseResult::UnsupportedTooManyImages {
            parsed_prefix: slices,
            total_found,
        }
    } else {
        D88ParseResult::Complete(slices)
    }
}

/// Parses and validates an in-memory `.d88` / `.88d` byte buffer.
pub fn inspect_d88_bytes(bytes: &[u8]) -> D88ParseResult {
    let mut cursor = Cursor::new(bytes);
    inspect_d88_reader(&mut cursor, bytes.len() as u64)
}

/// Parses and validates a `.d88` / `.88d` container file on disk, distinguishing complete valid
/// containers (`1..=32` images consuming the entire file) from unsupported (`> 32` images) or
/// corrupt/truncated/incomplete files.
pub fn inspect_d88_container(path: &Path) -> D88ParseResult {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(err) => {
            return D88ParseResult::InvalidOrIncomplete {
                reason: format!("Failed to stat {}: {}", path.display(), err),
            };
        }
    };
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(err) => {
            return D88ParseResult::InvalidOrIncomplete {
                reason: format!("Failed to open {}: {}", path.display(), err),
            };
        }
    };
    inspect_d88_reader(&mut file, meta.len())
}

/// Inspects a `.d88` / `.88d` file and returns the list of disk image slices contained inside it
/// ONLY when the container is complete and within the supported `1..=32` image limit.
/// Each `.d88` image starts with a 32-byte header prefix where:
/// - `0x00..0x11` (17 bytes): null-terminated Shift-JIS / ASCII disk label
/// - `0x1c..0x20` (4 bytes, LE u32): total byte size of that disk image
pub fn inspect_d88_images(path: &Path) -> Vec<D88ImageSlice> {
    match inspect_d88_container(path) {
        D88ParseResult::Complete(slices) => slices,
        D88ParseResult::UnsupportedTooManyImages { .. }
        | D88ParseResult::InvalidOrIncomplete { .. } => Vec::new(),
    }
}

fn decode_d88_ank_name(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &b in bytes {
        match b {
            0x20..=0x7E => out.push(b as char),
            0xA1..=0xDF => {
                if let Some(ch) = char::from_u32(0xFF61 + (b as u32 - 0xA1)) {
                    out.push(ch);
                }
            }
            _ => {}
        }
    }
    out.trim().to_string()
}

fn sanitize_label_for_filename(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else if ch.is_whitespace() && !out.ends_with('_') {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "disk".to_string()
    } else {
        trimmed.chars().take(24).collect()
    }
}

/// Materializes all floppy disks for a `Pc88GameEntry` using the default PC-88 disk cache directory.
pub fn materialize_game_floppies(entry: &Pc88GameEntry) -> Result<(Vec<PathBuf>, Vec<String>)> {
    materialize_game_floppies_in(entry, &super::core_sync::pc88_disk_cache_dir())
}

/// Materializes all floppy disks for a `Pc88GameEntry` using `cache_root` for unpacked multi-disk `.d88` containers.
/// - Single-image `.d88` files (and other floppy formats) are returned directly at their original path.
/// - Complete multi-image `.d88` container files (`2..=32` slices consuming the entire file) are unpacked into
///   individual per-disk `.d88` files in `<cache_root>/<hash>/` so `quasi88_libretro` can mount Disk 1 in FDD1
///   and Disk 2 in FDD2 simultaneously and swap any disk image live from the UI.
/// - Incomplete, corrupt, or `> 32`-image containers are never unpacked or truncated; their original path is
///   returned untouched without creating a cache manifest.
pub fn materialize_game_floppies_in(
    entry: &Pc88GameEntry,
    cache_root: &Path,
) -> Result<(Vec<PathBuf>, Vec<String>)> {
    let mut out_paths = Vec::new();
    let mut out_labels = Vec::new();

    for fpath in &entry.floppies {
        let ext = normalized_extension(fpath).unwrap_or_default();
        let file_name = fpath
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("disk.d88");

        if ext == "d88" || ext == "88d" {
            match inspect_d88_container(fpath) {
                D88ParseResult::Complete(slices) if slices.len() > 1 => {
                    let (slice_paths, slice_labels) =
                        materialize_multi_d88_file_in(fpath, &slices, out_paths.len(), cache_root)?;
                    out_paths.extend(slice_paths);
                    out_labels.extend(slice_labels);
                    continue;
                }
                D88ParseResult::Complete(slices) => {
                    if let Some(first) = slices.first() {
                        let disk_num = out_paths.len() + 1;
                        let label = if first.name.is_empty() {
                            format!("[{}] {}", disk_num, file_name)
                        } else {
                            format!("[{}] {} ({})", disk_num, file_name, first.name)
                        };
                        out_paths.push(fpath.clone());
                        out_labels.push(label);
                        continue;
                    }
                }
                D88ParseResult::UnsupportedTooManyImages { total_found, .. } => {
                    tracing::warn!(
                        "Skipping multi-D88 unpack for {}: container has {} images (supported max {})",
                        fpath.display(),
                        total_found,
                        D88_MAX_CONTAINER_IMAGES
                    );
                }
                D88ParseResult::InvalidOrIncomplete { ref reason } => {
                    tracing::warn!("Skipping D88 unpack for {}: {}", fpath.display(), reason);
                }
            }
        }

        let disk_num = out_paths.len() + 1;
        out_paths.push(fpath.clone());
        out_labels.push(format!("[{}] {}", disk_num, file_name));
    }

    Ok((out_paths, out_labels))
}

fn materialize_multi_d88_file_in(
    source_d88: &Path,
    slices: &[D88ImageSlice],
    base_disk_idx: usize,
    cache_root: &Path,
) -> Result<(Vec<PathBuf>, Vec<String>)> {
    if slices.len() < 2 || slices.len() > D88_MAX_CONTAINER_IMAGES {
        anyhow::bail!(
            "Refusing to materialize D88 container {} with {} slices",
            source_d88.display(),
            slices.len()
        );
    }

    let raw_bytes = fs::read(source_d88)
        .with_context(|| format!("Failed to read multi-D88 file {}", source_d88.display()))?;
    match inspect_d88_bytes(&raw_bytes) {
        D88ParseResult::Complete(ref parsed) if parsed == slices => {}
        other => {
            anyhow::bail!(
                "Multi-D88 file {} did not pass complete container validation ({:?})",
                source_d88.display(),
                other
            );
        }
    }

    let canonical_source = fs::canonicalize(source_d88).with_context(|| {
        format!(
            "Failed to canonicalize multi-D88 source path {}",
            source_d88.display()
        )
    })?;
    let canonical_source_str = canonical_source.to_string_lossy().to_string();
    let hash_hex = cache_entry_dir_name(&canonical_source);

    let cache_dir = cache_root.join(&hash_hex);
    fs::create_dir_all(&cache_dir).with_context(|| {
        format!(
            "Failed to create multi-D88 cache directory {}",
            cache_dir.display()
        )
    })?;

    let stem = source_d88
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("game");
    let file_name = source_d88
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("game.d88");

    let current_source_fp = fingerprint_bytes(&raw_bytes);

    let mut slice_paths = Vec::with_capacity(slices.len());
    let mut slice_labels = Vec::with_capacity(slices.len());
    let mut desired_manifest_slices = Vec::with_capacity(slices.len());

    for (idx, slice) in slices.iter().enumerate() {
        let safe_label = sanitize_label_for_filename(&slice.name);
        let part_name = format!("{}__{:02}_{}.d88", stem, idx + 1, safe_label);
        let part_path = cache_dir.join(&part_name);
        let start = slice.offset as usize;
        let end = (slice.offset + slice.size) as usize;
        let slice_fp = fingerprint_bytes(&raw_bytes[start..end]);

        desired_manifest_slices.push(D88ManifestSlice {
            file_name: part_name,
            label: slice.name.clone(),
            original_fingerprint: slice_fp,
        });

        let disk_num = base_disk_idx + idx + 1;
        let display_label = if slice.name.is_empty() {
            format!("[{}] {} (Disk {})", disk_num, file_name, idx + 1)
        } else {
            format!("[{}] {} ({})", disk_num, slice.name, file_name)
        };
        slice_paths.push(part_path);
        slice_labels.push(display_label);
    }

    let manifest_path = cache_dir.join(D88_MANIFEST_FILE_NAME);
    let existing_manifest = fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|txt| serde_json::from_str::<D88SourceManifest>(&txt).ok())
        .filter(|m| {
            m.version == D88_MANIFEST_VERSION
                && m.source_path == canonical_source_str
                && m.slices.len() == slices.len()
        });

    if let Some(manifest) = existing_manifest {
        let mut all_slices_present_and_valid = true;
        let mut any_cached_slice_modified = false;

        for (idx, m_slice) in manifest.slices.iter().enumerate() {
            if m_slice.file_name != desired_manifest_slices[idx].file_name
                || !is_safe_slice_file_name(&m_slice.file_name)
            {
                all_slices_present_and_valid = false;
                break;
            }
            let part_path = &slice_paths[idx];
            match read_valid_single_d88_slice(part_path) {
                Ok(disk_bytes) => {
                    let disk_fp = fingerprint_bytes(&disk_bytes);
                    if disk_fp != m_slice.original_fingerprint {
                        any_cached_slice_modified = true;
                    }
                }
                Err(_) => {
                    all_slices_present_and_valid = false;
                }
            }
        }

        if manifest.source_fingerprint == current_source_fp {
            if all_slices_present_and_valid {
                return Ok((slice_paths, slice_labels));
            }
            if any_cached_slice_modified {
                anyhow::bail!(
                    "Cached multi-D88 entry for {} has missing/corrupt slice(s) alongside unsynced modifications",
                    canonical_source.display()
                );
            }
        } else if any_cached_slice_modified {
            anyhow::bail!(
                "Conflict detected for {}: source D88 was modified externally while cached disk slice(s) have unsynced modifications",
                canonical_source.display()
            );
        }
    }

    for (idx, slice) in slices.iter().enumerate() {
        let start = slice.offset as usize;
        let end = (slice.offset + slice.size) as usize;
        write_file_atomic(&slice_paths[idx], &raw_bytes[start..end])
            .with_context(|| format!("Failed to write D88 slice {}", slice_paths[idx].display()))?;
    }

    let new_manifest = D88SourceManifest {
        version: D88_MANIFEST_VERSION,
        source_path: canonical_source_str,
        source_fingerprint: current_source_fp,
        slices: desired_manifest_slices,
    };
    let manifest_bytes =
        serde_json::to_vec_pretty(&new_manifest).context("Failed to serialize D88 manifest")?;
    write_file_atomic(&manifest_path, &manifest_bytes)
        .with_context(|| format!("Failed to write D88 manifest {}", manifest_path.display()))?;

    Ok((slice_paths, slice_labels))
}

fn is_safe_slice_file_name(name: &str) -> bool {
    if name.is_empty() || name.starts_with('.') || name.contains('/') || name.contains('\\') {
        return false;
    }
    let mut comps = Path::new(name).components();
    matches!(comps.next(), Some(Component::Normal(_))) && comps.next().is_none()
}

fn read_valid_single_d88_slice(path: &Path) -> Result<Vec<u8>> {
    let sym_meta = fs::symlink_metadata(path)
        .with_context(|| format!("Failed to stat D88 slice {}", path.display()))?;
    if sym_meta.file_type().is_symlink() || !sym_meta.is_file() {
        anyhow::bail!("D88 slice {} is not a regular file", path.display());
    }
    let bytes =
        fs::read(path).with_context(|| format!("Failed to read D88 slice {}", path.display()))?;
    if (bytes.len() as u64) < D88_MIN_IMAGE_SIZE || bytes.len() > u32::MAX as usize {
        anyhow::bail!(
            "D88 slice {} has invalid byte length {}",
            path.display(),
            bytes.len()
        );
    }
    Ok(bytes)
}

fn write_file_atomic(target: &Path, bytes: &[u8]) -> Result<()> {
    let parent = target
        .parent()
        .context("Target path has no parent directory")?;
    let file_name = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("d88.bin");
    let tmp_name = format!(".{}.michadame-{}.tmp", file_name, std::process::id());
    let tmp_path = parent.join(tmp_name);

    let write_res = (|| -> Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)
            .with_context(|| format!("Failed to create temporary file {}", tmp_path.display()))?;
        f.write_all(bytes)
            .with_context(|| format!("Failed to write temporary file {}", tmp_path.display()))?;
        f.flush()
            .with_context(|| format!("Failed to flush temporary file {}", tmp_path.display()))?;
        f.sync_all()
            .with_context(|| format!("Failed to sync temporary file {}", tmp_path.display()))?;
        Ok(())
    })();

    if let Err(err) = write_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    if let Err(err) = fs::rename(&tmp_path, target) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err).with_context(|| {
            format!(
                "Failed to atomically publish {} over {}",
                tmp_path.display(),
                target.display()
            )
        });
    }

    if let Ok(dir_file) = fs::File::open(parent) {
        let _ = dir_file.sync_all();
    }

    Ok(())
}

/// Checks if any floppy paths in `floppies` are materialized slices of a multi-image `.d88` file,
/// and if any slice was modified by the emulator (e.g. saving to a User disk), re-packs and writes
/// the updated multi-image `.d88` back to its original path, logging any error.
pub fn sync_back_multi_d88_files(floppies: &[PathBuf]) {
    if let Err(err) = try_sync_back_multi_d88_files(floppies) {
        tracing::warn!("Failed to sync back multi-disk D88 file: {:#}", err);
    }
}

/// Fallible variant of `sync_back_multi_d88_files` using the default PC-88 disk cache directory.
pub fn try_sync_back_multi_d88_files(floppies: &[PathBuf]) -> Result<()> {
    try_sync_back_multi_d88_files_in(floppies, &super::core_sync::pc88_disk_cache_dir())
}

/// Fallible variant of `sync_back_multi_d88_files` using an explicit `cache_root`.
pub fn try_sync_back_multi_d88_files_in(floppies: &[PathBuf], cache_root: &Path) -> Result<()> {
    try_sync_back_multi_d88_files_with_hook(floppies, cache_root, &mut |_| Ok(()))
}

fn try_sync_back_multi_d88_files_with_hook<F>(
    floppies: &[PathBuf],
    cache_root: &Path,
    before_publish_hook: &mut F,
) -> Result<()>
where
    F: FnMut(&Path) -> Result<()>,
{
    let canonical_cache_root = fs::canonicalize(cache_root).ok();
    let mut visited_cache_dirs = HashSet::new();

    for path in floppies {
        let Some(parent) = path.parent() else {
            continue;
        };
        let manifest_path = parent.join(D88_MANIFEST_FILE_NAME);
        let has_manifest = fs::symlink_metadata(&manifest_path).is_ok();

        let canonical_parent = fs::canonicalize(parent).ok();
        let is_owned_cache_dir = match (&canonical_parent, &canonical_cache_root) {
            (Some(cp), Some(cr)) => cp.parent() == Some(cr.as_path()),
            _ => false,
        };

        if !is_owned_cache_dir {
            if has_manifest {
                anyhow::bail!(
                    "Refusing to trust D88 manifest outside owned cache root: {}",
                    manifest_path.display()
                );
            }
            continue;
        }

        let cache_dir = canonical_parent.unwrap();
        if !visited_cache_dirs.insert(cache_dir.clone()) {
            continue;
        }

        sync_back_single_cache_dir(&cache_dir, before_publish_hook)?;
    }

    Ok(())
}

fn sync_back_single_cache_dir<F>(cache_dir: &Path, before_publish_hook: &mut F) -> Result<()>
where
    F: FnMut(&Path) -> Result<()>,
{
    let manifest_path = cache_dir.join(D88_MANIFEST_FILE_NAME);
    let manifest_meta = fs::symlink_metadata(&manifest_path).with_context(|| {
        format!(
            "Missing D88 manifest in owned cache directory {}",
            manifest_path.display()
        )
    })?;
    if manifest_meta.file_type().is_symlink() || !manifest_meta.is_file() {
        anyhow::bail!(
            "D88 manifest {} is not a regular file",
            manifest_path.display()
        );
    }

    let manifest_text = fs::read_to_string(&manifest_path)
        .with_context(|| format!("Failed to read D88 manifest {}", manifest_path.display()))?;
    let mut manifest: D88SourceManifest = serde_json::from_str(&manifest_text)
        .with_context(|| format!("Malformed D88 manifest {}", manifest_path.display()))?;

    if manifest.version != D88_MANIFEST_VERSION {
        anyhow::bail!(
            "Unsupported D88 manifest version {} in {}",
            manifest.version,
            manifest_path.display()
        );
    }
    if manifest.slices.len() < 2 || manifest.slices.len() > D88_MAX_CONTAINER_IMAGES {
        anyhow::bail!(
            "Invalid slice count {} in D88 manifest {}",
            manifest.slices.len(),
            manifest_path.display()
        );
    }
    if manifest.source_path.trim().is_empty() {
        anyhow::bail!(
            "Empty source_path in D88 manifest {}",
            manifest_path.display()
        );
    }

    for slice in &manifest.slices {
        if !is_safe_slice_file_name(&slice.file_name) {
            anyhow::bail!(
                "Unsafe slice file name {:?} in D88 manifest {}",
                slice.file_name,
                manifest_path.display()
            );
        }
    }

    let source_path = PathBuf::from(&manifest.source_path);
    let expected_dir_name = cache_entry_dir_name(&source_path);
    let actual_dir_name = cache_dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if actual_dir_name != expected_dir_name {
        anyhow::bail!(
            "D88 manifest source_path {} does not match cache entry directory {}",
            source_path.display(),
            actual_dir_name
        );
    }

    let mut reassembled = Vec::new();
    let mut updated_slice_fps = Vec::with_capacity(manifest.slices.len());
    let mut any_slice_modified = false;

    for m_slice in &manifest.slices {
        let part_path = cache_dir.join(&m_slice.file_name);
        let mut bytes = read_valid_single_d88_slice(&part_path)?;
        let raw_fp = fingerprint_bytes(&bytes);
        if raw_fp != m_slice.original_fingerprint {
            any_slice_modified = true;
        }

        let len_u32 = bytes.len() as u32;
        bytes[0x1c..0x20].copy_from_slice(&len_u32.to_le_bytes());

        match inspect_d88_bytes(&bytes) {
            D88ParseResult::Complete(ref parsed) if parsed.len() == 1 => {}
            other => {
                anyhow::bail!(
                    "Cached D88 slice {} is not a valid single-disk D88 image ({:?})",
                    part_path.display(),
                    other
                );
            }
        }

        let final_slice_fp = fingerprint_bytes(&bytes);
        if final_slice_fp != raw_fp {
            write_file_atomic(&part_path, &bytes)?;
        }
        updated_slice_fps.push(final_slice_fp);
        reassembled.extend_from_slice(&bytes);
    }

    // If no cached slice was modified by the emulator, never overwrite the source file.
    if !any_slice_modified {
        return Ok(());
    }

    let source_sym = fs::symlink_metadata(&source_path).with_context(|| {
        format!(
            "Source D88 file no longer exists at {}",
            source_path.display()
        )
    })?;
    if source_sym.file_type().is_symlink() || !source_sym.is_file() {
        anyhow::bail!(
            "Source D88 path {} is not a regular file",
            source_path.display()
        );
    }
    let canonical_source = fs::canonicalize(&source_path).with_context(|| {
        format!(
            "Failed to canonicalize source D88 path {}",
            source_path.display()
        )
    })?;
    if canonical_source != source_path || cache_entry_dir_name(&canonical_source) != actual_dir_name
    {
        anyhow::bail!(
            "Canonical source D88 path {} no longer matches manifest {}",
            canonical_source.display(),
            source_path.display()
        );
    }

    let orig_bytes = fs::read(&canonical_source).with_context(|| {
        format!(
            "Failed to read original source D88 {}",
            canonical_source.display()
        )
    })?;
    let disk_source_fp = fingerprint_bytes(&orig_bytes);
    if disk_source_fp != manifest.source_fingerprint {
        anyhow::bail!(
            "Conflict detected for {}: source D88 was modified externally while cached disk slice(s) were also modified",
            canonical_source.display()
        );
    }

    match inspect_d88_bytes(&reassembled) {
        D88ParseResult::Complete(ref parsed) if parsed.len() == manifest.slices.len() => {}
        other => {
            anyhow::bail!(
                "Reassembled multi-D88 container for {} failed validation ({:?})",
                canonical_source.display(),
                other
            );
        }
    }

    let backup_path = cache_dir.join(D88_BACKUP_FILE_NAME);
    write_file_atomic(&backup_path, &orig_bytes).with_context(|| {
        format!(
            "Failed to preserve backup copy at {}",
            backup_path.display()
        )
    })?;

    tracing::info!(
        "Syncing modified multi-disk D88 save data back to {}",
        canonical_source.display()
    );

    publish_source_file_with_hook(&canonical_source, &reassembled, before_publish_hook)?;

    manifest.source_fingerprint = fingerprint_bytes(&reassembled);
    for (m_slice, new_fp) in manifest.slices.iter_mut().zip(updated_slice_fps) {
        m_slice.original_fingerprint = new_fp;
    }
    let updated_manifest_bytes =
        serde_json::to_vec_pretty(&manifest).context("Failed to serialize updated D88 manifest")?;
    write_file_atomic(&manifest_path, &updated_manifest_bytes)?;

    Ok(())
}

fn publish_source_file_with_hook<F>(
    target: &Path,
    bytes: &[u8],
    before_publish_hook: &mut F,
) -> Result<()>
where
    F: FnMut(&Path) -> Result<()>,
{
    let parent = target
        .parent()
        .context("Source D88 path has no parent directory")?;
    let file_name = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("game.d88");
    let tmp_path = parent.join(format!(
        ".{}.michadame-sync-{}.tmp",
        file_name,
        std::process::id()
    ));

    let write_res = (|| -> Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)
            .with_context(|| {
                format!("Failed to create temporary D88 file {}", tmp_path.display())
            })?;
        f.write_all(bytes).with_context(|| {
            format!("Failed to write temporary D88 file {}", tmp_path.display())
        })?;
        f.flush().with_context(|| {
            format!("Failed to flush temporary D88 file {}", tmp_path.display())
        })?;
        f.sync_all()
            .with_context(|| format!("Failed to sync temporary D88 file {}", tmp_path.display()))?;
        Ok(())
    })();

    if let Err(err) = write_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    if let Err(err) = before_publish_hook(&tmp_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    if let Err(err) = fs::rename(&tmp_path, target) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err).with_context(|| {
            format!(
                "Failed to atomically publish {} over {}",
                tmp_path.display(),
                target.display()
            )
        });
    }

    if let Ok(dir_file) = fs::File::open(parent) {
        let _ = dir_file.sync_all();
    }

    Ok(())
}

/// Filters and ranks `games` against `query`, returning indices into `games`.
pub fn filter_and_rank_games(games: &[Pc88GameEntry], query: &str) -> Vec<usize> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return (0..games.len()).collect();
    }

    let mut scored: Vec<(usize, i32)> = games
        .iter()
        .enumerate()
        .filter_map(|(idx, game)| {
            match_search_index_score(&game.search_index, trimmed).map(|s| (idx, s))
        })
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored.into_iter().map(|(idx, _)| idx).collect()
}

/// Scans a root games directory (such as `/home/morg/Games/PC88/GameList`) for PC-88 games.
/// Supports both flat game directories (`root/<GameTitle>/...`) and categorized directories
/// (`root/<Category>/<GameTitle>/...`, e.g. `0-9`, `A`..`Z`, `あ`..`ん`, `PC-8001`).
pub fn scan_games_directory(root_dir: &Path) -> Result<Vec<Pc88GameEntry>> {
    if !root_dir.exists() {
        anyhow::bail!("Games directory does not exist: {}", root_dir.display());
    }
    if !root_dir.is_dir() {
        anyhow::bail!("Path is not a directory: {}", root_dir.display());
    }

    let mut entries = Vec::new();
    scan_dir_recursive(root_dir, root_dir, None, 0, &mut entries)?;

    entries.sort_by(|a, b| {
        a.title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });

    Ok(entries)
}

fn scan_dir_recursive(
    root_dir: &Path,
    current_dir: &Path,
    category: Option<&str>,
    depth: usize,
    out: &mut Vec<Pc88GameEntry>,
) -> Result<()> {
    if depth > 4 {
        return Ok(());
    }

    let read_dir = fs::read_dir(current_dir)
        .with_context(|| format!("Failed to read directory {}", current_dir.display()))?;

    let mut files = Vec::new();
    let mut subdirs = Vec::new();

    for item in read_dir.flatten() {
        let path = item.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            subdirs.push(path);
        } else if path.is_file() {
            files.push(path);
        }
    }

    let mut floppies = Vec::new();
    let mut tapes = Vec::new();

    for f in &files {
        if is_pc88_floppy_path(f) {
            floppies.push(f.clone());
        } else if is_pc88_tape_path(f) {
            tapes.push(f.clone());
        }
    }

    // Also check one level of immediate subdirectories for media if this is already a game folder
    if depth >= 1 && (!floppies.is_empty() || !tapes.is_empty()) {
        for sub in &subdirs {
            if let Ok(sub_rd) = fs::read_dir(sub) {
                for item in sub_rd.flatten() {
                    let p = item.path();
                    if !p.is_file() {
                        continue;
                    }
                    if is_pc88_floppy_path(&p) {
                        floppies.push(p);
                    } else if is_pc88_tape_path(&p) {
                        tapes.push(p);
                    }
                }
            }
        }
    }

    let has_media = !floppies.is_empty() || !tapes.is_empty();

    if has_media && current_dir != root_dir {
        sort_media_paths(&mut floppies);
        sort_media_paths(&mut tapes);

        let boot_mode = if !floppies.is_empty() {
            Pc88BootMode::Floppy
        } else {
            Pc88BootMode::Tape
        };

        let rel_id = current_dir
            .strip_prefix(root_dir)
            .unwrap_or(current_dir)
            .to_string_lossy()
            .to_string();

        let title = current_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&rel_id)
            .to_string();

        let search_index = Pc98SearchIndex::build(&title, category, &floppies, &[], &tapes);
        out.push(Pc88GameEntry {
            id: rel_id,
            title,
            category: category.map(|s| s.to_string()),
            folder_path: current_dir.to_path_buf(),
            floppies,
            tapes,
            boot_mode,
            search_index,
        });
        return Ok(());
    }

    subdirs.sort();
    for sub in subdirs {
        let sub_name = sub
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let next_category = if current_dir == root_dir && is_category_folder_name(&sub_name) {
            Some(sub_name)
        } else {
            category.map(|s| s.to_string())
        };
        scan_dir_recursive(root_dir, &sub, next_category.as_deref(), depth + 1, out)?;
    }

    Ok(())
}

/// Detects whether a top-level folder name is an index/category bucket like `"A"`, `"あ"`, `"0-9"`, `"PC-8001"`.
fn is_category_folder_name(name: &str) -> bool {
    let trimmed = name.trim();
    let char_count = trimmed.chars().count();
    if char_count == 1 {
        return true;
    }
    matches!(
        trimmed.to_ascii_uppercase().as_str(),
        "0-9" | "00-99" | "ETC" | "OTHER" | "OTHERS" | "PC-8001" | "PC8001" | "PC-8801" | "PC8801"
    )
}

/// Sorts disk/tape paths using natural alphanumeric order while deprioritizing utility/user/save disks
/// so that Boot/System/Disk A/Disk 1 always land in FDD1 and Disk B/Disk 2 land in FDD2.
fn sort_media_paths(paths: &mut [PathBuf]) {
    paths.sort_by(|a, b| {
        let a_name = a
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        let b_name = b
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        let a_prio = disk_boot_priority(&a_name);
        let b_prio = disk_boot_priority(&b_name);
        a_prio
            .cmp(&b_prio)
            .then_with(|| natural_cmp(&a_name, &b_name))
    });
}

fn disk_boot_priority(stem_lower: &str) -> u8 {
    if stem_lower.contains("user")
        || stem_lower.contains("save")
        || stem_lower.contains("blank")
        || stem_lower.contains("omake")
        || stem_lower.contains("ユーザー")
        || stem_lower.contains("セーブ")
    {
        return 2;
    }
    0
}

fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ia = a.chars().peekable();
    let mut ib = b.chars().peekable();

    loop {
        match (ia.peek(), ib.peek()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(&ca), Some(&cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let mut na = 0u64;
                    while let Some(&d) = ia.peek() {
                        if d.is_ascii_digit() {
                            na = na
                                .saturating_mul(10)
                                .saturating_add((d as u8 - b'0') as u64);
                            ia.next();
                        } else {
                            break;
                        }
                    }
                    let mut nb = 0u64;
                    while let Some(&d) = ib.peek() {
                        if d.is_ascii_digit() {
                            nb = nb
                                .saturating_mul(10)
                                .saturating_add((d as u8 - b'0') as u64);
                            ib.next();
                        } else {
                            break;
                        }
                    }
                    let ord = na.cmp(&nb);
                    if ord != std::cmp::Ordering::Equal {
                        return ord;
                    }
                } else {
                    let ord = ca.cmp(&cb);
                    if ord != std::cmp::Ordering::Equal {
                        return ord;
                    }
                    ia.next();
                    ib.next();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn make_d88_slice(label: &str, fill: u8) -> Vec<u8> {
        let size = 0x300usize;
        let mut buf = vec![fill; size];
        for b in &mut buf[..0x2b0] {
            *b = 0;
        }
        let lbl_bytes = label.as_bytes();
        let copy_len = lbl_bytes.len().min(16);
        buf[..copy_len].copy_from_slice(&lbl_bytes[..copy_len]);
        let sz_u32 = size as u32;
        buf[0x1c..0x20].copy_from_slice(&sz_u32.to_le_bytes());
        buf
    }

    fn make_game_entry(folder: &Path, d88_path: &Path) -> Pc88GameEntry {
        Pc88GameEntry {
            id: "T/testgame".to_string(),
            title: "testgame".to_string(),
            category: Some("T".to_string()),
            folder_path: folder.to_path_buf(),
            floppies: vec![d88_path.to_path_buf()],
            tapes: Vec::new(),
            boot_mode: Pc88BootMode::Floppy,
            search_index: Pc98SearchIndex::build(
                "testgame",
                Some("T"),
                &[d88_path.to_path_buf()],
                &[],
                &[],
            ),
        }
    }

    #[test]
    fn test_normalized_extension_handles_fullwidth_d88() {
        let p = Path::new("/tmp/game.ｄ88");
        assert_eq!(normalized_extension(p).as_deref(), Some("d88"));
        assert!(is_pc88_floppy_path(p));
    }

    #[test]
    fn test_multi_d88_materialize_and_sync_back() {
        let game_dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();

        let mut combined = Vec::new();
        combined.extend_from_slice(&make_d88_slice("DiskA", 0x11));
        combined.extend_from_slice(&make_d88_slice("DiskB", 0x22));

        let d88_path = game_dir.path().join("testgame.d88");
        fs::write(&d88_path, &combined).unwrap();

        let slices = inspect_d88_images(&d88_path);
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].name, "DiskA");
        assert_eq!(slices[1].name, "DiskB");

        let entry = make_game_entry(game_dir.path(), &d88_path);

        let (paths, labels) = materialize_game_floppies_in(&entry, cache_dir.path()).unwrap();
        assert_eq!(paths.len(), 2);
        assert_eq!(labels.len(), 2);
        assert!(labels[0].contains("DiskA"));
        assert!(labels[1].contains("DiskB"));

        // Simulate emulator modifying DiskB
        let mut disk_b_bytes = fs::read(&paths[1]).unwrap();
        disk_b_bytes[0x2b0] = 0x99;
        fs::write(&paths[1], &disk_b_bytes).unwrap();

        try_sync_back_multi_d88_files_in(&paths, cache_dir.path()).unwrap();

        let updated_combined = fs::read(&d88_path).unwrap();
        assert_eq!(updated_combined[0x300 + 0x2b0], 0x99);
    }

    #[test]
    fn test_f02_unchanged_reopen_and_source_only_change_survives() {
        let game_dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();

        let mut combined = Vec::new();
        combined.extend_from_slice(&make_d88_slice("DiskA", 0x00));
        combined.extend_from_slice(&make_d88_slice("DiskB", 0x00));

        let d88_path = game_dir.path().join("probe.d88");
        fs::write(&d88_path, &combined).unwrap();
        let entry = make_game_entry(game_dir.path(), &d88_path);

        // 1. Materialize a two-disk source
        let (paths_first, _) = materialize_game_floppies_in(&entry, cache_dir.path()).unwrap();
        assert_eq!(paths_first.len(), 2);

        // Unchanged reopen + sync back does not rewrite source
        let (paths_reopen, _) = materialize_game_floppies_in(&entry, cache_dir.path()).unwrap();
        assert_eq!(paths_reopen, paths_first);
        try_sync_back_multi_d88_files_in(&paths_reopen, cache_dir.path()).unwrap();
        assert_eq!(fs::read(&d88_path).unwrap(), combined);

        // 2. External source-only edit: change a byte in the original source to 99
        let mut edited_source = fs::read(&d88_path).unwrap();
        edited_source[0x2b0] = 99;
        fs::write(&d88_path, &edited_source).unwrap();

        //Even if sync_back runs before re-materializing (while cache is unmodified), source edit survives
        try_sync_back_multi_d88_files_in(&paths_first, cache_dir.path()).unwrap();
        assert_eq!(fs::read(&d88_path).unwrap()[0x2b0], 99);

        // 3. Materialize again, and sync back without modifying the cached disks
        let (paths_second, _) = materialize_game_floppies_in(&entry, cache_dir.path()).unwrap();
        assert_eq!(fs::read(&paths_second[0]).unwrap()[0x2b0], 99);
        try_sync_back_multi_d88_files_in(&paths_second, cache_dir.path()).unwrap();

        // Source byte must still be 99 (never overwritten with stale 0)
        let after_bytes = fs::read(&d88_path).unwrap();
        assert_eq!(after_bytes[0x2b0], 99);
        assert_eq!(after_bytes, edited_source);
    }

    #[test]
    fn test_f02_conflict_both_source_and_cache_modified_is_rejected() {
        let game_dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();

        let mut combined = Vec::new();
        combined.extend_from_slice(&make_d88_slice("DiskA", 0x10));
        combined.extend_from_slice(&make_d88_slice("DiskB", 0x20));

        let d88_path = game_dir.path().join("conflict.d88");
        fs::write(&d88_path, &combined).unwrap();
        let entry = make_game_entry(game_dir.path(), &d88_path);

        let (paths, _) = materialize_game_floppies_in(&entry, cache_dir.path()).unwrap();

        // Modify cached slice 1 (emulator save)
        let mut cached_b = fs::read(&paths[1]).unwrap();
        cached_b[0x2b0] = 0xAA;
        fs::write(&paths[1], &cached_b).unwrap();

        // Also modify original source externally
        let mut external_source = fs::read(&d88_path).unwrap();
        external_source[0x2b0] = 0xBB;
        fs::write(&d88_path, &external_source).unwrap();

        // Both sync-back and re-materialize must detect the conflict and refuse to overwrite either
        let sync_err = try_sync_back_multi_d88_files_in(&paths, cache_dir.path()).unwrap_err();
        assert!(
            sync_err.to_string().contains("Conflict"),
            "Unexpected error: {:#}",
            sync_err
        );
        assert_eq!(fs::read(&d88_path).unwrap(), external_source);
        assert_eq!(fs::read(&paths[1]).unwrap(), cached_b);

        let mat_err = materialize_game_floppies_in(&entry, cache_dir.path()).unwrap_err();
        assert!(
            mat_err.to_string().contains("Conflict"),
            "Unexpected error: {:#}",
            mat_err
        );
        assert_eq!(fs::read(&d88_path).unwrap(), external_source);
        assert_eq!(fs::read(&paths[1]).unwrap(), cached_b);
    }

    #[test]
    fn test_f02_moved_source_and_unowned_or_malformed_manifests_rejected() {
        let game_dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();

        let mut combined = Vec::new();
        combined.extend_from_slice(&make_d88_slice("DiskA", 0x11));
        combined.extend_from_slice(&make_d88_slice("DiskB", 0x22));

        let d88_path = game_dir.path().join("movable.d88");
        fs::write(&d88_path, &combined).unwrap();
        let entry = make_game_entry(game_dir.path(), &d88_path);

        let (paths, _) = materialize_game_floppies_in(&entry, cache_dir.path()).unwrap();

        // Modify cached slice 0, then move source file
        let mut slice0 = fs::read(&paths[0]).unwrap();
        slice0[0x2b0] = 0x55;
        fs::write(&paths[0], &slice0).unwrap();

        let moved_path = game_dir.path().join("moved.d88");
        fs::rename(&d88_path, &moved_path).unwrap();

        let err = try_sync_back_multi_d88_files_in(&paths, cache_dir.path()).unwrap_err();
        assert!(
            err.to_string().contains("no longer exists"),
            "Expected missing source error, got: {:#}",
            err
        );
        assert!(!d88_path.exists());
        assert_eq!(fs::read(&moved_path).unwrap(), combined);

        // Restore source file and verify path-escaping slice name in manifest is rejected
        fs::rename(&moved_path, &d88_path).unwrap();
        let entry_cache_dir = paths[0].parent().unwrap().to_path_buf();
        let manifest_path = entry_cache_dir.join(D88_MANIFEST_FILE_NAME);
        let orig_manifest_bytes = fs::read(&manifest_path).unwrap();

        let mut evil_manifest: D88SourceManifest =
            serde_json::from_slice(&orig_manifest_bytes).unwrap();
        evil_manifest.slices[0].file_name = "../evil.d88".to_string();
        fs::write(&manifest_path, serde_json::to_vec(&evil_manifest).unwrap()).unwrap();

        let escape_err = try_sync_back_multi_d88_files_in(&paths, cache_dir.path()).unwrap_err();
        assert!(
            escape_err.to_string().contains("Unsafe slice file name"),
            "Unexpected error: {:#}",
            escape_err
        );

        // Malformed non-JSON manifest in owned cache dir is rejected
        fs::write(&manifest_path, b"not valid json").unwrap();
        assert!(try_sync_back_multi_d88_files_in(&paths, cache_dir.path()).is_err());

        // Unowned manifest located beside a regular floppy outside cache_root is rejected
        let unowned_dir = tempfile::tempdir().unwrap();
        let unowned_floppy = unowned_dir.path().join("disk1.d88");
        fs::write(&unowned_floppy, make_d88_slice("Single", 0x33)).unwrap();
        fs::write(
            unowned_dir.path().join(D88_MANIFEST_FILE_NAME),
            &orig_manifest_bytes,
        )
        .unwrap();
        let unowned_err =
            try_sync_back_multi_d88_files_in(&[unowned_floppy], cache_dir.path()).unwrap_err();
        assert!(
            unowned_err.to_string().contains("outside owned cache root"),
            "Unexpected error: {:#}",
            unowned_err
        );
        assert_eq!(fs::read(&d88_path).unwrap(), combined);
    }

    #[test]
    fn test_f02_interruption_before_publication_and_backup_preservation() {
        let game_dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();

        let mut combined = Vec::new();
        combined.extend_from_slice(&make_d88_slice("DiskA", 0x11));
        combined.extend_from_slice(&make_d88_slice("DiskB", 0x22));

        let d88_path = game_dir.path().join("atomic.d88");
        fs::write(&d88_path, &combined).unwrap();
        let entry = make_game_entry(game_dir.path(), &d88_path);

        let (paths, _) = materialize_game_floppies_in(&entry, cache_dir.path()).unwrap();
        let entry_cache_dir = paths[0].parent().unwrap().to_path_buf();
        let orig_manifest =
            fs::read_to_string(entry_cache_dir.join(D88_MANIFEST_FILE_NAME)).unwrap();

        // Modify cached DiskB
        let mut disk_b = fs::read(&paths[1]).unwrap();
        disk_b[0x2b0] = 0x77;
        fs::write(&paths[1], &disk_b).unwrap();

        // Inject an interruption right before rename publication
        let mut observed_tmp_exists = false;
        let interrupted =
            try_sync_back_multi_d88_files_with_hook(&paths, cache_dir.path(), &mut |tmp_path| {
                observed_tmp_exists = tmp_path.is_file();
                anyhow::bail!("simulated interruption before atomic rename")
            });
        assert!(interrupted.is_err());
        assert!(observed_tmp_exists);

        // Source and manifest must remain at their pre-sync generation, and backup must be preserved
        assert_eq!(fs::read(&d88_path).unwrap(), combined);
        assert_eq!(
            fs::read(entry_cache_dir.join(D88_BACKUP_FILE_NAME)).unwrap(),
            combined
        );
        assert_eq!(
            fs::read_to_string(entry_cache_dir.join(D88_MANIFEST_FILE_NAME)).unwrap(),
            orig_manifest
        );

        // Retrying without interruption completes publication and updates source + manifest
        try_sync_back_multi_d88_files_in(&paths, cache_dir.path()).unwrap();
        let updated_source = fs::read(&d88_path).unwrap();
        assert_eq!(updated_source[0x300 + 0x2b0], 0x77);
        assert_eq!(
            fs::read(entry_cache_dir.join(D88_BACKUP_FILE_NAME)).unwrap(),
            combined
        );
    }

    #[test]
    fn test_f03_inspect_d88_container_counts_and_33_image_nondestructive() {
        let game_dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();

        for count in [1usize, 2, 32] {
            let mut bytes = Vec::new();
            for i in 0..count {
                bytes.extend_from_slice(&make_d88_slice(&format!("Disk{}", i + 1), i as u8));
            }
            let p = game_dir.path().join(format!("valid_{}.d88", count));
            fs::write(&p, &bytes).unwrap();
            match inspect_d88_container(&p) {
                D88ParseResult::Complete(slices) => assert_eq!(slices.len(), count),
                other => panic!(
                    "Expected Complete({}) for {}, got {:?}",
                    count,
                    p.display(),
                    other
                ),
            }
            assert_eq!(inspect_d88_images(&p).len(), count);
        }

        // 33 synthetic disk images (33 * 768 = 25,344 bytes, reproducing the F03 report probe)
        let mut bytes_33 = Vec::new();
        for i in 0..33usize {
            bytes_33.extend_from_slice(&make_d88_slice(&format!("Disk{}", i + 1), (i + 1) as u8));
        }
        assert_eq!(bytes_33.len(), 25_344);

        let path_33 = game_dir.path().join("container33.d88");
        fs::write(&path_33, &bytes_33).unwrap();

        match inspect_d88_container(&path_33) {
            D88ParseResult::UnsupportedTooManyImages {
                parsed_prefix,
                total_found,
            } => {
                assert_eq!(parsed_prefix.len(), 32);
                assert_eq!(total_found, 33);
            }
            other => panic!("Expected UnsupportedTooManyImages, got {:?}", other),
        }
        assert!(inspect_d88_images(&path_33).is_empty());

        let entry_33 = make_game_entry(game_dir.path(), &path_33);
        let (mat_paths, _) = materialize_game_floppies_in(&entry_33, cache_dir.path()).unwrap();
        assert_eq!(mat_paths, vec![path_33.clone()]);

        try_sync_back_multi_d88_files_in(&mat_paths, cache_dir.path()).unwrap();
        let after_33 = fs::read(&path_33).unwrap();
        assert_eq!(after_33.len(), 25_344);
        assert_eq!(after_33, bytes_33);
    }

    struct FailingReader {
        inner: Cursor<Vec<u8>>,
        fail_at_offset: u64,
    }

    impl Read for FailingReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.inner.position() >= self.fail_at_offset {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "simulated read failure",
                ));
            }
            self.inner.read(buf)
        }
    }

    impl Seek for FailingReader {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    #[test]
    fn test_f03_corrupt_later_header_trailing_bytes_truncated_and_read_error() {
        let game_dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();

        // 1. Corrupt second header (dsize < 0x2b0)
        let mut corrupt_second = make_d88_slice("Disk1", 0x11);
        let mut bad_second = make_d88_slice("Disk2", 0x22);
        bad_second[0x1c..0x20].copy_from_slice(&16u32.to_le_bytes());
        corrupt_second.extend_from_slice(&bad_second);
        let p_corrupt = game_dir.path().join("corrupt_second.d88");
        fs::write(&p_corrupt, &corrupt_second).unwrap();

        // 2. Trailing unconsumed bytes (< 32 bytes after valid slices)
        let mut trailing_short = make_d88_slice("Disk1", 0x11);
        trailing_short.extend_from_slice(&make_d88_slice("Disk2", 0x22));
        trailing_short.extend_from_slice(b"TRAILING_BYTES");
        let p_trailing = game_dir.path().join("trailing.d88");
        fs::write(&p_trailing, &trailing_short).unwrap();

        // 3. Truncated second image
        let mut truncated = make_d88_slice("Disk1", 0x11);
        let second = make_d88_slice("Disk2", 0x22);
        truncated.extend_from_slice(&second[..0x2c0]);
        let p_truncated = game_dir.path().join("truncated.d88");
        fs::write(&p_truncated, &truncated).unwrap();

        for (p, orig) in [
            (&p_corrupt, &corrupt_second),
            (&p_trailing, &trailing_short),
            (&p_truncated, &truncated),
        ] {
            assert!(matches!(
                inspect_d88_container(p),
                D88ParseResult::InvalidOrIncomplete { .. }
            ));
            assert!(inspect_d88_images(p).is_empty());

            let entry = make_game_entry(game_dir.path(), p);
            let (paths, _) = materialize_game_floppies_in(&entry, cache_dir.path()).unwrap();
            assert_eq!(paths, vec![p.clone()]);
            try_sync_back_multi_d88_files_in(&paths, cache_dir.path()).unwrap();
            assert_eq!(&fs::read(p).unwrap(), orig);
        }

        // 4. Simulated read error on second slice
        let mut valid_two = make_d88_slice("Disk1", 0x11);
        valid_two.extend_from_slice(&make_d88_slice("Disk2", 0x22));
        let total_len = valid_two.len() as u64;
        let mut reader = FailingReader {
            inner: Cursor::new(valid_two),
            fail_at_offset: 0x300,
        };
        assert!(matches!(
            inspect_d88_reader(&mut reader, total_len),
            D88ParseResult::InvalidOrIncomplete { .. }
        ));
    }

    #[test]
    fn test_scan_synthetic_pc88_gamelist_excludes_utility_roms() {
        let root = tempfile::tempdir().unwrap();

        let game_a = root.path().join("A/AlphaQuest");
        fs::create_dir_all(&game_a).unwrap();
        fs::write(game_a.join("disk1.d88"), make_d88_slice("Alpha1", 0x11)).unwrap();

        let game_tape = root.path().join("PC-8001/TapeHero");
        fs::create_dir_all(&game_tape).unwrap();
        fs::write(game_tape.join("hero.t88"), b"T88_TAPE_DATA").unwrap();

        let rom_dir = root.path().join("R/rom_88fa");
        fs::create_dir_all(&rom_dir).unwrap();
        fs::write(rom_dir.join("N88.ROM"), vec![0u8; 1024]).unwrap();

        let kanji_dir = root.path().join("K/kanji_88");
        fs::create_dir_all(&kanji_dir).unwrap();
        fs::write(kanji_dir.join("KANJI1.ROM"), vec![0u8; 1024]).unwrap();

        let games = scan_games_directory(root.path()).unwrap();
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].title, "AlphaQuest");
        assert_eq!(games[0].category.as_deref(), Some("A"));
        assert_eq!(games[0].boot_mode, Pc88BootMode::Floppy);
        assert_eq!(games[1].title, "TapeHero");
        assert_eq!(games[1].category.as_deref(), Some("PC-8001"));
        assert_eq!(games[1].boot_mode, Pc88BootMode::Tape);
        assert!(!games
            .iter()
            .any(|g| g.title == "rom_88fa" || g.title == "kanji_88"));
    }

    #[test]
    #[ignore = "requires local personal game library"]
    fn test_scan_real_pc88_gamelist_if_present() {
        let real_path = Path::new("/home/morg/Games/PC88/GameList");
        if !real_path.is_dir() {
            return;
        }
        let games =
            scan_games_directory(real_path).expect("Failed to scan /home/morg/Games/PC88/GameList");
        assert!(
            games.len() > 500,
            "Expected > 500 games in /home/morg/Games/PC88/GameList, found {}",
            games.len()
        );
        // Utility ROM-only folders like R/rom_88fa or K/kanji_88 should not be listed as games
        assert!(!games
            .iter()
            .any(|g| g.title == "rom_88fa" || g.title == "kanji_88"));
    }
}
