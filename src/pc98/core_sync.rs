use super::{Pc98Config, Pc98Event, Pc98MediaSlot};
use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const NP2KAI_CORE_URL: &str =
    "https://buildbot.libretro.com/nightly/linux/x86_64/latest/np2kai_libretro.so.zip";

const EMBEDDED_JP_FONT: &[u8] = include_bytes!("../../assets/NotoSansJP-Regular.ttf");
pub const EMBEDDED_FDDSEEK_WAV: &[u8] = include_bytes!("../../assets/fddseek.wav");
pub const EMBEDDED_FDDSEEK1_WAV: &[u8] = include_bytes!("../../assets/fddseek1.wav");

pub fn pc98_base_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.trim().is_empty() {
            return PathBuf::from(xdg).join("michadame").join("pc98");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("michadame")
                .join("pc98");
        }
    }
    std::env::temp_dir().join("michadame-pc98")
}

pub fn cores_dir() -> PathBuf {
    pc98_base_dir().join("cores")
}

pub fn system_dir() -> PathBuf {
    pc98_base_dir().join("system")
}

pub fn saves_dir() -> PathBuf {
    pc98_base_dir().join("saves")
}

pub fn np2kai_system_subdir() -> PathBuf {
    system_dir().join("np2kai")
}

/// Ensures the Libretro system and save directories exist and seeds `np2kai/`
/// with `default.ttf` (from Michadame's embedded NotoSansJP) plus any local PC-98
/// font/ROM files discovered on the machine.
pub fn ensure_system_dir() -> Result<PathBuf> {
    let sys_dir = system_dir();
    let np2_dir = np2kai_system_subdir();
    let save_dir = saves_dir();

    fs::create_dir_all(&np2_dir)
        .with_context(|| format!("Failed to create PC-98 system dir {:?}", np2_dir))?;
    fs::create_dir_all(&save_dir)
        .with_context(|| format!("Failed to create PC-98 saves dir {:?}", save_dir))?;

    // 1. Seed default.ttf so fontmng_init() and text-VRAM Kanji rendering always work out of the box
    let default_ttf = np2_dir.join("default.ttf");
    if !default_ttf.exists() {
        let _ = fs::write(&default_ttf, EMBEDDED_JP_FONT);
    }

    // 2. Remove conflicting PC-9821 / IDE / SASI / SCSI expansion ROMs from np2_dir.
    // When bios9821.rom, d8000.rom, or ide.rom is present in system/np2kai/, NP2kai maps them into
    // the 0xD8000 expansion ROM window even on PC-9801VX, causing disk bootstrap to jump into an
    // incompatible expansion ROM and hang on a black screen after the memory check.
    for conflicting_rom in [
        "bios9821.rom",
        "BIOS9821.ROM",
        "d8000.rom",
        "D8000.ROM",
        "ide.rom",
        "IDE.ROM",
        "pci.rom",
        "PCI.ROM",
        "sasi.rom",
        "SASI.ROM",
        "scsi.rom",
        "SCSI.ROM",
    ] {
        let p = np2_dir.join(conflicting_rom);
        if p.exists() {
            let _ = fs::remove_file(&p);
        }
    }

    // 3. Opportunistically seed FONT.ROM / font.bmp / sound.rom / BIOS ROMs if user has NekoProject2 or RetroArch files
    seed_optional_rom_files(&np2_dir);

    // 4. Seed authentic NP2 floppy stepper motor seek WAV samples (fddseek.wav / fddseek1.wav)
    let fddseek_wav = np2_dir.join("fddseek.wav");
    if !fddseek_wav.exists() {
        let _ = fs::write(&fddseek_wav, EMBEDDED_FDDSEEK_WAV);
    }
    let fddseek1_wav = np2_dir.join("fddseek1.wav");
    if !fddseek1_wav.exists() {
        let _ = fs::write(&fddseek1_wav, EMBEDDED_FDDSEEK1_WAV);
    }

    Ok(sys_dir)
}

fn seed_optional_rom_files(np2_dir: &Path) {
    let mut candidate_dirs = vec![np2_dir.to_path_buf()];
    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(home);
        candidate_dirs.push(home_path.join("Games/NekoProject2"));
        candidate_dirs.push(home_path.join("Games/NekoProject2/np21w-0.86-rev92/bin"));
        candidate_dirs.push(home_path.join(".config/retroarch/system/np2kai"));
        candidate_dirs.push(home_path.join(".config/np2kai"));
        candidate_dirs.push(home_path.join(".config/xnp2"));
    }

    let rom_filenames = [
        ("FONT.ROM", "FONT.ROM"),
        ("font.rom", "font.rom"),
        ("font.bmp", "font.bmp"),
        ("font.tmp", "font.bmp"),
        ("bios.rom", "bios.rom"),
        ("BIOS.ROM", "bios.rom"),
        ("sound.rom", "sound.rom"),
        ("SOUND.ROM", "sound.rom"),
        ("itf.rom", "itf.rom"),
        ("ITF.ROM", "itf.rom"),
        ("fddseek.wav", "fddseek.wav"),
        ("FDDSEEK.WAV", "fddseek.wav"),
        ("fddseek1.wav", "fddseek1.wav"),
        ("FDDSEEK1.WAV", "fddseek1.wav"),
        ("2608_bd.wav", "2608_bd.wav"),
        ("2608_sd.wav", "2608_sd.wav"),
        ("2608_top.wav", "2608_top.wav"),
        ("2608_hh.wav", "2608_hh.wav"),
        ("2608_tom.wav", "2608_tom.wav"),
        ("2608_rim.wav", "2608_rim.wav"),
        ("bios_NEC_PC-98_2608_bd.wav", "2608_bd.wav"),
        ("bios_NEC_PC-98_2608_sd.wav", "2608_sd.wav"),
        ("bios_NEC_PC-98_2608_top.wav", "2608_top.wav"),
        ("bios_NEC_PC-98_2608_hh.wav", "2608_hh.wav"),
        ("bios_NEC_PC-98_2608_tom.wav", "2608_tom.wav"),
        ("bios_NEC_PC-98_2608_rim.wav", "2608_rim.wav"),
    ];

    for dir in candidate_dirs {
        if !dir.is_dir() {
            continue;
        }
        for &(src_name, dst_name) in &rom_filenames {
            let dst = np2_dir.join(dst_name);
            if dst.exists() {
                continue;
            }
            let src = dir.join(src_name);
            if src.is_file() {
                if fs::copy(&src, &dst).is_ok() {
                    tracing::info!("Seeded PC-98 system file {:?} from {:?}", dst, src);
                }
            }
        }
    }
}

/// Ensures `np2kai_libretro.so` is present in `~/.local/share/michadame/pc98/cores/`,
/// downloading and extracting it from the official Libretro buildbot if needed.
pub fn ensure_np2kai_core(event_tx: Option<&Sender<Pc98Event>>) -> Result<PathBuf> {
    let dir = cores_dir();
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create cores directory {:?}", dir))?;

    let target_so = dir.join("np2kai_libretro.so");
    if target_so.is_file()
        && fs::metadata(&target_so)
            .map(|m| m.len() > 100_000)
            .unwrap_or(false)
    {
        return Ok(target_so);
    }

    // Check if RetroArch or system libretro already has np2kai_libretro.so installed
    let mut local_candidates = vec![
        PathBuf::from("/usr/lib/libretro/np2kai_libretro.so"),
        PathBuf::from("/usr/lib64/libretro/np2kai_libretro.so"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        local_candidates.push(
            PathBuf::from(home).join(".config/retroarch/cores/np2kai_libretro.so"),
        );
    }
    for cand in local_candidates {
        if cand.is_file() {
            if fs::copy(&cand, &target_so).is_ok() {
                set_executable(&target_so);
                return Ok(target_so);
            }
        }
    }

    if let Some(tx) = event_tx {
        let _ = tx.send(Pc98Event::SyncProgress(
            "Downloading NP2kai PC-98 core from Libretro buildbot...".to_string(),
        ));
    }
    tracing::info!("Downloading NP2kai Libretro core from {}", NP2KAI_CORE_URL);

    let response = ureq::get(NP2KAI_CORE_URL)
        .timeout(Duration::from_secs(30))
        .call()
        .context("Failed to download np2kai_libretro.so.zip from Libretro buildbot")?;

    let mut zip_bytes = Vec::with_capacity(1_500_000);
    response
        .into_reader()
        .take(25 * 1024 * 1024)
        .read_to_end(&mut zip_bytes)
        .context("Failed to read np2kai_libretro.so.zip stream")?;

    extract_core_zip(&zip_bytes, &target_so)?;
    Ok(target_so)
}

pub fn extract_core_zip(zip_bytes: &[u8], target_so: &Path) -> Result<()> {
    let reader = Cursor::new(zip_bytes);
    let mut archive =
        zip::ZipArchive::new(reader).context("Failed to open np2kai_libretro.so.zip archive")?;

    let mut found = false;
    let tmp_so = target_so.with_extension(format!("so.tmp-{}", std::process::id()));

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if entry.name().ends_with(".so") {
            let mut out = fs::File::create(&tmp_so)
                .with_context(|| format!("Failed to create temp core file {:?}", tmp_so))?;
            std::io::copy(&mut entry, &mut out)?;
            out.flush()?;
            found = true;
            break;
        }
    }

    if !found {
        let _ = fs::remove_file(&tmp_so);
        anyhow::bail!("No .so file found inside np2kai_libretro.so.zip");
    }

    set_executable(&tmp_so);
    fs::rename(&tmp_so, target_so)
        .with_context(|| format!("Failed to move {:?} to {:?}", tmp_so, target_so))?;

    Ok(())
}

fn set_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
    }
}

/// Builds a temporary `.cmd` file for `np2kai` (`np2_main` command-line arguments).
/// When `hdd1_path` is present, floppy disks (`fdd1_path` / `fdd2_path`) are omitted from
/// power-on arguments and mounted shortly after POST via Libretro's disk control interface
/// so a non-bootable data/installer floppy in FDD 1 never hijacks the BIOS boot order away
/// from HDD 1.
pub fn prepare_boot_cmd(config: &Pc98Config) -> Result<Option<PathBuf>> {
    let mut args: Vec<String> = Vec::new();
    let defer_fdd = super::library::should_defer_fdd_boot(config);

    for (slot, path_str) in [
        (Pc98MediaSlot::Hdd1, config.hdd1_path.trim()),
        (Pc98MediaSlot::Hdd2, config.hdd2_path.trim()),
        (Pc98MediaSlot::CdRom, config.cdrom_path.trim()),
        (Pc98MediaSlot::Fdd1, config.fdd1_path.trim()),
        (Pc98MediaSlot::Fdd2, config.fdd2_path.trim()),
    ] {
        if defer_fdd && matches!(slot, Pc98MediaSlot::Fdd1 | Pc98MediaSlot::Fdd2) {
            continue;
        }
        if !path_str.is_empty() {
            let p = Path::new(path_str);
            if p.exists() {
                args.push(format!("\"{}\"", p.to_string_lossy()));
            }
        }
    }

    if args.is_empty() {
        // Always pass a harmless placeholder argument so np2_main enters its argv loop
        // and registers RETRO_ENVIRONMENT_SET_DISK_CONTROL_INTERFACE for live FDD swapping.
        args.push("dummy.txt".to_string());
    }

    let base = pc98_base_dir();
    fs::create_dir_all(&base)?;
    let cmd_path = base.join("boot.cmd");
    let line = format!("np2kai {}", args.join(" "));
    fs::write(&cmd_path, line)
        .with_context(|| format!("Failed to write PC-98 boot command file {:?}", cmd_path))?;

    Ok(Some(cmd_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    #[test]
    fn test_extract_core_zip_roundtrip() {
        let mut zip_buf = Vec::new();
        {
            let mut zip_writer = zip::ZipWriter::new(Cursor::new(&mut zip_buf));
            zip_writer
                .start_file("np2kai_libretro.so", SimpleFileOptions::default())
                .unwrap();
            zip_writer.write_all(b"\x7fELF-dummy-np2kai-core").unwrap();
            zip_writer.finish().unwrap();
        }

        let tmp_dir = tempfile::tempdir().unwrap();
        let target_so = tmp_dir.path().join("np2kai_libretro.so");
        extract_core_zip(&zip_buf, &target_so).unwrap();

        assert!(target_so.exists());
        assert_eq!(
            fs::read(&target_so).unwrap(),
            b"\x7fELF-dummy-np2kai-core"
        );
    }
}
