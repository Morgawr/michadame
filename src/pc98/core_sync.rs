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

pub const CONFLICTING_9801_EXPANSION_ROMS: &[&str] = &[
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
];

fn is_conflicting_expansion_rom_for_model(file_name: &str, model: &str) -> bool {
    let model_upper = model.trim().to_ascii_uppercase();
    let is_9821 = model_upper.contains("9821");
    if is_9821 && file_name.eq_ignore_ascii_case("bios9821.rom") {
        return false;
    }
    CONFLICTING_9801_EXPANSION_ROMS
        .iter()
        .any(|cand| file_name.eq_ignore_ascii_case(cand))
}

fn sanitize_model_dir_slug(model: &str) -> String {
    let mut slug = String::new();
    for ch in model.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('_') && !slug.is_empty() {
            slug.push('_');
        }
    }
    let trimmed = slug.trim_matches('_');
    if trimmed.is_empty() {
        "vx".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Ensures the user's canonical Libretro system (`<base>/system/np2kai`) and save directories exist,
/// seeds missing default font/audio assets without deleting any user-supplied ROMs, and constructs
/// a model-specific runtime system directory (`<base>/runtime_system/<model>/np2kai`) that exposes
/// only the ROMs compatible with `model`.
pub fn ensure_system_dir() -> Result<PathBuf> {
    ensure_system_dir_for_model("VX")
}

/// Prepares the PC-98 system directory for a specific `np2kai_model` without deleting any user
/// ROM files from the source `system/np2kai` directory.
pub fn ensure_system_dir_for_model(model: &str) -> Result<PathBuf> {
    prepare_system_dir_in(&pc98_base_dir(), model, true)
}

/// Prepares the PC-98 system directory inside `base_dir` for `model`.
/// All user-supplied ROM files in `<base_dir>/system/np2kai` are preserved intact.
/// Returns the model-specific runtime system root (`<base_dir>/runtime_system/<model_slug>`) whose
/// `np2kai/` subdirectory contains only the model-compatible ROM set.
pub fn prepare_system_dir_in(
    base_dir: &Path,
    model: &str,
    seed_from_host: bool,
) -> Result<PathBuf> {
    let np2_source_dir = base_dir.join("system").join("np2kai");
    let save_dir = base_dir.join("saves");

    fs::create_dir_all(&np2_source_dir)
        .with_context(|| format!("Failed to create PC-98 system dir {:?}", np2_source_dir))?;
    fs::create_dir_all(&save_dir)
        .with_context(|| format!("Failed to create PC-98 saves dir {:?}", save_dir))?;

    // 1. Seed default.ttf in source dir if missing
    let default_ttf = np2_source_dir.join("default.ttf");
    if !default_ttf.exists() {
        let _ = fs::write(&default_ttf, EMBEDDED_JP_FONT);
    }

    // 2. Opportunistically seed FONT.ROM / font.bmp / sound.rom / BIOS ROMs if missing (never overwrites or deletes)
    if seed_from_host {
        seed_optional_rom_files(&np2_source_dir);
    }

    // 3. Seed authentic NP2 floppy stepper motor seek WAV samples if missing
    let fddseek_wav = np2_source_dir.join("fddseek.wav");
    if !fddseek_wav.exists() {
        let _ = fs::write(&fddseek_wav, EMBEDDED_FDDSEEK_WAV);
    }
    let fddseek1_wav = np2_source_dir.join("fddseek1.wav");
    if !fddseek1_wav.exists() {
        let _ = fs::write(&fddseek1_wav, EMBEDDED_FDDSEEK1_WAV);
    }

    // 4. Construct a model-specific runtime directory that exposes only compatible ROMs
    // while keeping every user file in `np2_source_dir` completely untouched.
    let model_slug = sanitize_model_dir_slug(model);
    let runtime_sys_dir = base_dir.join("runtime_system").join(&model_slug);
    let runtime_np2_dir = runtime_sys_dir.join("np2kai");
    fs::create_dir_all(&runtime_np2_dir).with_context(|| {
        format!(
            "Failed to create PC-98 runtime system dir {:?}",
            runtime_np2_dir
        )
    })?;

    // Clean any previously synced files in the owned runtime directory that are incompatible or stale
    if let Ok(entries) = fs::read_dir(&runtime_np2_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(fname) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let src_path = np2_source_dir.join(fname);
            if !src_path.is_file() || is_conflicting_expansion_rom_for_model(fname, model) {
                let _ = fs::remove_file(&path);
            }
        }
    }

    // Sync compatible files from `np2_source_dir` into `runtime_np2_dir`
    if let Ok(entries) = fs::read_dir(&np2_source_dir) {
        for entry in entries.flatten() {
            let src_path = entry.path();
            if !src_path.is_file() {
                continue;
            }
            let Some(fname) = src_path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if is_conflicting_expansion_rom_for_model(fname, model) {
                continue;
            }
            let dst_path = runtime_np2_dir.join(fname);
            let needs_copy = match (fs::metadata(&src_path), fs::metadata(&dst_path)) {
                (Ok(src_m), Ok(dst_m)) => src_m.len() != dst_m.len(),
                (Ok(_), Err(_)) => true,
                _ => false,
            };
            if needs_copy {
                let _ = fs::copy(&src_path, &dst_path);
            }
        }
    }

    Ok(runtime_sys_dir)
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
        local_candidates
            .push(PathBuf::from(home).join(".config/retroarch/cores/np2kai_libretro.so"));
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
        assert_eq!(fs::read(&target_so).unwrap(), b"\x7fELF-dummy-np2kai-core");
    }

    #[test]
    fn test_prepare_system_dir_preserves_all_user_roms_across_models() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let base_dir = tmp_dir.path();
        let source_np2_dir = base_dir.join("system").join("np2kai");
        fs::create_dir_all(&source_np2_dir).unwrap();

        let mut sentinels = Vec::new();
        for (idx, &rom_name) in CONFLICTING_9801_EXPANSION_ROMS.iter().enumerate() {
            let sentinel = format!("USER_ROM_SENTINEL_{idx}_{rom_name}").into_bytes();
            fs::write(source_np2_dir.join(rom_name), &sentinel).unwrap();
            sentinels.push((rom_name, sentinel));
        }
        let main_bios_sentinel = b"USER_MAIN_BIOS_ROM_DATA".to_vec();
        fs::write(source_np2_dir.join("bios.rom"), &main_bios_sentinel).unwrap();

        for model in ["PC-9801VX", "PC-9801VM", "PC-286", "PC-9821"] {
            let runtime_sys = prepare_system_dir_in(base_dir, model, false).unwrap();
            let runtime_np2 = runtime_sys.join("np2kai");
            assert!(runtime_np2.is_dir());

            // Every original user ROM in system/np2kai must remain intact with its exact sentinel bytes
            for (rom_name, expected_bytes) in &sentinels {
                let user_rom_path = source_np2_dir.join(rom_name);
                assert!(
                    user_rom_path.is_file(),
                    "User ROM {rom_name} was deleted when preparing model {model}"
                );
                assert_eq!(
                    &fs::read(&user_rom_path).unwrap(),
                    expected_bytes,
                    "User ROM {rom_name} contents changed when preparing model {model}"
                );
            }
            assert_eq!(
                fs::read(source_np2_dir.join("bios.rom")).unwrap(),
                main_bios_sentinel
            );

            // Runtime directory must have bios.rom and default.ttf
            assert_eq!(
                fs::read(runtime_np2.join("bios.rom")).unwrap(),
                main_bios_sentinel
            );
            assert!(runtime_np2.join("default.ttf").is_file());

            // On PC-9801VX / VM / PC-286, conflicting expansion ROMs must not be exposed in runtime_np2
            if !model.contains("9821") {
                for &(rom_name, _) in &sentinels {
                    assert!(
                        !runtime_np2.join(rom_name).exists(),
                        "Conflicting expansion ROM {rom_name} should not be exposed in runtime dir for {model}"
                    );
                }
            } else {
                // On PC-9821, bios9821.rom is allowed in the runtime directory
                assert!(
                    runtime_np2.join("bios9821.rom").is_file()
                        || runtime_np2.join("BIOS9821.ROM").is_file()
                );
            }
        }
    }
}
