use super::Pc88Event;
use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// URL for the official Libretro nightly Linux x86_64 Quasi88 core archive.
pub const QUASI88_LIBRETRO_URL: &str =
    "https://buildbot.libretro.com/nightly/linux/x86_64/latest/quasi88_libretro.so.zip";

/// Returns the base data directory used for PC-88 cores, BIOS/ROMs, and saves:
/// `~/.local/share/michadame/pc88` (falling back to `/tmp/michadame-pc88`).
pub fn pc88_base_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.trim().is_empty() {
            return PathBuf::from(xdg).join("michadame").join("pc88");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("michadame")
                .join("pc88");
        }
    }
    std::env::temp_dir().join("michadame-pc88")
}

pub fn pc88_cores_dir() -> PathBuf {
    pc88_base_dir().join("cores")
}

pub fn pc88_system_dir() -> PathBuf {
    pc88_base_dir().join("system")
}

pub fn pc88_saves_dir() -> PathBuf {
    pc88_base_dir().join("saves")
}

pub fn pc88_disk_cache_dir() -> PathBuf {
    pc88_base_dir().join("disk_cache")
}

/// Ensures the Libretro system directory `<system_dir>/quasi88` exists and seeds it with
/// any PC-88 BIOS/Kanji/Dictionary ROMs found on the system or inside the user's `games_dir`
/// (including automatically splitting 112KB `PC88.ROM` bundles into Quasi88's individual ROM files).
pub fn ensure_pc88_system_dir(games_dir: Option<&str>) -> Result<PathBuf> {
    let sys_dir = pc88_system_dir();
    let quasi88_dir = sys_dir.join("quasi88");
    fs::create_dir_all(&quasi88_dir).with_context(|| {
        format!(
            "Failed to create PC-88 system directory {}",
            quasi88_dir.display()
        )
    })?;
    fs::create_dir_all(pc88_saves_dir()).ok();
    fs::create_dir_all(pc88_disk_cache_dir()).ok();

    seed_pc88_roms(&quasi88_dir, games_dir);

    Ok(sys_dir)
}

/// Splits a combined 112KB (`114,688` bytes = `0x1C000`) M88/P88SR `PC88.ROM` file into the
/// individual ROM files expected by `quasi88_libretro` (matching `memory_allocate` in Quasi88):
/// - `0x00000..0x08000` (32KB) -> `N88.ROM`
/// - `0x0C000..0x0E000` (8KB)  -> `N88_0.ROM`
/// - `0x0E000..0x10000` (8KB)  -> `N88_1.ROM`
/// - `0x10000..0x12000` (8KB)  -> `N88_2.ROM`
/// - `0x12000..0x14000` (8KB)  -> `N88_3.ROM`
/// - `0x14000..0x16000` (8KB)  -> `DISK.ROM`
/// - `0x16000..0x1C000` (24KB) + `0x08000..0x0A000` (8KB) -> `N80.ROM` (32KB)
pub fn split_combined_pc88_rom(pc88_rom_bytes: &[u8], dest_dir: &Path) -> Result<()> {
    if pc88_rom_bytes.len() < 0x1C000 {
        anyhow::bail!(
            "PC88.ROM is too small ({} bytes, expected >= 114688)",
            pc88_rom_bytes.len()
        );
    }

    let write_if_different = |name: &str, bytes: &[u8]| -> Result<()> {
        let target = dest_dir.join(name);
        if fs::read(&target).ok().as_deref() != Some(bytes) {
            fs::write(&target, bytes)?;
        }
        Ok(())
    };

    write_if_different("N88.ROM", &pc88_rom_bytes[0x00000..0x08000])?;
    write_if_different("N88_0.ROM", &pc88_rom_bytes[0x0C000..0x0E000])?;
    write_if_different("N88_1.ROM", &pc88_rom_bytes[0x0E000..0x10000])?;
    write_if_different("N88_2.ROM", &pc88_rom_bytes[0x10000..0x12000])?;
    write_if_different("N88_3.ROM", &pc88_rom_bytes[0x12000..0x14000])?;
    write_if_different("DISK.ROM", &pc88_rom_bytes[0x14000..0x14800])?;

    let mut n80 = Vec::with_capacity(0x8000);
    n80.extend_from_slice(&pc88_rom_bytes[0x16000..0x1C000]);
    n80.extend_from_slice(&pc88_rom_bytes[0x08000..0x0A000]);
    write_if_different("N80.ROM", &n80)?;

    Ok(())
}

fn seed_pc88_roms(quasi88_dir: &Path, games_dir: Option<&str>) {
    let mut split_bundle_done = false;
    let mut candidate_dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(&home);
        candidate_dirs.push(home_path.join(".config/retroarch/system/quasi88"));
        candidate_dirs.push(home_path.join(".config/retroarch/system"));
        candidate_dirs.push(home_path.join(".quasi88/rom"));
        candidate_dirs.push(home_path.join(".quasi88"));
        candidate_dirs.push(home_path.join("Games/PC88/ROM"));
        candidate_dirs.push(home_path.join("Games/PC88/bios"));
    }

    let mut game_roots = Vec::new();
    if let Some(gdir) = games_dir {
        let trimmed = gdir.trim();
        if !trimmed.is_empty() {
            game_roots.push(PathBuf::from(trimmed));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let default_gamelist = PathBuf::from(home).join("Games/PC88/GameList");
        if default_gamelist.is_dir() && !game_roots.contains(&default_gamelist) {
            game_roots.push(default_gamelist);
        }
    }

    for root in &game_roots {
        candidate_dirs.push(root.clone());
        candidate_dirs.push(root.join("R/rom_88fa"));
        candidate_dirs.push(root.join("R/rom_88ma2"));
        candidate_dirs.push(root.join("R/rom_98dop"));
        candidate_dirs.push(root.join("K/kanji_88"));
        candidate_dirs.push(root.join("M/mc"));
        candidate_dirs.push(root.join("R/rythm_98"));
    }

    for src_dir in candidate_dirs {
        if !src_dir.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(&src_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(fname) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let upper = fname.to_ascii_uppercase();

            // Combined 112KB PC88.ROM bundle
            if upper == "PC88.ROM" {
                if !split_bundle_done {
                    if let Ok(bytes) = fs::read(&path) {
                        if bytes.len() >= 0x1C000
                            && split_combined_pc88_rom(&bytes, quasi88_dir).is_ok()
                        {
                            split_bundle_done = true;
                        }
                    }
                }
                continue;
            }

            // Individual ROMs and rhythm samples
            let canonical_dest = match upper.as_str() {
                "N88.ROM" | "N88_0.ROM" | "N88_1.ROM" | "N88_2.ROM" | "N88_3.ROM"
                | "N88EXT0.ROM" | "N88EXT1.ROM" | "N88EXT2.ROM" | "N88EXT3.ROM" | "N80.ROM"
                | "N88N.ROM" | "DISK.ROM" | "N88SUB.ROM" | "KANJI1.ROM" | "N88KNJ1.ROM"
                | "KANJI2.ROM" | "N88KNJ2.ROM" | "JISYO.ROM" | "N88JISHO.ROM" | "FONT.ROM"
                | "FONT2.ROM" | "FONT3.ROM" => Some(upper),
                s if s.starts_with("2608_") && s.ends_with(".WAV") => Some(upper),
                _ => None,
            };

            if let Some(dest_name) = canonical_dest {
                let dest = quasi88_dir.join(dest_name);
                if !dest.exists() {
                    let _ = fs::copy(&path, &dest);
                }
            }
        }
    }
}

/// Ensures `quasi88_libretro.so` is present in `~/.local/share/michadame/pc88/cores/`,
/// copying from a local RetroArch installation if available or downloading from Libretro buildbot.
pub fn ensure_quasi88_core(event_tx: Option<&Sender<Pc88Event>>) -> Result<PathBuf> {
    let cores_dir = pc88_cores_dir();
    fs::create_dir_all(&cores_dir).with_context(|| {
        format!(
            "Failed to create PC-88 cores directory {}",
            cores_dir.display()
        )
    })?;

    let target_so = cores_dir.join("quasi88_libretro.so");
    if target_so.is_file() {
        if let Ok(meta) = fs::metadata(&target_so) {
            if meta.len() > 200_000 {
                return Ok(target_so);
            }
        }
    }

    // Check existing local RetroArch / system cores first
    let mut local_candidates = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let hp = PathBuf::from(home);
        local_candidates.push(hp.join(".config/retroarch/cores/quasi88_libretro.so"));
        local_candidates.push(
            hp.join(".var/app/org.libretro.RetroArch/config/retroarch/cores/quasi88_libretro.so"),
        );
    }
    local_candidates.push(PathBuf::from("/usr/lib/libretro/quasi88_libretro.so"));
    local_candidates.push(PathBuf::from("/usr/lib64/libretro/quasi88_libretro.so"));
    local_candidates.push(PathBuf::from(
        "/usr/lib/x86_64-linux-gnu/libretro/quasi88_libretro.so",
    ));

    for cand in local_candidates {
        if cand.is_file() {
            if let Ok(meta) = fs::metadata(&cand) {
                if meta.len() > 200_000 {
                    fs::copy(&cand, &target_so)?;
                    return Ok(target_so);
                }
            }
        }
    }

    if let Some(tx) = event_tx {
        let _ = tx.send(Pc88Event::SyncProgress(
            "Downloading NEC PC-88 core (quasi88_libretro.so) from Libretro buildbot..."
                .to_string(),
        ));
    }
    tracing::info!(
        "Downloading Quasi88 Libretro core from {}",
        QUASI88_LIBRETRO_URL
    );

    let response = ureq::get(QUASI88_LIBRETRO_URL)
        .timeout(std::time::Duration::from_secs(60))
        .call()
        .context("Failed to download quasi88_libretro.so.zip from Libretro buildbot")?;

    let mut zip_bytes = Vec::with_capacity(1_500_000);
    response
        .into_reader()
        .take(25 * 1024 * 1024)
        .read_to_end(&mut zip_bytes)
        .context("Failed to read quasi88_libretro.so.zip payload")?;

    if let Some(tx) = event_tx {
        let _ = tx.send(Pc88Event::SyncProgress(
            "Extracting quasi88_libretro.so...".to_string(),
        ));
    }

    let cursor = std::io::Cursor::new(&zip_bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).context("Failed to open quasi88_libretro.so.zip archive")?;

    let mut extracted = false;
    let tmp_so = cores_dir.join("quasi88_libretro.so.tmp");
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if entry.name().ends_with("quasi88_libretro.so") {
            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut buf)?;
            fs::write(&tmp_so, &buf)
                .with_context(|| format!("Failed to write {}", tmp_so.display()))?;
            fs::rename(&tmp_so, &target_so)
                .with_context(|| format!("Failed to move {} into place", target_so.display()))?;
            extracted = true;
            break;
        }
    }

    if !extracted {
        anyhow::bail!("quasi88_libretro.so was not found inside downloaded archive");
    }

    tracing::info!("Quasi88 Libretro core installed at {}", target_so.display());
    Ok(target_so)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_combined_pc88_rom() {
        let mut fake_rom = vec![0u8; 0x1C000];
        fake_rom[0x00000] = 0xAA; // N88.ROM start
        fake_rom[0x0C000] = 0xBB; // N88_0.ROM start
        fake_rom[0x0E000] = 0xB1; // N88_1.ROM start
        fake_rom[0x10000] = 0xB2; // N88_2.ROM start
        fake_rom[0x12000] = 0xB3; // N88_3.ROM start
        fake_rom[0x14000] = 0xCC; // DISK.ROM start
        fake_rom[0x16000] = 0xDD; // N80.ROM part 1 start
        fake_rom[0x08000] = 0xEE; // N80.ROM part 2 start

        let tmp =
            std::env::temp_dir().join(format!("michadame_pc88_rom_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        split_combined_pc88_rom(&fake_rom, &tmp).unwrap();

        let n88 = fs::read(tmp.join("N88.ROM")).unwrap();
        assert_eq!(n88.len(), 0x8000);
        assert_eq!(n88[0], 0xAA);

        let n88_0 = fs::read(tmp.join("N88_0.ROM")).unwrap();
        assert_eq!(n88_0.len(), 0x2000);
        assert_eq!(n88_0[0], 0xBB);

        let n88_1 = fs::read(tmp.join("N88_1.ROM")).unwrap();
        assert_eq!(n88_1.len(), 0x2000);
        assert_eq!(n88_1[0], 0xB1);

        let n88_2 = fs::read(tmp.join("N88_2.ROM")).unwrap();
        assert_eq!(n88_2.len(), 0x2000);
        assert_eq!(n88_2[0], 0xB2);

        let n88_3 = fs::read(tmp.join("N88_3.ROM")).unwrap();
        assert_eq!(n88_3.len(), 0x2000);
        assert_eq!(n88_3[0], 0xB3);

        let disk = fs::read(tmp.join("DISK.ROM")).unwrap();
        assert_eq!(disk.len(), 0x800);
        assert_eq!(disk[0], 0xCC);

        let n80 = fs::read(tmp.join("N80.ROM")).unwrap();
        assert_eq!(n80.len(), 0x8000);
        assert_eq!(n80[0], 0xDD);
        assert_eq!(n80[0x6000], 0xEE);

        let _ = fs::remove_dir_all(&tmp);
    }
}
