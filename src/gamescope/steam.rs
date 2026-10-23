use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SteamGame {
    pub appid: String,
    pub name: String,
}

impl SteamGame {
    pub fn steam_launch_args(&self) -> Vec<String> {
        vec!["-applaunch".to_string(), self.appid.clone()]
    }
}

pub fn default_launcher_script_path() -> String {
    if let Ok(home) = std::env::var("HOME") {
        let home_script = PathBuf::from(home).join("bin/michadame-gamescope-steam.sh");
        if home_script.is_file() {
            return home_script.to_string_lossy().to_string();
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let repo_script = cwd.join("scripts/michadame-gamescope-steam.sh");
        if repo_script.is_file() {
            return repo_script.to_string_lossy().to_string();
        }
    }
    String::new()
}

pub fn find_steam_roots() -> Vec<PathBuf> {
    let Ok(home) = std::env::var("HOME") else {
        return Vec::new();
    };
    let home_path = PathBuf::from(home);
    let candidates = [
        home_path.join(".local/share/Steam"),
        home_path.join(".steam/steam"),
        home_path.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ];
    let mut roots = Vec::new();
    let mut seen_canonical = Vec::new();
    for cand in candidates {
        if cand.is_dir() {
            let canon = cand.canonicalize().unwrap_or_else(|_| cand.clone());
            if !seen_canonical.contains(&canon) {
                seen_canonical.push(canon);
                roots.push(cand);
            }
        }
    }
    roots
}

pub fn is_filtered_steam_tool(appid: &str, name: &str) -> bool {
    if appid == "228980" {
        return true;
    }
    let trimmed = name.trim();
    trimmed.starts_with("Proton ")
        || trimmed.starts_with("Proton -")
        || trimmed.starts_with("Steam Linux Runtime")
        || trimmed.eq_ignore_ascii_case("Steamworks Common Redistributables")
}

pub fn parse_vdf_kv_line(line: &str) -> Option<(String, String)> {
    let mut chars = line.chars().peekable();
    let mut tokens = Vec::with_capacity(2);

    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }
        if ch == '"' {
            chars.next();
            let mut val = String::new();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(escaped) = chars.next() {
                        val.push(escaped);
                    }
                } else if c == '"' {
                    break;
                } else {
                    val.push(c);
                }
            }
            tokens.push(val);
            if tokens.len() == 2 {
                return Some((tokens.remove(0), tokens.remove(0)));
            }
        } else {
            break;
        }
    }
    None
}

pub fn parse_library_folders_vdf(content: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in content.lines() {
        if let Some((k, v)) = parse_vdf_kv_line(line) {
            if k.eq_ignore_ascii_case("path") && !v.trim().is_empty() {
                paths.push(PathBuf::from(v));
            }
        }
    }
    paths
}

pub fn parse_appmanifest_acf(content: &str) -> Option<SteamGame> {
    let mut appid = None;
    let mut name = None;
    for line in content.lines() {
        if let Some((k, v)) = parse_vdf_kv_line(line) {
            if k.eq_ignore_ascii_case("appid") {
                appid = Some(v);
            } else if k.eq_ignore_ascii_case("name") {
                name = Some(v);
            }
        }
    }
    let appid = appid?.trim().to_string();
    let name = name?.trim().to_string();
    if appid.is_empty() || name.is_empty() || is_filtered_steam_tool(&appid, &name) {
        return None;
    }
    Some(SteamGame { appid, name })
}

pub fn scan_steam_games() -> Vec<SteamGame> {
    let roots = find_steam_roots();
    scan_steam_games_from_roots(&roots)
}

pub fn scan_steam_games_from_roots(roots: &[PathBuf]) -> Vec<SteamGame> {
    let mut by_appid: BTreeMap<String, SteamGame> = BTreeMap::new();
    let mut library_dirs = Vec::new();

    for root in roots {
        if !library_dirs.contains(root) {
            library_dirs.push(root.clone());
        }
        let vdf_path = root.join("steamapps/libraryfolders.vdf");
        if let Ok(content) = fs::read_to_string(&vdf_path) {
            for lib_path in parse_library_folders_vdf(&content) {
                if !library_dirs.contains(&lib_path) {
                    library_dirs.push(lib_path);
                }
            }
        }
    }

    for lib_dir in &library_dirs {
        let steamapps = lib_dir.join("steamapps");
        let Ok(entries) = fs::read_dir(&steamapps) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();
            if name_str.starts_with("appmanifest_") && name_str.ends_with(".acf") {
                if let Ok(content) = fs::read_to_string(entry.path()) {
                    if let Some(game) = parse_appmanifest_acf(&content) {
                        by_appid.entry(game.appid.clone()).or_insert(game);
                    }
                }
            }
        }
    }

    let mut games: Vec<SteamGame> = by_appid.into_values().collect();
    games.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.appid.cmp(&b.appid))
    });
    games
}

fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn build_ephemeral_wrapper_script(
    steam_root: &Path,
    appid: &str,
    launcher_script: &Path,
    michadame_pid: u32,
) -> String {
    let quoted_root = shell_single_quote(&steam_root.to_string_lossy());
    let quoted_target = shell_single_quote(&format!("AppId={appid}"));
    let quoted_launcher = shell_single_quote(&launcher_script.to_string_lossy());

    format!(
        "#!/bin/sh\n\
{{\n\
    SELF=\"$0\"\n\
    REAL=\"${{SELF}}.real\"\n\
    STEAM_ROOT={quoted_root}\n\
    TARGET_APPID={quoted_target}\n\
    MICHADAME_PID=\"{michadame_pid}\"\n\
    LAUNCHER_SCRIPT={quoted_launcher}\n\
\n\
    restore_all() {{\n\
        for dir in \"$STEAM_ROOT/steamrt64\" \"$STEAM_ROOT/ubuntu12_32\"; do\n\
            if [ -f \"$dir/steam-launch-wrapper.real\" ]; then\n\
                mv -f \"$dir/steam-launch-wrapper.real\" \"$dir/steam-launch-wrapper\" 2>/dev/null || true\n\
            fi\n\
        done\n\
    }}\n\
\n\
    if ! kill -0 \"$MICHADAME_PID\" 2>/dev/null; then\n\
        restore_all\n\
        if [ -x \"$REAL\" ]; then\n\
            exec \"$REAL\" \"$@\"\n\
        fi\n\
        exec \"$SELF\" \"$@\"\n\
    fi\n\
\n\
    is_target=0\n\
    is_install=0\n\
    for arg in \"$@\"; do\n\
        if [ \"$arg\" = \"$TARGET_APPID\" ]; then\n\
            is_target=1\n\
        elif [ \"$arg\" = \"Install=1\" ]; then\n\
            is_install=1\n\
        fi\n\
    done\n\
\n\
    if [ \"$is_install\" = \"1\" ]; then\n\
        if [ -x \"$REAL\" ]; then\n\
            exec \"$REAL\" \"$@\"\n\
        fi\n\
        restore_all\n\
        exec \"$SELF\" \"$@\"\n\
    fi\n\
\n\
    restore_all\n\
\n\
    if [ -n \"${{GAMESCOPE_WAYLAND_DISPLAY:-}}\" ] || [ \"${{PULSE_SINK:-}}\" = \"michadame_gamescope\" ]; then\n\
        exec \"$SELF\" \"$@\"\n\
    fi\n\
\n\
    if [ \"$is_target\" = \"1\" ] && [ -x \"$LAUNCHER_SCRIPT\" ]; then\n\
        exec \"$LAUNCHER_SCRIPT\" \"$SELF\" \"$@\"\n\
    fi\n\
\n\
    exec \"$SELF\" \"$@\"\n\
}}\n"
    )
}

pub fn restore_steam_launch_wrappers() {
    let roots = find_steam_roots();
    restore_steam_launch_wrappers_in_roots(&roots);
}

pub fn restore_steam_launch_wrappers_in_roots(roots: &[PathBuf]) {
    for root in roots {
        for sub in ["steamrt64", "ubuntu12_32"] {
            let dir = root.join(sub);
            let wrapper = dir.join("steam-launch-wrapper");
            let real = dir.join("steam-launch-wrapper.real");
            if real.is_file() {
                if let Err(e) = fs::rename(&real, &wrapper) {
                    tracing::warn!(
                        "Failed to restore {} -> {}: {e}",
                        real.display(),
                        wrapper.display()
                    );
                } else {
                    tracing::info!("Restored original {}", wrapper.display());
                }
            }
        }
    }
}

pub fn arm_steam_launch_wrapper_in_roots(
    roots: &[PathBuf],
    appid: &str,
    launcher_script: &Path,
    michadame_pid: u32,
) -> Result<Vec<PathBuf>> {
    restore_steam_launch_wrappers_in_roots(roots);

    let mut armed = Vec::new();
    for root in roots {
        let script_content =
            build_ephemeral_wrapper_script(root, appid, launcher_script, michadame_pid);
        for sub in ["steamrt64", "ubuntu12_32"] {
            let dir = root.join(sub);
            let wrapper = dir.join("steam-launch-wrapper");
            let real = dir.join("steam-launch-wrapper.real");
            if wrapper.is_file() {
                fs::rename(&wrapper, &real).with_context(|| {
                    format!(
                        "Failed to move {} to {}",
                        wrapper.display(),
                        real.display()
                    )
                })?;
                if let Err(e) = (|| -> Result<()> {
                    fs::write(&wrapper, script_content.as_bytes())?;
                    let mut perms = fs::metadata(&wrapper)?.permissions();
                    perms.set_mode(0o755);
                    fs::set_permissions(&wrapper, perms)?;
                    Ok(())
                })() {
                    let _ = fs::rename(&real, &wrapper);
                    return Err(e).with_context(|| {
                        format!(
                            "Failed to write ephemeral wrapper to {}",
                            wrapper.display()
                        )
                    });
                }
                armed.push(wrapper);
            }
        }
    }
    Ok(armed)
}

pub fn launch_steam_game(game: &SteamGame, launcher_script_str: &str) -> Result<()> {
    let trimmed = launcher_script_str.trim();
    if trimmed.is_empty() {
        anyhow::bail!(
            "No launcher script configured. Set the Launcher Script path in Capture & Replay settings."
        );
    }
    let launcher_path = PathBuf::from(trimmed);
    if !launcher_path.is_file() {
        anyhow::bail!(
            "Launcher script not found at '{}'. Check Capture & Replay settings.",
            launcher_path.display()
        );
    }

    let roots = find_steam_roots();
    if roots.is_empty() {
        anyhow::bail!("Could not locate local Steam installation directory.");
    }

    let pid = std::process::id();
    let armed = arm_steam_launch_wrapper_in_roots(&roots, &game.appid, &launcher_path, pid)?;
    if armed.is_empty() {
        anyhow::bail!("Could not find steam-launch-wrapper in Steam installation.");
    }

    let args = game.steam_launch_args();
    tracing::info!(
        "Launching Steam game '{}' (AppId={}) with args {:?} via wrapper {}",
        game.name,
        game.appid,
        args,
        launcher_path.display()
    );

    if let Err(e) = Command::new("steam")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        restore_steam_launch_wrappers_in_roots(&roots);
        anyhow::bail!("Failed to execute 'steam' command: {e}");
    }

    // Fallback cleanup thread in case Steam never invokes steam-launch-wrapper
    // (e.g. user cancels a pre-launch dialog in Steam).
    let roots_clone = roots.clone();
    let _ = std::thread::Builder::new()
        .name("steam-wrapper-restore".to_string())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(30));
            restore_steam_launch_wrappers_in_roots(&roots_clone);
        });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_library_folders_and_acf() {
        let vdf = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"/home/user/.local/share/Steam"
		"label"		""
	}
	"1"
	{
		"path"		"/mnt/games/SteamLibrary"
	}
}
"#;
        let folders = parse_library_folders_vdf(vdf);
        assert_eq!(
            folders,
            vec![
                PathBuf::from("/home/user/.local/share/Steam"),
                PathBuf::from("/mnt/games/SteamLibrary")
            ]
        );

        let acf_game = r#"
"AppState"
{
	"appid"		"3259780"
	"Universe"		"1"
	"name"		"FINAL FANTASY RESONANCE"
	"StateFlags"		"4"
}
"#;
        let parsed = parse_appmanifest_acf(acf_game).expect("should parse valid game");
        assert_eq!(parsed.appid, "3259780");
        assert_eq!(parsed.name, "FINAL FANTASY RESONANCE");
        assert_eq!(
            parsed.steam_launch_args(),
            vec!["-applaunch".to_string(), "3259780".to_string()]
        );

        let acf_proton = r#"
"AppState"
{
	"appid"		"1493710"
	"name"		"Proton Experimental"
}
"#;
        assert!(parse_appmanifest_acf(acf_proton).is_none());

        let acf_slr = r#"
"AppState"
{
	"appid"		"1628350"
	"name"		"Steam Linux Runtime 3.0 (sniper)"
}
"#;
        assert!(parse_appmanifest_acf(acf_slr).is_none());
    }

    #[test]
    fn test_ephemeral_wrapper_self_restores_and_wraps() {
        let temp_dir = std::env::temp_dir().join(format!(
            "michadame_steam_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let rt64 = temp_dir.join("steamrt64");
        let u32_dir = temp_dir.join("ubuntu12_32");
        fs::create_dir_all(&rt64).unwrap();
        fs::create_dir_all(&u32_dir).unwrap();

        let orig_content = "#!/bin/sh\necho \"ORIG_RAN:$*\"\n";
        for sub in [&rt64, &u32_dir] {
            let w = sub.join("steam-launch-wrapper");
            fs::write(&w, orig_content).unwrap();
            let mut p = fs::metadata(&w).unwrap().permissions();
            p.set_mode(0o755);
            fs::set_permissions(&w, p).unwrap();
        }

        let launcher = temp_dir.join("launcher.sh");
        fs::write(&launcher, "#!/bin/sh\necho \"LAUNCHER_WRAPPED\"\nexec \"$@\"\n").unwrap();
        let mut lp = fs::metadata(&launcher).unwrap().permissions();
        lp.set_mode(0o755);
        fs::set_permissions(&launcher, lp).unwrap();

        let roots = vec![temp_dir.clone()];
        let armed = arm_steam_launch_wrapper_in_roots(
            &roots,
            "3259780",
            &launcher,
            std::process::id(),
        )
        .unwrap();
        assert_eq!(armed.len(), 2);
        assert!(rt64.join("steam-launch-wrapper.real").is_file());
        assert!(u32_dir.join("steam-launch-wrapper.real").is_file());

        // 1. Install=1 step should not consume the hook or wrap
        let out_install = Command::new(rt64.join("steam-launch-wrapper"))
            .args(["--", "reaper", "SteamLaunch", "AppId=3259780", "Install=1", "--", "setup.exe"])
            .output()
            .unwrap();
        let stdout_install = String::from_utf8_lossy(&out_install.stdout);
        assert!(stdout_install.contains("ORIG_RAN:"));
        assert!(!stdout_install.contains("LAUNCHER_WRAPPED"));
        assert!(rt64.join("steam-launch-wrapper.real").is_file());

        // 2. Actual game launch should wrap with launcher AND immediately restore both binaries
        let out_game = Command::new(rt64.join("steam-launch-wrapper"))
            .args(["--", "reaper", "SteamLaunch", "AppId=3259780", "--", "game.exe", "-skipintro"])
            .output()
            .unwrap();
        let stdout_game = String::from_utf8_lossy(&out_game.stdout);
        assert!(stdout_game.contains("LAUNCHER_WRAPPED"));
        assert!(stdout_game.contains("ORIG_RAN:-- reaper SteamLaunch AppId=3259780 -- game.exe -skipintro"));
        assert!(!rt64.join("steam-launch-wrapper.real").exists());
        assert!(!u32_dir.join("steam-launch-wrapper.real").exists());
        assert_eq!(
            fs::read_to_string(rt64.join("steam-launch-wrapper")).unwrap(),
            orig_content
        );
        assert_eq!(
            fs::read_to_string(u32_dir.join("steam-launch-wrapper")).unwrap(),
            orig_content
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
