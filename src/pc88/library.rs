use crate::pc98::search::{match_search_index_score, Pc98SearchIndex};
use anyhow::{Context, Result};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Supported PC-88 floppy image extensions (lowercase).
/// Also handles fullwidth `.ｄ88` via `normalized_extension`.
pub const PC88_FLOPPY_EXTENSIONS: &[&str] = &[
    "d88", "88d", "2hd", "2dd", "fdi", "dsk", "dup",
];

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

/// Normalizes a file extension (handling fullwidth characters like `.ｄ88` -> `d88`).
pub fn normalized_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    let mut out = String::with_capacity(ext.len());
    for ch in ext.chars() {
        let mapped = match ch {
            '\u{FF21}'..='\u{FF3A}' => char::from_u32(ch as u32 - 0xFF21 + b'a' as u32).unwrap_or(ch),
            '\u{FF41}'..='\u{FF5A}' => char::from_u32(ch as u32 - 0xFF41 + b'a' as u32).unwrap_or(ch),
            '\u{FF10}'..='\u{FF19}' => char::from_u32(ch as u32 - 0xFF10 + b'0' as u32).unwrap_or(ch),
            _ => ch.to_ascii_lowercase(),
        };
        out.push(mapped);
    }
    Some(out)
}

pub fn is_pc88_floppy_path(path: &Path) -> bool {
    normalized_extension(path)
        .is_some_and(|ext| PC88_FLOPPY_EXTENSIONS.contains(&ext.as_str()))
}

pub fn is_pc88_tape_path(path: &Path) -> bool {
    normalized_extension(path)
        .is_some_and(|ext| PC88_TAPE_EXTENSIONS.contains(&ext.as_str()))
}

/// Inspects a `.d88` / `.88d` file and returns the list of disk image slices contained inside it.
/// Each `.d88` image starts with a 32-byte header prefix where:
/// - `0x00..0x11` (17 bytes): null-terminated Shift-JIS / ASCII disk label
/// - `0x1c..0x20` (4 bytes, LE u32): total byte size of that disk image
pub fn inspect_d88_images(path: &Path) -> Vec<D88ImageSlice> {
    let Ok(meta) = fs::metadata(path) else {
        return Vec::new();
    };
    let file_len = meta.len();
    if file_len < 0x2b0 {
        return Vec::new();
    }
    let Ok(mut file) = fs::File::open(path) else {
        return Vec::new();
    };

    let mut slices = Vec::new();
    let mut offset: u64 = 0;
    let mut hdr = [0u8; 32];

    while offset + 32 <= file_len {
        if file.seek(SeekFrom::Start(offset)).is_err() {
            break;
        }
        if file.read_exact(&mut hdr).is_err() {
            break;
        }
        let dsize = u32::from_le_bytes([hdr[0x1c], hdr[0x1d], hdr[0x1e], hdr[0x1f]]) as u64;
        if dsize < 0x2b0 || offset + dsize > file_len {
            break;
        }
        let raw_name = hdr[..17].split(|&b| b == 0).next().unwrap_or(&[]);
        let name = decode_d88_ank_name(raw_name);
        slices.push(D88ImageSlice {
            name,
            offset,
            size: dsize,
        });
        offset += dsize;
        if slices.len() >= 32 {
            break;
        }
    }

    slices
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

/// Materializes all floppy disks for a `Pc88GameEntry`.
/// - Single-image `.d88` files (and other floppy formats) are returned directly at their original path.
/// - Multi-image `.d88` container files (`slices.len() > 1`) are unpacked into individual per-disk `.d88`
///   files in `<pc88_disk_cache_dir>/<hash>/` so `quasi88_libretro` can mount Disk 1 in FDD1 and Disk 2 in FDD2
///   simultaneously and swap any disk image live from the UI.
/// - Also writes `.source_d88_manifest` in the cache folder so `sync_back_multi_d88_files` can write any
///   in-game saves back into the original multi-image `.d88` file when the emulator stops or swaps disks.
pub fn materialize_game_floppies(entry: &Pc88GameEntry) -> Result<(Vec<PathBuf>, Vec<String>)> {
    let mut out_paths = Vec::new();
    let mut out_labels = Vec::new();

    for fpath in &entry.floppies {
        let ext = normalized_extension(fpath).unwrap_or_default();
        let file_name = fpath
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("disk.d88");

        if ext == "d88" || ext == "88d" {
            let slices = inspect_d88_images(fpath);
            if slices.len() > 1 {
                let (slice_paths, slice_labels) =
                    materialize_multi_d88_file(fpath, &slices, out_paths.len())?;
                out_paths.extend(slice_paths);
                out_labels.extend(slice_labels);
                continue;
            } else if let Some(first) = slices.first() {
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

        let disk_num = out_paths.len() + 1;
        out_paths.push(fpath.clone());
        out_labels.push(format!("[{}] {}", disk_num, file_name));
    }

    Ok((out_paths, out_labels))
}

fn materialize_multi_d88_file(
    source_d88: &Path,
    slices: &[D88ImageSlice],
    base_disk_idx: usize,
) -> Result<(Vec<PathBuf>, Vec<String>)> {
    let mut hasher = DefaultHasher::new();
    source_d88.to_string_lossy().hash(&mut hasher);
    let hash_hex = format!("{:016x}", hasher.finish());

    let cache_dir = super::core_sync::pc88_disk_cache_dir().join(hash_hex);
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

    let mut slice_paths = Vec::with_capacity(slices.len());
    let mut slice_labels = Vec::with_capacity(slices.len());
    let mut manifest_lines = Vec::with_capacity(slices.len() + 1);
    manifest_lines.push(source_d88.to_string_lossy().to_string());

    let mut need_extract = false;
    for (idx, slice) in slices.iter().enumerate() {
        let safe_label = sanitize_label_for_filename(&slice.name);
        let part_name = format!("{}__{:02}_{}.d88", stem, idx + 1, safe_label);
        let part_path = cache_dir.join(&part_name);
        if !part_path.is_file() {
            need_extract = true;
        }
        manifest_lines.push(part_name);
        let disk_num = base_disk_idx + idx + 1;
        let display_label = if slice.name.is_empty() {
            format!("[{}] {} (Disk {})", disk_num, file_name, idx + 1)
        } else {
            format!("[{}] {} ({})", disk_num, slice.name, file_name)
        };
        slice_paths.push(part_path);
        slice_labels.push(display_label);
    }

    if need_extract {
        let raw_bytes = fs::read(source_d88)
            .with_context(|| format!("Failed to read multi-D88 file {}", source_d88.display()))?;
        for (idx, slice) in slices.iter().enumerate() {
            let start = slice.offset as usize;
            let end = (slice.offset + slice.size) as usize;
            if end <= raw_bytes.len() && !slice_paths[idx].is_file() {
                fs::write(&slice_paths[idx], &raw_bytes[start..end])?;
            }
        }
    }

    let manifest_path = cache_dir.join(".source_d88_manifest");
    let _ = fs::write(&manifest_path, manifest_lines.join("\n"));

    Ok((slice_paths, slice_labels))
}

/// Checks if any floppy paths in `floppies` are materialized slices of a multi-image `.d88` file,
/// and if any slice was modified by the emulator (e.g. saving to a User disk), re-packs and writes
/// the updated multi-image `.d88` back to its original path.
pub fn sync_back_multi_d88_files(floppies: &[PathBuf]) {
    let mut visited_cache_dirs = std::collections::HashSet::new();

    for path in floppies {
        let Some(parent) = path.parent() else {
            continue;
        };
        let manifest_path = parent.join(".source_d88_manifest");
        if !manifest_path.is_file() || !visited_cache_dirs.insert(parent.to_path_buf()) {
            continue;
        }

        let Ok(manifest_text) = fs::read_to_string(&manifest_path) else {
            continue;
        };
        let mut lines = manifest_text.lines();
        let Some(source_path_str) = lines.next() else {
            continue;
        };
        let source_path = PathBuf::from(source_path_str);
        if !source_path.is_file() {
            continue;
        }

        let part_names: Vec<&str> = lines.filter(|l| !l.trim().is_empty()).collect();
        if part_names.len() < 2 {
            continue;
        }

        let mut reassembled = Vec::new();
        let mut all_valid = true;
        for part_name in &part_names {
            let part_path = parent.join(part_name);
            match fs::read(&part_path) {
                Ok(mut bytes) if bytes.len() >= 0x2b0 => {
                    // Ensure the D88 header's internal size field matches the actual slice byte length
                    let len_u32 = bytes.len() as u32;
                    bytes[0x1c..0x20].copy_from_slice(&len_u32.to_le_bytes());
                    reassembled.extend_from_slice(&bytes);
                }
                _ => {
                    all_valid = false;
                    break;
                }
            }
        }

        if all_valid {
            if let Ok(orig_bytes) = fs::read(&source_path) {
                if orig_bytes != reassembled {
                    tracing::info!(
                        "Syncing modified multi-disk D88 save data back to {}",
                        source_path.display()
                    );
                    let _ = fs::write(&source_path, &reassembled);
                }
            }
        }
    }
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
        scan_dir_recursive(
            root_dir,
            &sub,
            next_category.as_deref(),
            depth + 1,
            out,
        )?;
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
                            na = na.saturating_mul(10).saturating_add((d as u8 - b'0') as u64);
                            ia.next();
                        } else {
                            break;
                        }
                    }
                    let mut nb = 0u64;
                    while let Some(&d) = ib.peek() {
                        if d.is_ascii_digit() {
                            nb = nb.saturating_mul(10).saturating_add((d as u8 - b'0') as u64);
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

    #[test]
    fn test_normalized_extension_handles_fullwidth_d88() {
        let p = Path::new("/tmp/game.ｄ88");
        assert_eq!(normalized_extension(p).as_deref(), Some("d88"));
        assert!(is_pc88_floppy_path(p));
    }

    #[test]
    fn test_multi_d88_materialize_and_sync_back() {
        let tmp = std::env::temp_dir().join(format!("michadame_pc88_d88_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let make_d88_slice = |label: &str, fill: u8| -> Vec<u8> {
            let size = 0x300usize;
            let mut buf = vec![fill; size];
            for b in &mut buf[..0x2b0] {
                *b = 0;
            }
            let lbl_bytes = label.as_bytes();
            buf[..lbl_bytes.len().min(16)].copy_from_slice(&lbl_bytes[..lbl_bytes.len().min(16)]);
            let sz_u32 = size as u32;
            buf[0x1c..0x20].copy_from_slice(&sz_u32.to_le_bytes());
            buf
        };

        let mut combined = Vec::new();
        combined.extend_from_slice(&make_d88_slice("DiskA", 0x11));
        combined.extend_from_slice(&make_d88_slice("DiskB", 0x22));

        let d88_path = tmp.join("testgame.d88");
        fs::write(&d88_path, &combined).unwrap();

        let slices = inspect_d88_images(&d88_path);
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].name, "DiskA");
        assert_eq!(slices[1].name, "DiskB");

        let entry = Pc88GameEntry {
            id: "T/testgame".to_string(),
            title: "testgame".to_string(),
            category: Some("T".to_string()),
            folder_path: tmp.clone(),
            floppies: vec![d88_path.clone()],
            tapes: Vec::new(),
            boot_mode: Pc88BootMode::Floppy,
            search_index: Pc98SearchIndex::build("testgame", Some("T"), &[d88_path.clone()], &[], &[]),
        };

        let (paths, labels) = materialize_game_floppies(&entry).unwrap();
        assert_eq!(paths.len(), 2);
        assert_eq!(labels.len(), 2);
        assert!(labels[0].contains("DiskA"));
        assert!(labels[1].contains("DiskB"));

        // Simulate emulator modifying DiskB
        let mut disk_b_bytes = fs::read(&paths[1]).unwrap();
        disk_b_bytes[0x2b0] = 0x99;
        fs::write(&paths[1], &disk_b_bytes).unwrap();

        sync_back_multi_d88_files(&paths);

        let updated_combined = fs::read(&d88_path).unwrap();
        assert_eq!(updated_combined[0x300 + 0x2b0], 0x99);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_scan_real_pc88_gamelist_if_present() {
        let real_path = Path::new("/home/morg/Games/PC88/GameList");
        if !real_path.is_dir() {
            return;
        }
        let games = scan_games_directory(real_path).expect("Failed to scan /home/morg/Games/PC88/GameList");
        assert!(
            games.len() > 500,
            "Expected > 500 games in /home/morg/Games/PC88/GameList, found {}",
            games.len()
        );
        // Utility ROM-only folders like R/rom_88fa or K/kanji_88 should not be listed as games
        assert!(!games.iter().any(|g| g.title == "rom_88fa" || g.title == "kanji_88"));
    }
}
