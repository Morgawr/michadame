use super::core_sync::{resolve_or_extract_media_path, saves_dir};
use super::{Pc98Config, Pc98MediaSlot};
use anyhow::Result;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const FDD_EXTS: &[&str] = &[
    "d88", "88d", "d98", "98d", "fdi", "xdf", "hdm", "dup", "2hd", "tfd", "nfd", "hd4", "hd5",
    "hd9", "fdd", "h01", "hdb", "ddb", "dd6", "dcp", "dcu", "flp", "img", "ima", "fim",
];

pub const HDD_EXTS: &[&str] = &["hdi", "thd", "nhd", "vhd", "slh", "hdn"];

pub const CDROM_EXTS: &[&str] = &["cue", "ccd", "cdm", "mds", "nrg", "iso"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pc98BootMode {
    /// Standalone bootable hard disk image (.hdi / .nhd) mounted in HDD 1,
    /// optionally with companion key/scenario floppies in FDD 1 / FDD 2.
    SelfBootHdd,
    /// Game hard disk image (.hdi / .nhd) paired with a dedicated boot floppy (起動ディスク.fdi, etc.).
    /// Boots from FDD 1 (A:) with the game HDD mounted in HDD 1 (C:).
    FloppyBootWithGameHdd,
    /// Non-bootable game hard disk image (.hdi / .nhd) that requires the base MS-DOS drive
    /// mounted in HDD 1 (A:) and the game HDD mounted in HDD 2 (B:).
    DosHddWithGameHdd,
    /// Self-bootable floppy disk game (.fdi / .d88 / .nfd / .hdm / etc.) mounted in FDD 1 (and FDD 2),
    /// with HDD 1 left empty so the PC-98 BIOS boots directly from FDD 1.
    SelfBootFloppy,
    /// Non-bootable DOS floppy disk game (.fdi / .d88 / .nfd / .hdm / etc.) that requires
    /// the base MS-DOS drive mounted in HDD 1 (A:) and the game floppies in FDD 1 (B:) / FDD 2 (C:).
    DosHddWithFloppy,
    /// Standalone CD-ROM image (mounted with the base MS-DOS drive in HDD 1 if configured).
    CdRom,
}

impl Pc98BootMode {
    pub fn short_badge(self, primary_ext: &str) -> String {
        let ext = if primary_ext.is_empty() {
            "DISK".to_string()
        } else {
            primary_ext.to_ascii_uppercase()
        };
        match self {
            Pc98BootMode::SelfBootHdd => format!("{ext} · Bootable HDD"),
            Pc98BootMode::FloppyBootWithGameHdd => "Boot FD + HDD".to_string(),
            Pc98BootMode::DosHddWithGameHdd => format!("{ext} · Needs DOS HDD"),
            Pc98BootMode::SelfBootFloppy => format!("{ext} · Self-Boot"),
            Pc98BootMode::DosHddWithFloppy => format!("{ext} · Needs DOS HDD"),
            Pc98BootMode::CdRom => "CD-ROM".to_string(),
        }
    }

    pub fn requires_dos_hdd(self) -> bool {
        matches!(
            self,
            Pc98BootMode::DosHddWithFloppy | Pc98BootMode::DosHddWithGameHdd | Pc98BootMode::CdRom
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pc98GameEntry {
    /// Unique identifier (relative path from `games_dir`, or file/folder name).
    pub id: String,
    /// Display title of the game.
    pub title: String,
    /// Optional category / parent folder label (e.g. "あ", "同人/ＣＧ集", "Banpresto").
    pub category: Option<String>,
    /// Directory or file path representing the game.
    pub root_path: PathBuf,
    /// Automatically detected boot & mount strategy.
    pub boot_mode: Pc98BootMode,
    /// Ordered list of floppy disk images belonging to this game (boot/system/Disk 1 first).
    pub floppies: Vec<PathBuf>,
    /// Ordered list of hard disk images belonging to this game.
    pub hdds: Vec<PathBuf>,
    /// Ordered list of CD-ROM images belonging to this game.
    pub cdroms: Vec<PathBuf>,
    /// Precomputed multi-representation fuzzy search index (Kanji, Hiragana, Katakana, Romaji, English).
    pub search_index: super::search::Pc98SearchIndex,
}

impl Pc98GameEntry {
    pub fn primary_format_ext(&self) -> &str {
        let primary = match self.boot_mode {
            Pc98BootMode::SelfBootHdd
            | Pc98BootMode::DosHddWithGameHdd
            | Pc98BootMode::FloppyBootWithGameHdd => self.hdds.first().or(self.floppies.first()),
            Pc98BootMode::SelfBootFloppy | Pc98BootMode::DosHddWithFloppy => {
                self.floppies.first().or(self.hdds.first())
            }
            Pc98BootMode::CdRom => self.cdroms.first(),
        };
        primary
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
            .unwrap_or("")
    }

    pub fn display_label(&self) -> String {
        let badge = self.boot_mode.short_badge(self.primary_format_ext());
        let disk_count_suffix = if self.floppies.len() > 1 && self.hdds.is_empty() {
            format!(" ({}FD)", self.floppies.len())
        } else {
            String::new()
        };
        if let Some(ref cat) = self.category {
            format!("[{}] {} — [{}{}]", cat, self.title, badge, disk_count_suffix)
        } else {
            format!("{} — [{}{}]", self.title, badge, disk_count_suffix)
        }
    }
}

pub fn is_fdd_ext(ext: &str) -> bool {
    FDD_EXTS.iter().any(|e| ext.eq_ignore_ascii_case(e))
}

pub fn is_hdd_ext(ext: &str) -> bool {
    HDD_EXTS.iter().any(|e| ext.eq_ignore_ascii_case(e))
}

pub fn is_cdrom_ext(ext: &str) -> bool {
    CDROM_EXTS.iter().any(|e| ext.eq_ignore_ascii_case(e))
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Checks whether `data` contains an actual FAT directory entry for `IO      SYS`
/// (as opposed to the `IO      SYSMSDOS   SYS` literal string inside a FAT boot sector).
pub fn has_io_sys_dir_entry(data: &[u8]) -> bool {
    let needle = b"IO      SYS";
    if data.len() < needle.len() {
        return false;
    }
    let mut idx = 0;
    while idx + needle.len() <= data.len() {
        if let Some(pos) = data[idx..].windows(needle.len()).position(|w| w == needle) {
            let abs = idx + pos;
            let after_start = abs + needle.len();
            let after_end = (after_start + 11).min(data.len());
            if &data[after_start..after_end] != b"MSDOS   SYS" {
                return true;
            }
            idx = abs + needle.len();
        } else {
            break;
        }
    }
    false
}

fn read_prefix_from_file_or_zip(path: &Path, max_bytes: usize, prefer_hdd: bool) -> Option<(String, Vec<u8>)> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "zip" {
        let file = fs::File::open(path).ok()?;
        let mut archive = zip::ZipArchive::new(file).ok()?;
        let allowed = if prefer_hdd { HDD_EXTS } else { FDD_EXTS };
        let mut best_idx = None;
        let mut best_name = String::new();
        for i in 0..archive.len() {
            let Ok(entry) = archive.by_index_raw(i) else {
                continue;
            };
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().to_string();
            let lower = name.to_ascii_lowercase();
            if lower.contains("/alts/") || lower.contains("\\alts\\") {
                continue;
            }
            let Some((_, e)) = lower.rsplit_once('.') else {
                continue;
            };
            if allowed.iter().any(|a| e == *a) {
                if best_idx.is_none() || natural_cmp(&name, &best_name).is_lt() {
                    best_idx = Some(i);
                    best_name = name;
                }
            }
        }
        let idx = best_idx?;
        let entry = archive.by_index(idx).ok()?;
        let inner_ext = Path::new(&best_name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut buf = Vec::with_capacity(max_bytes.min(65536));
        entry.take(max_bytes as u64).read_to_end(&mut buf).ok()?;
        Some((inner_ext, buf))
    } else {
        let file = fs::File::open(path).ok()?;
        let mut buf = Vec::with_capacity(max_bytes.min(65536));
        file.take(max_bytes as u64).read_to_end(&mut buf).ok()?;
        Some((ext, buf))
    }
}

/// Extracts the logical sector payload (starting at Track 0 Sector 1) from a floppy image header.
fn extract_floppy_payload(ext: &str, data: &[u8]) -> Option<Vec<u8>> {
    match ext {
        "fdi" => {
            if data.len() < 4096 + 1024 {
                return None;
            }
            let hdr = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
            let offset = if (512..=65536).contains(&hdr) {
                hdr
            } else {
                4096
            };
            if offset >= data.len() {
                return None;
            }
            Some(data[offset..].to_vec())
        }
        "d88" | "88d" | "d98" | "98d" => {
            if data.len() < 0x2b0 + 512 {
                return None;
            }
            let t0 = u32::from_le_bytes([data[0x20], data[0x21], data[0x22], data[0x23]]) as usize;
            if t0 == 0 || t0 + 16 >= data.len() {
                return None;
            }
            let mut out = Vec::with_capacity(65536);
            let mut pos = t0;
            while pos + 16 <= data.len() && out.len() < 65536 {
                let sec_size = u16::from_le_bytes([data[pos + 14], data[pos + 15]]) as usize;
                if sec_size == 0 || sec_size > 4096 || pos + 16 + sec_size > data.len() {
                    break;
                }
                out.extend_from_slice(&data[pos + 16..pos + 16 + sec_size]);
                pos += 16 + sec_size;
            }
            Some(out)
        }
        "nfd" => {
            if data.starts_with(b"T98FDDIMAGE.R0") || data.starts_with(b"T98FDDIMAGE.R1") {
                if data.len() < 0x114 {
                    return None;
                }
                let hdr =
                    u32::from_le_bytes([data[0x110], data[0x111], data[0x112], data[0x113]])
                        as usize;
                if hdr >= data.len() {
                    return None;
                }
                Some(data[hdr..].to_vec())
            } else {
                None
            }
        }
        "fim" => {
            if data.len() <= 256 {
                None
            } else {
                Some(data[256..].to_vec())
            }
        }
        "fdd" => {
            if data.len() <= 220 {
                None
            } else {
                Some(data[220..].to_vec())
            }
        }
        _ => Some(data.to_vec()),
    }
}

/// Inspects a PC-98 floppy image's boot sector (Track 0, Sector 1) and FAT root directory
/// to determine whether the disk is self-bootable or requires a separate MS-DOS boot drive.
pub fn is_floppy_self_bootable(path: &Path) -> bool {
    let Some((ext, raw)) = read_prefix_from_file_or_zip(path, 196_608, false) else {
        return false;
    };
    let Some(payload) = extract_floppy_payload(&ext, &raw) else {
        return false;
    };
    if payload.len() < 2048 {
        return false;
    }

    let bs = &payload[..1024];
    let rest = &payload[1024..];

    // Blank / unformatted boot sector
    if bs[..64].iter().all(|&b| b == 0) || bs[..64].iter().all(|&b| b == 0xE5) {
        return false;
    }

    // Immediate infinite loop (`JMP $` = 0xEB 0xFE) or `HLT` (0xF4) stub at start of boot sector
    if bs.starts_with(&[0xEB, 0xFE]) || bs[0] == 0xF4 {
        return false;
    }

    // Short JMP over BPB to an infinite-loop/halt/zero stub
    if bs[0] == 0xEB && (bs[1] as usize) < 0x80 {
        let target = 2 + (bs[1] as usize);
        if target + 16 <= bs.len() {
            let code = &bs[target..target + 16];
            if code.starts_with(&[0xEB, 0xFE])
                || code.starts_with(&[0xF4, 0xEB, 0xFE])
                || code.iter().all(|&b| b == 0)
            {
                return false;
            }
        }
    }

    let has_io_in_dir = has_io_sys_dir_entry(rest)
        || contains_bytes(rest, b"IBMBIO  COM")
        || (contains_bytes(bs, b"AL_DOS  SYS") && contains_bytes(rest, b"AL_DOS  SYS"));
    let bs_checks_iosys =
        contains_bytes(bs, b"IO      SYS") || contains_bytes(bs, b"MSDOS   SYS");
    if bs_checks_iosys && !has_io_in_dir {
        return false;
    }

    let has_nonsys_msg = contains_bytes(bs, b"Non-System disk")
        || contains_bytes(bs, b"NO SYSTEM")
        || contains_bytes(bs, b"\xbc\xbd\xc3\xd1\xc3\xde\xa8\xbd\xb8")
        || contains_bytes(bs, b"\x83\x56\x83\x58\x83\x65\x83\x80");
    if has_nonsys_msg && !has_io_in_dir {
        return false;
    }

    true
}

/// Checks whether a floppy disk image contains an `AUTOEXEC.BAT` entry in its directory area.
pub fn floppy_has_autoexec(path: &Path) -> bool {
    let Some((ext, raw)) = read_prefix_from_file_or_zip(path, 196_608, false) else {
        return false;
    };
    let Some(payload) = extract_floppy_payload(&ext, &raw) else {
        return false;
    };
    if payload.len() < 2048 {
        return false;
    }
    contains_bytes(&payload[1024..], b"AUTOEXECBAT")
}

/// Inspects a PC-98 hard disk image (`.hdi`, `.nhd`, etc.) and returns
/// `(is_self_bootable, has_autoexec_bat)`.
pub fn inspect_hdd_image(path: &Path) -> (bool, bool) {
    let Some((_ext, data)) = read_prefix_from_file_or_zip(path, 262_144, true) else {
        return (true, true);
    };
    if data.len() < 8192 {
        return (false, false);
    }
    let has_io_dir = has_io_sys_dir_entry(&data) || contains_bytes(&data, b"IBMBIO  COM");
    let has_autoexec = contains_bytes(&data, b"AUTOEXECBAT");
    let has_dos_boot_stub =
        contains_bytes(&data, b"IO      SYSMSDOS   SYS") || contains_bytes(&data, b"NO SYSTEM FILES");

    let is_self_bootable = if has_io_dir {
        true
    } else {
        !has_dos_boot_stub
    };
    (is_self_bootable, has_autoexec)
}

pub fn has_boot_floppy_keyword(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    ["起動", "boot", "kidou", "dos62", "dos5", "dos3"]
        .iter()
        .any(|k| lower.contains(k))
}

/// Determines whether `FDD 1` / `FDD 2` should be omitted at initial power-on and mounted
/// after POST (so the PC-98 BIOS boots from `HDD 1`), or mounted immediately at power-on
/// (so the PC-98 BIOS boots from `FDD 1` with `HDD 1` attached as `C:`).
pub fn should_defer_fdd_boot(config: &Pc98Config) -> bool {
    let hdd1_str = config.hdd1_path.trim();
    if hdd1_str.is_empty() {
        return false;
    }
    let hdd1_path = Path::new(hdd1_str);
    if !hdd1_path.exists() {
        return false;
    }

    // If HDD 1 is the configured Base DOS HDD, always boot from HDD 1 first.
    let dos_str = config.dos_hdd_path.trim();
    if !dos_str.is_empty() && Path::new(dos_str) == hdd1_path {
        return true;
    }
    if hdd1_path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.to_ascii_lowercase().contains("master_msdos"))
    {
        return true;
    }

    let fdd1_str = config.fdd1_path.trim();
    if fdd1_str.is_empty() {
        return true;
    }
    let fdd1_path = Path::new(fdd1_str);
    if !fdd1_path.exists() {
        return true;
    }

    // If FDD 1 is not self-bootable, defer it so it never hijacks the boot from HDD 1.
    if !is_floppy_self_bootable(fdd1_path) {
        return true;
    }

    // FDD 1 is self-bootable; check whether HDD 1 needs FDD 1 as its boot disk.
    let fdd1_name = fdd1_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let (hdd_bootable, hdd_has_autoexec) = inspect_hdd_image(hdd1_path);
    let fdd_is_dedicated_boot =
        has_boot_floppy_keyword(fdd1_name) && floppy_has_autoexec(fdd1_path);
    if !hdd_bootable || !hdd_has_autoexec || fdd_is_dedicated_boot {
        // Boot from FDD 1 (A:) with HDD 1 mounted at power-on (C:).
        return false;
    }

    true
}

/// Determines the DOS command sequence to auto-type if the emulated PC-98 boots `HDD 1`
/// and drops to an interactive `A:\>` prompt (e.g., `DosHddWithFloppy` games booted via
/// `Master_MSDOS62.hdi`, or `SelfBootHdd` games like `EVE.hdi` whose `AUTOEXEC.BAT` only
/// runs `echo off` / `cls`).
pub fn detect_startup_command(config: &Pc98Config) -> Option<String> {
    let hdd1_str = config.hdd1_path.trim();
    if hdd1_str.is_empty() {
        return None;
    }
    let hdd1_path = Path::new(hdd1_str);
    if !hdd1_path.exists() {
        return None;
    }

    let dos_str = config.dos_hdd_path.trim();
    let is_base_dos = (!dos_str.is_empty() && Path::new(dos_str) == hdd1_path)
        || hdd1_path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.to_ascii_lowercase().contains("master_msdos"));

    if is_base_dos {
        let fdd1_str = config.fdd1_path.trim();
        if !fdd1_str.is_empty() {
            let fdd1_path = Path::new(fdd1_str);
            if fdd1_path.exists() {
                return Some(detect_floppy_startup_command(fdd1_path));
            }
        }
        if !config.hdd2_path.trim().is_empty() {
            return Some("b:\ndir /w\n".to_string());
        }
        return None;
    }

    detect_hdd_fallback_command(hdd1_path)
}

fn is_ascii_dos_ident(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn is_utility_or_driver_exe(stem_upper: &str) -> bool {
    matches!(
        stem_upper,
        "COMMAND"
            | "FORMAT"
            | "SYS"
            | "FDISK"
            | "LHA"
            | "LH"
            | "UNZIP"
            | "PKUNZIP"
            | "DEARJ"
            | "LWEXT"
            | "README"
            | "REMOVE"
            | "MOUSE"
            | "S98MOUSE"
            | "PMD"
            | "PMDB"
            | "PMDB2"
            | "PMD98"
            | "PMD86"
            | "MMD"
            | "FMP"
            | "NAX"
            | "SDN"
            | "OPNDRV"
            | "EMM386"
            | "MAKE_SYS"
    ) || stem_upper.ends_with("_FD1")
}

fn is_installer_stem(stem_upper: &str) -> bool {
    stem_upper.starts_with("INST")
        || stem_upper.starts_with("WINST")
        || stem_upper.starts_with("HDINST")
        || stem_upper.starts_with("HDDINST")
        || stem_upper.starts_with("DOSINST")
        || stem_upper.starts_with("SETUP")
        || stem_upper == "HD"
        || stem_upper == "HDI"
        || stem_upper.starts_with("MAKE")
        || stem_upper.starts_with("COPY")
}

/// Inspects the FAT12 root directory of a floppy image mounted in `FDD 1` (`B:`)
/// and returns the command sequence to switch to `B:` and start the game or installer.
pub fn detect_floppy_startup_command(fdd1_path: &Path) -> String {
    let Some((ext, raw)) = read_prefix_from_file_or_zip(fdd1_path, 262_144, false) else {
        return "b:\ndir /w\n".to_string();
    };
    let Some(payload) = extract_floppy_payload(&ext, &raw) else {
        return "b:\ndir /w\n".to_string();
    };
    if payload.len() < 0x2c00 {
        return "b:\ndir /w\n".to_string();
    }

    let bps = u16::from_le_bytes([payload[0x0b], payload[0x0c]]) as usize;
    let spc = (payload[0x0d] as usize).max(1);
    let res = u16::from_le_bytes([payload[0x0e], payload[0x0f]]) as usize;
    let nfats = payload[0x10] as usize;
    let rent = u16::from_le_bytes([payload[0x11], payload[0x12]]) as usize;
    let spf = u16::from_le_bytes([payload[0x16], payload[0x17]]) as usize;

    let (bps, spc, res, nfats, rent, spf) =
        if matches!(bps, 512 | 1024) && (1..=2).contains(&nfats) && (16..=512).contains(&rent) && (1..=32).contains(&spf) {
            (bps, spc, res, nfats, rent, spf)
        } else {
            (1024, 1, 1, 2, 192, 2)
        };

    let rdir_off = (res + nfats * spf) * bps;
    let rdir_len = rent * 32;
    let data_off = rdir_off + rdir_len;
    if rdir_off >= payload.len() {
        return "b:\ndir /w\n".to_string();
    }

    let mut has_autoexec = false;
    let mut bats: Vec<String> = Vec::new();
    let mut exes: Vec<String> = Vec::new();
    let mut installers: Vec<(String, String, Vec<u8>)> = Vec::new();

    let rdir_end = (rdir_off + rdir_len).min(payload.len());
    for ent in payload[rdir_off..rdir_end].chunks_exact(32) {
        if ent[0] == 0x00 {
            break;
        }
        if ent[0] == 0xE5 || (ent[11] & 0x18) != 0 {
            continue;
        }
        let Ok(raw_name) = std::str::from_utf8(&ent[0..8]) else {
            continue;
        };
        let Ok(raw_ext) = std::str::from_utf8(&ent[8..11]) else {
            continue;
        };
        let stem = raw_name.trim().to_ascii_uppercase();
        let fext = raw_ext.trim().to_ascii_uppercase();
        if !is_ascii_dos_ident(&stem) {
            continue;
        }
        if !matches!(fext.as_str(), "BAT" | "EXE" | "COM") {
            continue;
        }

        let clus = u16::from_le_bytes([ent[26], ent[27]]) as usize;
        let fsize = u32::from_le_bytes([ent[28], ent[29], ent[30], ent[31]]) as usize;
        let body = if clus >= 2 && fext == "BAT" {
            let foff = data_off + (clus - 2) * spc * bps;
            let fend = (foff + fsize.min(2048)).min(payload.len());
            if foff < fend {
                payload[foff..fend].to_vec()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        if stem == "AUTOEXEC" {
            has_autoexec = true;
        } else if is_installer_stem(&stem) {
            installers.push((stem, fext, body));
        } else if fext == "BAT" {
            bats.push(stem);
        } else if !is_utility_or_driver_exe(&stem) {
            exes.push(stem);
        }
    }

    // 1. AUTOEXEC.BAT / AUTOEXEC.EXE on floppy
    if has_autoexec {
        return "b:\nautoexec\n".to_string();
    }

    // 2. Non-installer launcher .BAT on floppy
    for pref in ["START", "GAME", "MAIN", "GO", "RUN", "PLAY", "MENU", "CG98"] {
        if bats.iter().any(|b| b == pref) {
            return format!("b:\n{}\n", pref.to_ascii_lowercase());
        }
    }
    if bats.len() == 1 {
        return format!("b:\n{}\n", bats[0].to_ascii_lowercase());
    }

    // 3. Dedicated WINSTALL.EXE (Wizardry 6) or INSTALL.BAT taking source/dest drives (Ukiuki Island)
    if installers.iter().any(|(s, _, _)| s == "WINSTALL") {
        return "b:\nwinstall\n".to_string();
    }
    if let Some((_, _, body)) = installers
        .iter()
        .find(|(s, e, _)| s == "INSTALL" && e == "BAT")
    {
        if contains_bytes(body, b"%1") && contains_bytes(body, b"%2") {
            return "b:\ninstall b: a:\n".to_string();
        }
        return "b:\ninstall\n".to_string();
    }

    // 4. Single main game executable on floppy
    if exes.len() == 1 {
        return format!("b:\n{}\n", exes[0].to_ascii_lowercase());
    }
    for pref in ["START", "GAME", "MAIN", "PLAY", "MENU"] {
        if exes.iter().any(|e| e == pref) {
            return format!("b:\n{}\n", pref.to_ascii_lowercase());
        }
    }

    // 5. Fallback to installer if present
    for pref in ["INSTALL", "HDINST", "INST", "SETUP"] {
        if installers.iter().any(|(s, _, _)| s == pref) {
            return format!("b:\n{}\n", pref.to_ascii_lowercase());
        }
    }
    if let Some((stem, _, _)) = installers.first() {
        return format!("b:\n{}\n", stem.to_ascii_lowercase());
    }

    // 6. Otherwise switch to B: and display directory listing
    "b:\ndir /w\n".to_string()
}

fn detect_hdd_fallback_command(hdd1_path: &Path) -> Option<String> {
    let Some((_ext, data)) = read_prefix_from_file_or_zip(hdd1_path, 262_144, true) else {
        return None;
    };
    if data.len() < 8192 {
        return None;
    }

    // Locate IO.SYS directory entry to find the start of the root directory
    let needle = b"IO      SYS";
    let mut io_pos = None;
    let mut idx = 0;
    while idx + needle.len() <= data.len() {
        if let Some(pos) = data[idx..].windows(needle.len()).position(|w| w == needle) {
            let abs = idx + pos;
            let after_start = abs + needle.len();
            let after_end = (after_start + 11).min(data.len());
            if &data[after_start..after_end] != b"MSDOS   SYS" {
                io_pos = Some(abs);
                break;
            }
            idx = abs + needle.len();
        } else {
            break;
        }
    }
    let dir_start = (io_pos? / 1024) * 1024;
    let dir_end = (dir_start + 24576).min(data.len());

    let mut bats: Vec<String> = Vec::new();
    let mut exes: Vec<String> = Vec::new();

    for ent in data[dir_start..dir_end].chunks_exact(32) {
        if ent[0] == 0x00 || ent[0] == 0xE5 || (ent[11] & 0x18) != 0 {
            continue;
        }
        if !matches!(ent[11], 0x20 | 0x27 | 0x07 | 0x00 | 0x01) {
            continue;
        }
        if !ent[0..11].iter().all(|&b| (0x20..=0x7E).contains(&b)) {
            continue;
        }
        let Ok(raw_name) = std::str::from_utf8(&ent[0..8]) else {
            continue;
        };
        let Ok(raw_ext) = std::str::from_utf8(&ent[8..11]) else {
            continue;
        };
        let stem = raw_name.trim().to_ascii_uppercase();
        let fext = raw_ext.trim().to_ascii_uppercase();
        if !is_ascii_dos_ident(&stem) || stem == "AUTOEXEC" {
            continue;
        }
        if fext == "BAT" && !is_installer_stem(&stem) {
            bats.push(stem);
        } else if matches!(fext.as_str(), "EXE" | "COM")
            && !is_installer_stem(&stem)
            && !is_utility_or_driver_exe(&stem)
        {
            exes.push(stem);
        }
    }

    for pref in ["START", "GAME", "MAIN", "RUN"] {
        if bats.iter().any(|b| b == pref) {
            return Some(format!("{}\n", pref.to_ascii_lowercase()));
        }
    }
    if bats.len() == 1 {
        return Some(format!("{}\n", bats[0].to_ascii_lowercase()));
    }
    if bats.is_empty() && exes.len() == 1 {
        return Some(format!("{}\n", exes[0].to_ascii_lowercase()));
    }
    None
}

/// Case-insensitive natural string comparison (compares embedded numbers numerically).
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ia = a.chars().peekable();
    let mut ib = b.chars().peekable();

    loop {
        match (ia.peek(), ib.peek()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(&ca), Some(&cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let mut na: u64 = 0;
                    let mut len_a = 0;
                    while let Some(&d) = ia.peek() {
                        if d.is_ascii_digit() {
                            na = na.saturating_mul(10).saturating_add((d as u8 - b'0') as u64);
                            len_a += 1;
                            ia.next();
                        } else {
                            break;
                        }
                    }
                    let mut nb: u64 = 0;
                    let mut len_b = 0;
                    while let Some(&d) = ib.peek() {
                        if d.is_ascii_digit() {
                            nb = nb.saturating_mul(10).saturating_add((d as u8 - b'0') as u64);
                            len_b += 1;
                            ib.next();
                        } else {
                            break;
                        }
                    }
                    match na.cmp(&nb) {
                        std::cmp::Ordering::Equal => {
                            if len_a != len_b {
                                return len_a.cmp(&len_b);
                            }
                        }
                        ord => return ord,
                    }
                } else {
                    let la = ca.to_lowercase().next().unwrap_or(ca);
                    let lb = cb.to_lowercase().next().unwrap_or(cb);
                    match la.cmp(&lb) {
                        std::cmp::Ordering::Equal => {
                            ia.next();
                            ib.next();
                        }
                        ord => return ord,
                    }
                }
            }
        }
    }
}

/// Assigns a role priority tier to a floppy disk filename so system/boot/opening/program
/// disks are mounted in FDD 1, game/numbered/data disks in FDD 2, and user/save/utility disks last.
pub fn floppy_role_tier(filename: &str) -> u8 {
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename)
        .to_lowercase();

    // Tier 4: User / Save / Solution / Utility / Briefing / HD-Boot utility disks
    if [
        "user",
        "usr",
        "ﾕｰｻﾞ",
        "ユーザ",
        "save",
        "sav",
        "ｾｰﾌﾞ",
        "セーブ",
        "解答",
        "マップ",
        "エディタ",
        "brefing",
        "briefing",
        "hd起動",
        "hd_起動",
        "hdd起動",
        "hdboot",
        "hdinstall",
        "hdインストール",
        "hdｲﾝｽﾄｰﾙ",
    ]
    .iter()
    .any(|k| stem.contains(k))
    {
        return 4;
    }

    // Tier 3: Ending / Visual disks
    if [
        "end",
        "ending",
        "エンディング",
        "ｴﾝﾃﾞｨﾝｸﾞ",
        "vis",
        "visual",
    ]
    .iter()
    .any(|k| stem.contains(k))
    {
        return 3;
    }

    // Tier 0: Explicit Boot / System / Program / Opening / Start disks
    if [
        "起動",
        "boot",
        "kidou",
        "sys",
        "system",
        "ｼｽﾃﾑ",
        "システム",
        "prog",
        "program",
        "ﾌﾟﾛｸﾞﾗﾑ",
        "プログラム",
        "start",
        "open",
        "opening",
        "opn",
        "main",
    ]
    .iter()
    .any(|k| stem.contains(k))
        || stem.ends_with("_s")
        || stem.ends_with("-s")
        || stem.ends_with("sy")
        || stem.ends_with("pr")
    {
        return 0;
    }

    // Tier 2: Explicit Data / Scenario disks (placed just after Game / numbered disks)
    if [
        "data",
        "dat",
        "ﾃﾞｰﾀ",
        "データ",
        "scen",
        "sinario",
        "ｼﾅﾘｵ",
        "シナリオ",
    ]
    .iter()
    .any(|k| stem.contains(k))
        || stem.ends_with("da")
    {
        return 2;
    }

    // Tier 1: Standard numbered / lettered / game disks (Disk 1, Disk A, Game Disk, etc.)
    1
}

/// Orders floppy disk paths so the boot/system/first disk is at index 0 (`FDD 1`)
/// and the second disk (`Disk 2` / `Disk B` / `Game` / `Data`) is at index 1 (`FDD 2`).
/// Also returns whether at least one floppy in the set is self-bootable.
pub fn order_floppies_and_check_bootable(floppies: &mut Vec<PathBuf>) -> bool {
    floppies.sort_by(|a, b| {
        let name_a = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let name_b = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let tier_a = floppy_role_tier(name_a);
        let tier_b = floppy_role_tier(name_b);
        tier_a
            .cmp(&tier_b)
            .then_with(|| natural_cmp(name_a, name_b))
    });

    if floppies.is_empty() {
        return false;
    }

    if is_floppy_self_bootable(&floppies[0]) {
        return true;
    }

    // If floppies[0] is not self-bootable, check if another non-save/non-user disk in the folder
    // is a genuine self-bootable disk (e.g., `ウルティマⅣ_Ultima.fdi` after `ウルティマⅣ_Britania.fdi`,
    // or `Avgﾂｸﾙ_m.D88` after `AVGﾂｸﾙ_A.D88`).
    for idx in 1..floppies.len() {
        let name = floppies[idx]
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if floppy_role_tier(name) >= 3 {
            continue;
        }
        // Do not promote a sequentially numbered disk 2..9 above disk 1 when disk 1 is non-bootable
        if is_numbered_continuation_disk(name) {
            continue;
        }
        if is_floppy_self_bootable(&floppies[idx]) {
            let boot_disk = floppies.remove(idx);
            floppies.insert(0, boot_disk);
            return true;
        }
    }

    false
}

fn is_numbered_continuation_disk(filename: &str) -> bool {
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename)
        .trim();
    // Ends with digit 2..=9 or 10..
    if let Some(last_char) = stem.chars().last() {
        if last_char.is_ascii_digit() && last_char != '1' && last_char != '0' {
            return true;
        }
    }
    false
}

fn is_format_subfolder_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "d88" | "fdi" | "hdi" | "nhd" | "nfd" | "hdm" | "2hd" | "xdf" | "dup" | "fdd" | "cd" | "cdrom"
    )
}

fn is_ignored_subfolder_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "alts" | "alt" | "scp" | "mfi" | "hfe" | "extras" | "extra" | "manual" | "manuals"
    )
}

#[derive(Default)]
struct RawFolderMedia {
    floppies: Vec<PathBuf>,
    hdds: Vec<PathBuf>,
    cdroms: Vec<PathBuf>,
    zip_floppies: Vec<PathBuf>,
    zip_hdds: Vec<PathBuf>,
    zip_cdroms: Vec<PathBuf>,
}

impl RawFolderMedia {
    fn is_empty(&self) -> bool {
        self.floppies.is_empty()
            && self.hdds.is_empty()
            && self.cdroms.is_empty()
            && self.zip_floppies.is_empty()
            && self.zip_hdds.is_empty()
            && self.zip_cdroms.is_empty()
    }
}

fn classify_zip_archive(zip_path: &Path) -> (bool, bool, bool) {
    let fname_lower = zip_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if fname_lower.contains("extra") || fname_lower.contains("manual") {
        return (false, false, false);
    }

    let Ok(file) = fs::File::open(zip_path) else {
        return (false, false, false);
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return (false, false, false);
    };

    let mut has_fdd = false;
    let mut has_hdd = false;
    let mut has_cd = false;

    for i in 0..archive.len() {
        let Ok(entry) = archive.by_index_raw(i) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let name = entry.name();
        let Some((_, ext)) = name.rsplit_once('.') else {
            continue;
        };
        if is_hdd_ext(ext) {
            has_hdd = true;
        } else if is_fdd_ext(ext) {
            has_fdd = true;
        } else if is_cdrom_ext(ext) {
            has_cd = true;
        }
    }

    (has_fdd, has_hdd, has_cd)
}

/// Attempts to automatically locate a master bootable MS-DOS hard disk image
/// (such as `Master_MSDOS62.hdi`) inside `root_dir`.
pub fn detect_default_dos_hdd(root_dir: &Path) -> Option<PathBuf> {
    if !root_dir.is_dir() {
        return None;
    }
    let direct = root_dir.join("Master_MSDOS62.hdi");
    if direct.is_file() {
        return Some(direct);
    }
    if let Ok(entries) = fs::read_dir(root_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                let name = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if (name.contains("master_msdos") || name.starts_with("msdos"))
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(is_hdd_ext)
                {
                    return Some(p);
                }
            }
        }
    }
    let os_sub = root_dir.join("ＯＳ").join("Master_MSDOS62").join("Master_MSDOS62.hdi");
    if os_sub.is_file() {
        return Some(os_sub);
    }
    None
}

fn is_master_dos_file(path: &Path, dos_hdd_path: Option<&Path>) -> bool {
    if let Some(dos_p) = dos_hdd_path {
        if path == dos_p {
            return true;
        }
    }
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("Master_MSDOS62.hdi"))
}

/// Recursively scans `root_dir` and returns all discovered PC-98 games, sorted by title.
pub fn scan_games_directory(root_dir: &Path, dos_hdd_path: &str) -> Result<Vec<Pc98GameEntry>> {
    if !root_dir.exists() {
        anyhow::bail!("Directory does not exist: {}", root_dir.display());
    }
    if !root_dir.is_dir() {
        anyhow::bail!("Path is not a directory: {}", root_dir.display());
    }

    let dos_path_opt = {
        let trimmed = dos_hdd_path.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(Path::new(trimmed))
        }
    };

    let mut folder_map: BTreeMap<PathBuf, RawFolderMedia> = BTreeMap::new();
    let mut dirs_to_visit = vec![root_dir.to_path_buf()];

    while let Some(current_dir) = dirs_to_visit.pop() {
        let Ok(read_dir) = fs::read_dir(&current_dir) else {
            continue;
        };

        let mut subdirs = Vec::new();
        for entry in read_dir.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !dir_name.starts_with('.') && !is_ignored_subfolder_name(dir_name) {
                    subdirs.push(path);
                }
            } else if ft.is_file() {
                if is_master_dos_file(&path, dos_path_opt) {
                    continue;
                }
                let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                    continue;
                };
                let is_fdd = is_fdd_ext(ext);
                let is_hdd = is_hdd_ext(ext);
                let is_cd = is_cdrom_ext(ext);
                let is_zip = ext.eq_ignore_ascii_case("zip");

                if !is_fdd && !is_hdd && !is_cd && !is_zip {
                    continue;
                }

                // Determine which logical game folder owns this media file.
                let group_key = if current_dir == root_dir {
                    // Loose disk image directly in root_dir: treat the file itself as its own entry
                    path.clone()
                } else {
                    let dir_name = current_dir
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("");
                    if is_format_subfolder_name(dir_name) {
                        current_dir
                            .parent()
                            .unwrap_or(&current_dir)
                            .to_path_buf()
                    } else {
                        current_dir.clone()
                    }
                };

                let media = folder_map.entry(group_key).or_default();
                if is_hdd {
                    media.hdds.push(path);
                } else if is_fdd {
                    media.floppies.push(path);
                } else if is_cd {
                    media.cdroms.push(path);
                } else if is_zip {
                    let (zf, zh, zc) = classify_zip_archive(&path);
                    if zh {
                        media.zip_hdds.push(path);
                    } else if zf {
                        media.zip_floppies.push(path);
                    } else if zc {
                        media.zip_cdroms.push(path);
                    }
                }
            }
        }

        subdirs.sort();
        for d in subdirs.into_iter().rev() {
            dirs_to_visit.push(d);
        }
    }

    let mut games = Vec::with_capacity(folder_map.len());
    for (group_path, mut raw) in folder_map {
        if raw.is_empty() {
            continue;
        }

        // Prefer extracted/loose disk images in the folder over .zip backups in the same folder.
        let mut hdds = if !raw.hdds.is_empty() {
            raw.hdds
        } else {
            raw.zip_hdds.sort_by(|a, b| {
                natural_cmp(
                    a.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                    b.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                )
            });
            raw.zip_hdds.into_iter().take(1).collect()
        };

        let mut floppies = if !raw.floppies.is_empty() {
            raw.floppies
        } else if hdds.is_empty() {
            raw.zip_floppies.sort_by(|a, b| {
                natural_cmp(
                    a.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                    b.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                )
            });
            raw.zip_floppies.into_iter().take(1).collect()
        } else {
            Vec::new()
        };

        let mut cdroms = if !raw.cdroms.is_empty() {
            raw.cdroms
        } else {
            raw.zip_cdroms
        };

        hdds.sort_by(|a, b| {
            natural_cmp(
                a.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                b.file_name().and_then(|n| n.to_str()).unwrap_or(""),
            )
        });
        cdroms.sort_by(|a, b| {
            natural_cmp(
                a.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                b.file_name().and_then(|n| n.to_str()).unwrap_or(""),
            )
        });
        let any_fdd_bootable = order_floppies_and_check_bootable(&mut floppies);

        let boot_mode = if !hdds.is_empty() {
            let (hdd_bootable, hdd_has_autoexec) = inspect_hdd_image(&hdds[0]);
            let fdd_has_boot_kw = floppies.iter().any(|f| {
                f.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(has_boot_floppy_keyword)
                    && floppy_has_autoexec(f)
            });
            if any_fdd_bootable && (!hdd_bootable || !hdd_has_autoexec || fdd_has_boot_kw) {
                Pc98BootMode::FloppyBootWithGameHdd
            } else if hdd_bootable {
                Pc98BootMode::SelfBootHdd
            } else {
                Pc98BootMode::DosHddWithGameHdd
            }
        } else if !floppies.is_empty() {
            if any_fdd_bootable {
                Pc98BootMode::SelfBootFloppy
            } else {
                Pc98BootMode::DosHddWithFloppy
            }
        } else {
            Pc98BootMode::CdRom
        };

        let rel = group_path.strip_prefix(root_dir).unwrap_or(&group_path);
        let id = rel.to_string_lossy().to_string();
        let title = if group_path.is_file() {
            group_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&id)
                .to_string()
        } else {
            group_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&id)
                .to_string()
        };

        let category = rel
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_string_lossy().replace('\\', "/"));

        let search_index = super::search::Pc98SearchIndex::build(
            &title,
            category.as_deref(),
            &floppies,
            &hdds,
            &cdroms,
        );

        games.push(Pc98GameEntry {
            id,
            title,
            category,
            root_path: group_path,
            boot_mode,
            floppies,
            hdds,
            cdroms,
            search_index,
        });
    }

    games.sort_by(|a, b| {
        a.category
            .as_deref()
            .unwrap_or("")
            .cmp(b.category.as_deref().unwrap_or(""))
            .then_with(|| natural_cmp(&a.title, &b.title))
    });

    Ok(games)
}

/// Extracts all matching disk images from a `.zip` archive into `<saves_dir>/extracted/<zip_stem>/`
/// and returns their ordered paths. If `path` is not a `.zip`, returns `vec![path.to_path_buf()]`.
pub fn resolve_or_extract_all_floppies(path: &Path) -> Result<Vec<PathBuf>> {
    let is_zip = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    if !is_zip {
        return Ok(vec![path.to_path_buf()]);
    }

    let first = resolve_or_extract_media_path(path, Pc98MediaSlot::Fdd1)?;
    let Some(parent) = first.parent() else {
        return Ok(vec![first]);
    };

    let mut extracted = Vec::new();
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(is_fdd_ext)
            {
                extracted.push(p);
            }
        }
    }
    if extracted.is_empty() {
        let _ = saves_dir();
        Ok(vec![first])
    } else {
        order_floppies_and_check_bootable(&mut extracted);
        Ok(extracted)
    }
}

/// Configures `config`'s `hdd1_path`, `hdd2_path`, `fdd1_path`, `fdd2_path`, and `cdrom_path`
/// for the given `game` entry according to its detected `Pc98BootMode`.
pub fn apply_game_to_config(game: &mut Pc98GameEntry, config: &mut Pc98Config) -> Result<()> {
    // Expand a single .zip floppy entry into its extracted disk list on selection
    if game.floppies.len() == 1
        && game.floppies[0]
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
    {
        if let Ok(expanded) = resolve_or_extract_all_floppies(&game.floppies[0]) {
            game.floppies = expanded;
        }
    }

    let resolve_slot = |p: &Path, slot: Pc98MediaSlot| -> String {
        resolve_or_extract_media_path(p, slot)
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .to_string()
    };

    let effective_floppies: Vec<PathBuf> = if game.boot_mode == Pc98BootMode::SelfBootHdd {
        game.floppies
            .iter()
            .filter(|f| {
                let is_generic_boot = f
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(has_boot_floppy_keyword)
                    && !floppy_has_autoexec(f);
                !is_generic_boot
            })
            .cloned()
            .collect()
    } else {
        game.floppies.clone()
    };

    let fdd1 = effective_floppies
        .first()
        .map(|p| resolve_slot(p, Pc98MediaSlot::Fdd1))
        .unwrap_or_default();
    let fdd2 = effective_floppies
        .get(1)
        .map(|p| resolve_slot(p, Pc98MediaSlot::Fdd2))
        .unwrap_or_default();
    let cdrom = game
        .cdroms
        .first()
        .map(|p| resolve_slot(p, Pc98MediaSlot::CdRom))
        .unwrap_or_default();

    match game.boot_mode {
        Pc98BootMode::SelfBootHdd | Pc98BootMode::FloppyBootWithGameHdd => {
            config.hdd1_path = game
                .hdds
                .first()
                .map(|p| resolve_slot(p, Pc98MediaSlot::Hdd1))
                .unwrap_or_default();
            config.hdd2_path = game
                .hdds
                .get(1)
                .map(|p| resolve_slot(p, Pc98MediaSlot::Hdd2))
                .unwrap_or_default();
            config.fdd1_path = fdd1;
            config.fdd2_path = fdd2;
            config.cdrom_path = cdrom;
        }
        Pc98BootMode::DosHddWithGameHdd => {
            config.hdd1_path = config.dos_hdd_path.trim().to_string();
            config.hdd2_path = game
                .hdds
                .first()
                .map(|p| resolve_slot(p, Pc98MediaSlot::Hdd2))
                .unwrap_or_default();
            config.fdd1_path = fdd1;
            config.fdd2_path = fdd2;
            config.cdrom_path = cdrom;
        }
        Pc98BootMode::SelfBootFloppy => {
            config.hdd1_path.clear();
            config.hdd2_path.clear();
            config.fdd1_path = fdd1;
            config.fdd2_path = fdd2;
            config.cdrom_path = cdrom;
        }
        Pc98BootMode::DosHddWithFloppy | Pc98BootMode::CdRom => {
            config.hdd1_path = config.dos_hdd_path.trim().to_string();
            config.hdd2_path.clear();
            config.fdd1_path = fdd1;
            config.fdd2_path = fdd2;
            config.cdrom_path = cdrom;
        }
    }

    config.selected_game = game.id.clone();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fdi_image(boot_sector: &[u8], root_dir_data: &[u8]) -> Vec<u8> {
        let mut buf = vec![0u8; 4096 + 8192];
        // FDI header_size = 4096 at offset 8
        buf[8..12].copy_from_slice(&4096u32.to_le_bytes());
        let bs_len = boot_sector.len().min(1024);
        buf[4096..4096 + bs_len].copy_from_slice(&boot_sector[..bs_len]);
        let rd_len = root_dir_data.len().min(4096);
        buf[4096 + 1024..4096 + 1024 + rd_len].copy_from_slice(&root_dir_data[..rd_len]);
        buf
    }

    #[test]
    fn test_bootable_vs_non_bootable_fdi_detection() {
        let tmp = tempfile::tempdir().unwrap();

        // 1. Self-bootable FDI with custom IPL
        let boot_fdi = tmp.path().join("selfboot.fdi");
        let mut bs_custom = vec![0xEBu8, 0x1C, 0x90];
        bs_custom.extend_from_slice(b"IPL 1.00");
        bs_custom.resize(1024, 0x90);
        fs::write(&boot_fdi, make_fdi_image(&bs_custom, b"")).unwrap();
        assert!(is_floppy_self_bootable(&boot_fdi));

        // 2. Non-bootable DOS FDI (boot sector looks for IO.SYS/MSDOS.SYS, but no IO.SYS in dir)
        let nonboot_fdi = tmp.path().join("needs_dos.fdi");
        let mut bs_dos = vec![0xEBu8, 0x1C, 0x90];
        bs_dos.extend_from_slice(b"NEC  5.0");
        bs_dos.resize(512, 0x90);
        bs_dos.extend_from_slice(b"IO      SYSMSDOS   SYS");
        bs_dos.resize(1024, 0);
        fs::write(
            &nonboot_fdi,
            make_fdi_image(&bs_dos, b"GAME    EXE\x20\x00\x00\x00"),
        )
        .unwrap();
        assert!(!is_floppy_self_bootable(&nonboot_fdi));

        // 3. Bootable DOS FDI (boot sector looks for IO.SYS AND IO.SYS is present in root dir)
        let dos_boot_fdi = tmp.path().join("dos_boot.fdi");
        fs::write(
            &dos_boot_fdi,
            make_fdi_image(&bs_dos, b"IO      SYS\x07\x00\x00\x00\x00MSDOS   SYS\x07"),
        )
        .unwrap();
        assert!(is_floppy_self_bootable(&dos_boot_fdi));

        // 4. EB FE (infinite loop stub) non-bootable disk
        let ebfe_fdi = tmp.path().join("ebfe.fdi");
        let mut bs_ebfe = vec![0xEBu8, 0xFE, 0x90];
        bs_ebfe.extend_from_slice(b"S/3M 1.0");
        bs_ebfe.resize(1024, 0);
        fs::write(&ebfe_fdi, make_fdi_image(&bs_ebfe, b"")).unwrap();
        assert!(!is_floppy_self_bootable(&ebfe_fdi));
    }

    #[test]
    fn test_natural_sort_and_floppy_role_ordering() {
        let mut disks = vec![
            PathBuf::from("Userdisk.fdi"),
            PathBuf::from("Disk_10.fdi"),
            PathBuf::from("Disk_2.fdi"),
            PathBuf::from("Disk_1.fdi"),
            PathBuf::from("System.fdi"),
        ];
        order_floppies_and_check_bootable(&mut disks);
        let names: Vec<&str> = disks
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
            .collect();
        assert_eq!(
            names,
            vec![
                "System.fdi",
                "Disk_1.fdi",
                "Disk_2.fdi",
                "Disk_10.fdi",
                "Userdisk.fdi"
            ]
        );
    }

    #[test]
    fn test_scan_games_directory_and_apply_config() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let dos_hdd = root.join("Master_MSDOS62.hdi");
        fs::write(&dos_hdd, vec![0u8; 16384]).unwrap();

        // Game 1: Self-bootable HDI game
        let g1_dir = root.join("あ").join("YUNO");
        fs::create_dir_all(&g1_dir).unwrap();
        let mut hdi_bytes = vec![0u8; 16384];
        hdi_bytes[8..12].copy_from_slice(&4096u32.to_le_bytes());
        hdi_bytes[8192..8192 + 12].copy_from_slice(b"IO      SYS\x07");
        fs::write(g1_dir.join("YUNO.HDI"), hdi_bytes).unwrap();

        // Game 2: Non-bootable D88 game requiring DOS HDD
        let g2_dir = root.join("さ").join("SimGame");
        fs::create_dir_all(&g2_dir).unwrap();
        let mut bs_dos = vec![0xEBu8, 0x1C, 0x90];
        bs_dos.extend_from_slice(b"NEC  5.0");
        bs_dos.resize(512, 0x90);
        bs_dos.extend_from_slice(b"IO      SYSMSDOS   SYS");
        bs_dos.resize(1024, 0);
        fs::write(g2_dir.join("DISK_1.FDI"), make_fdi_image(&bs_dos, b"")).unwrap();
        fs::write(g2_dir.join("DISK_2.FDI"), make_fdi_image(&bs_dos, b"")).unwrap();

        // Game 3: Self-bootable FDI game
        let g3_dir = root.join("ね").join("Neji");
        fs::create_dir_all(&g3_dir).unwrap();
        let mut bs_custom = vec![0xEBu8, 0x1C, 0x90];
        bs_custom.resize(1024, 0x90);
        fs::write(g3_dir.join("neji_1.fdi"), make_fdi_image(&bs_custom, b"")).unwrap();
        fs::write(g3_dir.join("neji_2.fdi"), make_fdi_image(&bs_custom, b"")).unwrap();

        let detected_dos = detect_default_dos_hdd(root).unwrap();
        assert_eq!(detected_dos, dos_hdd);

        let mut games = scan_games_directory(root, &dos_hdd.to_string_lossy()).unwrap();
        assert_eq!(games.len(), 3);

        let mut cfg = Pc98Config {
            dos_hdd_path: dos_hdd.to_string_lossy().to_string(),
            ..Default::default()
        };

        // Check Game 1 (YUNO -> SelfBootHdd)
        assert_eq!(games[0].title, "YUNO");
        assert_eq!(games[0].boot_mode, Pc98BootMode::SelfBootHdd);
        apply_game_to_config(&mut games[0], &mut cfg).unwrap();
        assert!(cfg.hdd1_path.ends_with("YUNO.HDI"));
        assert!(cfg.fdd1_path.is_empty());

        // Check Game 2 (SimGame -> DosHddWithFloppy)
        assert_eq!(games[1].title, "SimGame");
        assert_eq!(games[1].boot_mode, Pc98BootMode::DosHddWithFloppy);
        apply_game_to_config(&mut games[1], &mut cfg).unwrap();
        assert_eq!(cfg.hdd1_path, dos_hdd.to_string_lossy());
        assert!(cfg.fdd1_path.ends_with("DISK_1.FDI"));
        assert!(cfg.fdd2_path.ends_with("DISK_2.FDI"));

        // Check Game 3 (Neji -> SelfBootFloppy)
        assert_eq!(games[2].title, "Neji");
        assert_eq!(games[2].boot_mode, Pc98BootMode::SelfBootFloppy);
        apply_game_to_config(&mut games[2], &mut cfg).unwrap();
        assert!(cfg.hdd1_path.is_empty());
        assert!(cfg.fdd1_path.ends_with("neji_1.fdi"));
        assert!(cfg.fdd2_path.ends_with("neji_2.fdi"));
    }

    #[test]
    fn test_floppy_boot_with_game_hdd_and_dos_hdd_with_game_hdd() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let dos_hdd = root.join("Master_MSDOS62.hdi");
        fs::write(&dos_hdd, vec![0u8; 16384]).unwrap();

        // Game A: Boot floppy (起動ディスク.fdi) + Game HDD (game.hdi without AUTOEXEC.BAT)
        let ga_dir = root.join("り").join("Ryuou");
        fs::create_dir_all(&ga_dir).unwrap();
        let mut bs_custom = vec![0xEBu8, 0x1C, 0x90];
        bs_custom.resize(1024, 0x90);
        fs::write(
            ga_dir.join("龍王三国志 起動ディスク.fdi"),
            make_fdi_image(&bs_custom, b"IO      SYS\x07AUTOEXECBAT\x20"),
        )
        .unwrap();
        let mut hdi_no_auto = vec![0u8; 16384];
        hdi_no_auto[8..12].copy_from_slice(&4096u32.to_le_bytes());
        hdi_no_auto[8192..8192 + 12].copy_from_slice(b"IO      SYS\x07");
        fs::write(ga_dir.join("龍王三国志.hdi"), hdi_no_auto).unwrap();

        // Game B: Non-bootable HDD (no IO.SYS in dir, has "NO SYSTEM FILES")
        let gb_dir = root.join("る").join("Ruri");
        fs::create_dir_all(&gb_dir).unwrap();
        let mut hdi_nonboot = vec![0u8; 16384];
        hdi_nonboot[8..12].copy_from_slice(&4096u32.to_le_bytes());
        hdi_nonboot[4096..4096 + 37].copy_from_slice(b"IO      SYSMSDOS   SYSNO SYSTEM FILES");
        fs::write(gb_dir.join("瑠璃色の雪.hdi"), hdi_nonboot).unwrap();

        let mut games = scan_games_directory(root, &dos_hdd.to_string_lossy()).unwrap();
        assert_eq!(games.len(), 2);

        let mut cfg = Pc98Config {
            dos_hdd_path: dos_hdd.to_string_lossy().to_string(),
            ..Default::default()
        };

        // Game A -> FloppyBootWithGameHdd (should NOT defer FDD boot!)
        assert_eq!(games[0].title, "Ryuou");
        assert_eq!(games[0].boot_mode, Pc98BootMode::FloppyBootWithGameHdd);
        apply_game_to_config(&mut games[0], &mut cfg).unwrap();
        assert!(cfg.hdd1_path.ends_with("龍王三国志.hdi"));
        assert!(cfg.fdd1_path.ends_with("龍王三国志 起動ディスク.fdi"));
        assert!(!should_defer_fdd_boot(&cfg));

        // Game B -> DosHddWithGameHdd (HDD1 = Master_MSDOS62.hdi, HDD2 = 瑠璃色の雪.hdi)
        assert_eq!(games[1].title, "Ruri");
        assert_eq!(games[1].boot_mode, Pc98BootMode::DosHddWithGameHdd);
        apply_game_to_config(&mut games[1], &mut cfg).unwrap();
        assert_eq!(cfg.hdd1_path, dos_hdd.to_string_lossy());
        assert!(cfg.hdd2_path.ends_with("瑠璃色の雪.hdi"));
        assert!(cfg.fdd1_path.is_empty());
    }

    #[test]
    fn test_hd_boot_utility_disk_sorted_after_game_disks() {
        let mut disks = vec![
            PathBuf::from("エアーマネジメントⅡ_HD起動.fdi"),
            PathBuf::from("エアーマネジメントⅡ_C.fdi"),
            PathBuf::from("エアーマネジメントⅡ_B.fdi"),
            PathBuf::from("エアーマネジメントⅡ_A.fdi"),
        ];
        order_floppies_and_check_bootable(&mut disks);
        let names: Vec<&str> = disks
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
            .collect();
        assert_eq!(
            names,
            vec![
                "エアーマネジメントⅡ_A.fdi",
                "エアーマネジメントⅡ_B.fdi",
                "エアーマネジメントⅡ_C.fdi",
                "エアーマネジメントⅡ_HD起動.fdi",
            ]
        );
    }

    #[test]
    fn test_generic_dos_boot_floppy_ignored_when_hdd_has_autoexec() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let dos_hdd = root.join("Master_MSDOS62.hdi");
        fs::write(&dos_hdd, vec![0u8; 16384]).unwrap();

        let eve_dir = root.join("い").join("EVE burst error");
        fs::create_dir_all(&eve_dir).unwrap();

        // Bootable DOS floppy WITHOUT AUTOEXEC.BAT (DOS62.FDI)
        let mut bs_dos = vec![0xEBu8, 0x1C, 0x90];
        bs_dos.resize(1024, 0x90);
        fs::write(
            eve_dir.join("DOS62.FDI"),
            make_fdi_image(&bs_dos, b"IO      SYS\x07\x00\x00\x00\x00MSDOS   SYS\x07"),
        )
        .unwrap();

        // Self-bootable EVE.hdi WITH AUTOEXEC.BAT and EVE.BAT in root dir
        let mut hdi = vec![0u8; 16384];
        hdi[8..12].copy_from_slice(&4096u32.to_le_bytes());
        let mut root_ent = vec![0u8; 32 * 4];
        root_ent[0..12].copy_from_slice(b"IO      SYS\x27");
        root_ent[32..44].copy_from_slice(b"MSDOS   SYS\x27");
        root_ent[64..76].copy_from_slice(b"AUTOEXECBAT\x20");
        root_ent[96..108].copy_from_slice(b"EVE     BAT\x20");
        hdi[8192..8192 + root_ent.len()].copy_from_slice(&root_ent);
        fs::write(eve_dir.join("EVE.hdi"), hdi).unwrap();

        let mut games = scan_games_directory(root, &dos_hdd.to_string_lossy()).unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].boot_mode, Pc98BootMode::SelfBootHdd);

        let mut cfg = Pc98Config {
            dos_hdd_path: dos_hdd.to_string_lossy().to_string(),
            ..Default::default()
        };
        apply_game_to_config(&mut games[0], &mut cfg).unwrap();
        assert!(cfg.hdd1_path.ends_with("EVE.hdi"));
        assert!(cfg.fdd1_path.is_empty());
        assert_eq!(detect_startup_command(&cfg).as_deref(), Some("eve\n"));
    }
}
