use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};

const WRAPPER_MARKER: &str = "# MICHADAME_WRAPPER_V1";
const WRAPPER_SESSION_PREFIX: &str = "# MICHADAME_SESSION=";
const WRAPPER_DIGEST_PREFIX: &str = "# MICHADAME_REAL_DIGEST=";
const WRAPPER_META_FILENAME: &str = ".steam-launch-wrapper.michadame";
const WRAPPER_TMP_FILENAME: &str = ".steam-launch-wrapper.michadame.tmp";

static NEXT_WRAPPER_SESSION_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
struct WrapperOwnershipMeta {
    session_id: String,
    real_digest: String,
}

pub fn next_wrapper_session_id(pid: u32) -> String {
    let seq = NEXT_WRAPPER_SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{pid}-{nanos}-{seq}")
}

fn compute_content_digest(bytes: &[u8]) -> String {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001b3;
    let mut hash = FNV_OFFSET_BASIS;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{}:{hash:016x}", bytes.len())
}

fn compute_file_digest(path: &Path) -> Result<String> {
    let bytes =
        fs::read(path).with_context(|| format!("Failed to read {} for digest", path.display()))?;
    Ok(compute_content_digest(&bytes))
}

fn has_michadame_wrapper_marker(content: &str) -> bool {
    content.lines().any(|line| line.trim() == WRAPPER_MARKER)
}

fn parse_wrapper_metadata(content: &str) -> Option<WrapperOwnershipMeta> {
    let mut has_marker = false;
    let mut session_id = None;
    let mut real_digest = None;
    for line in content.lines().take(16) {
        let trimmed = line.trim();
        if trimmed == WRAPPER_MARKER {
            has_marker = true;
        } else if let Some(rest) = trimmed.strip_prefix(WRAPPER_SESSION_PREFIX) {
            if !rest.is_empty() {
                session_id = Some(rest.to_string());
            }
        } else if let Some(rest) = trimmed.strip_prefix(WRAPPER_DIGEST_PREFIX) {
            if !rest.is_empty() {
                real_digest = Some(rest.to_string());
            }
        }
    }
    if !has_marker {
        return None;
    }
    Some(WrapperOwnershipMeta {
        session_id: session_id?,
        real_digest: real_digest?,
    })
}

fn format_wrapper_metadata(meta: &WrapperOwnershipMeta) -> String {
    format!(
        "{WRAPPER_MARKER}\n{WRAPPER_SESSION_PREFIX}{}\n{WRAPPER_DIGEST_PREFIX}{}\n",
        meta.session_id, meta.real_digest
    )
}

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
    let session_id = next_wrapper_session_id(michadame_pid);
    build_ephemeral_wrapper_script_with_session(
        steam_root,
        appid,
        launcher_script,
        michadame_pid,
        &session_id,
        "0:0000000000000000",
    )
}

pub fn build_ephemeral_wrapper_script_with_session(
    steam_root: &Path,
    appid: &str,
    launcher_script: &Path,
    michadame_pid: u32,
    session_id: &str,
    real_digest: &str,
) -> String {
    build_wrapper(
        steam_root,
        appid,
        launcher_script,
        michadame_pid,
        session_id,
        real_digest,
        false,
    )
}

fn build_wrapper(
    steam_root: &Path,
    appid: &str,
    launcher_script: &Path,
    michadame_pid: u32,
    session_id: &str,
    real_digest: &str,
    deck: bool,
) -> String {
    if deck {
        return include_str!("../../scripts/michadame-steam-deck-wrapper.sh")
            .replace("@SESSION_HEADER@", &session_id.replace(['\n', '\r'], ""))
            .replace("@DIGEST_HEADER@", &real_digest.replace(['\n', '\r'], ""))
            .replace(
                "@STEAM_ROOT@",
                &shell_single_quote(&steam_root.to_string_lossy()),
            )
            .replace("@APPID@", &shell_single_quote(appid))
            .replace("@PID@", &michadame_pid.to_string())
            .replace("@SESSION@", &shell_single_quote(session_id))
            .replace(
                "@LAUNCHER@",
                &shell_single_quote(&launcher_script.to_string_lossy()),
            )
            .replace(
                "@FLOCK@",
                &shell_single_quote(&crate::portable::helper_path("flock").to_string_lossy()),
            );
    }
    let nesting_guard = if deck {
        "[ \"${PULSE_SINK:-}\" = \"michadame_gamescope\" ]"
    } else {
        "[ -n \"${GAMESCOPE_WAYLAND_DISPLAY:-}\" ] || [ \"${PULSE_SINK:-}\" = \"michadame_gamescope\" ]"
    };
    let clean_session: String = session_id
        .chars()
        .filter(|&c| c != '\n' && c != '\r')
        .collect();
    let clean_digest: String = real_digest
        .chars()
        .filter(|&c| c != '\n' && c != '\r')
        .collect();
    let quoted_root = shell_single_quote(&steam_root.to_string_lossy());
    let quoted_target = shell_single_quote(&format!("AppId={appid}"));
    let quoted_launcher = shell_single_quote(&launcher_script.to_string_lossy());
    let quoted_session = shell_single_quote(&clean_session);

    format!(
        "#!/bin/sh\n\
{WRAPPER_MARKER}\n\
{WRAPPER_SESSION_PREFIX}{clean_session}\n\
{WRAPPER_DIGEST_PREFIX}{clean_digest}\n\
{{\n\
    SELF=\"$0\"\n\
    REAL=\"${{SELF}}.real\"\n\
    STEAM_ROOT={quoted_root}\n\
    TARGET_APPID={quoted_target}\n\
    MICHADAME_PID=\"{michadame_pid}\"\n\
    MICHADAME_SESSION={quoted_session}\n\
    LAUNCHER_SCRIPT={quoted_launcher}\n\
\n\
    restore_all() {{\n\
        for dir in \"$STEAM_ROOT/steamrt64\" \"$STEAM_ROOT/ubuntu12_32\"; do\n\
            if [ -f \"$dir/steam-launch-wrapper.real\" ] && [ -f \"$dir/steam-launch-wrapper\" ]; then\n\
                if grep -Fxq '{WRAPPER_MARKER}' \"$dir/steam-launch-wrapper\" 2>/dev/null && \\\n\
                   grep -Fxq \"{WRAPPER_SESSION_PREFIX}$MICHADAME_SESSION\" \"$dir/steam-launch-wrapper\" 2>/dev/null; then\n\
                    mv -f \"$dir/steam-launch-wrapper.real\" \"$dir/steam-launch-wrapper\" 2>/dev/null || true\n\
                    rm -f \"$dir/{WRAPPER_META_FILENAME}\" 2>/dev/null || true\n\
                fi\n\
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
    if {nesting_guard}; then\n\
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
    restore_steam_launch_wrappers_for_session(roots, None);
}

pub fn restore_steam_launch_wrappers_for_session(
    roots: &[PathBuf],
    expected_session: Option<&str>,
) {
    for root in roots {
        for sub in ["steamrt64", "ubuntu12_32"] {
            let dir = root.join(sub);
            let wrapper = dir.join("steam-launch-wrapper");
            let real = dir.join("steam-launch-wrapper.real");
            let meta_path = dir.join(WRAPPER_META_FILENAME);
            let tmp_path = dir.join(WRAPPER_TMP_FILENAME);

            if expected_session.is_none() && tmp_path.is_file() {
                let _ = fs::remove_file(&tmp_path);
            }

            if !real.is_file() {
                if meta_path.is_file() {
                    let should_remove = match expected_session {
                        None => true,
                        Some(req_session) => fs::read(&meta_path)
                            .ok()
                            .and_then(|b| parse_wrapper_metadata(&String::from_utf8_lossy(&b)))
                            .is_some_and(|m| m.session_id == req_session),
                    };
                    if should_remove {
                        let _ = fs::remove_file(&meta_path);
                    }
                }
                continue;
            }

            let meta_file_meta = fs::read(&meta_path)
                .ok()
                .and_then(|b| parse_wrapper_metadata(&String::from_utf8_lossy(&b)));
            let wrapper_bytes = fs::read(&wrapper).ok();
            let wrapper_meta = wrapper_bytes
                .as_deref()
                .and_then(|b| parse_wrapper_metadata(&String::from_utf8_lossy(b)));
            let wrapper_has_marker = wrapper_bytes
                .as_deref()
                .is_some_and(|b| has_michadame_wrapper_marker(&String::from_utf8_lossy(b)));

            if let Some(w_meta) = wrapper_meta {
                if let Some(req_session) = expected_session {
                    if w_meta.session_id != req_session {
                        continue;
                    }
                }
                if let Some(ref m_meta) = meta_file_meta {
                    if m_meta.session_id != w_meta.session_id
                        || m_meta.real_digest != w_meta.real_digest
                    {
                        tracing::warn!(
                            "Refusing to restore {}: wrapper header and companion metadata disagree",
                            wrapper.display()
                        );
                        continue;
                    }
                }
                let Ok(actual_real_digest) = compute_file_digest(&real) else {
                    tracing::warn!(
                        "Refusing to restore {}: failed to digest {}",
                        wrapper.display(),
                        real.display()
                    );
                    continue;
                };
                if actual_real_digest != w_meta.real_digest {
                    tracing::warn!(
                        "Refusing to restore {}: {} digest mismatch (expected {}, got {})",
                        wrapper.display(),
                        real.display(),
                        w_meta.real_digest,
                        actual_real_digest
                    );
                    continue;
                }

                if let Err(e) = fs::rename(&real, &wrapper) {
                    tracing::warn!(
                        "Failed to restore {} -> {}: {e}",
                        real.display(),
                        wrapper.display()
                    );
                } else {
                    let _ = fs::remove_file(&meta_path);
                    tracing::info!("Restored original {}", wrapper.display());
                }
            } else if !wrapper_has_marker {
                let Some(m_meta) = meta_file_meta else {
                    // No Michadame marker in wrapper and no Michadame companion metadata:
                    // `.real` is an unowned file created outside Michadame. Leave untouched.
                    continue;
                };
                if let Some(req_session) = expected_session {
                    if m_meta.session_id != req_session {
                        continue;
                    }
                }
                let Ok(actual_real_digest) = compute_file_digest(&real) else {
                    continue;
                };
                if actual_real_digest != m_meta.real_digest {
                    continue;
                }

                if wrapper_bytes.is_some() {
                    // Steam (or another external update) replaced `steam-launch-wrapper`
                    // with a new non-wrapper file while Michadame was armed. Preserve the
                    // updated file and remove our obsolete `.real` backup and metadata.
                    let _ = fs::remove_file(&real);
                    let _ = fs::remove_file(&meta_path);
                    tracing::info!(
                        "Detected external update to {}; preserved updated file and removed obsolete backup {}",
                        wrapper.display(),
                        real.display()
                    );
                } else {
                    // `steam-launch-wrapper` is missing after an interrupted transition;
                    // restore the verified `.real` backup.
                    if fs::rename(&real, &wrapper).is_ok() {
                        let _ = fs::remove_file(&meta_path);
                        tracing::info!(
                            "Recovered missing {} from verified backup {}",
                            wrapper.display(),
                            real.display()
                        );
                    }
                }
            }
        }
    }
}

fn rollback_armed_dirs(armed_dirs: &[PathBuf]) {
    for dir in armed_dirs.iter().rev() {
        let wrapper = dir.join("steam-launch-wrapper");
        let real = dir.join("steam-launch-wrapper.real");
        let meta_path = dir.join(WRAPPER_META_FILENAME);
        let tmp_path = dir.join(WRAPPER_TMP_FILENAME);
        let _ = fs::remove_file(&tmp_path);
        if real.is_file() {
            let _ = fs::rename(&real, &wrapper);
        }
        let _ = fs::remove_file(&meta_path);
    }
}

pub fn arm_steam_launch_wrapper_in_roots(
    roots: &[PathBuf],
    appid: &str,
    launcher_script: &Path,
    michadame_pid: u32,
) -> Result<Vec<PathBuf>> {
    let session_id = next_wrapper_session_id(michadame_pid);
    arm_steam_launch_wrapper_in_roots_with_session(
        roots,
        appid,
        launcher_script,
        michadame_pid,
        &session_id,
    )
}

pub fn arm_steam_launch_wrapper_in_roots_with_session(
    roots: &[PathBuf],
    appid: &str,
    launcher_script: &Path,
    michadame_pid: u32,
    session_id: &str,
) -> Result<Vec<PathBuf>> {
    arm_wrapper(
        roots,
        appid,
        launcher_script,
        michadame_pid,
        session_id,
        false,
    )
}

fn arm_wrapper(
    roots: &[PathBuf],
    appid: &str,
    launcher_script: &Path,
    michadame_pid: u32,
    session_id: &str,
    deck: bool,
) -> Result<Vec<PathBuf>> {
    if session_id.trim().is_empty() {
        anyhow::bail!("Wrapper session ID must not be empty");
    }

    restore_steam_launch_wrappers_in_roots(roots);

    let mut armed = Vec::new();
    let mut armed_dirs = Vec::new();

    for root in roots {
        for sub in ["steamrt64", "ubuntu12_32"] {
            let dir = root.join(sub);
            let wrapper = dir.join("steam-launch-wrapper");
            let real = dir.join("steam-launch-wrapper.real");
            let meta_path = dir.join(WRAPPER_META_FILENAME);
            let tmp_path = dir.join(WRAPPER_TMP_FILENAME);

            match fs::symlink_metadata(&real) {
                Ok(_) => {
                    rollback_armed_dirs(&armed_dirs);
                    anyhow::bail!(
                        "Refusing to arm {}: unowned or unrestorable backup {} already exists",
                        wrapper.display(),
                        real.display()
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    rollback_armed_dirs(&armed_dirs);
                    return Err(e).with_context(|| {
                        format!("Failed to inspect backup path {}", real.display())
                    });
                }
            }

            let wrapper_is_file = match fs::metadata(&wrapper) {
                Ok(m) => m.is_file(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(e) => {
                    rollback_armed_dirs(&armed_dirs);
                    return Err(e).with_context(|| {
                        format!("Failed to inspect wrapper path {}", wrapper.display())
                    });
                }
            };

            if !wrapper_is_file {
                continue;
            }

            let step_result = (|| -> Result<()> {
                let orig_bytes = fs::read(&wrapper)
                    .with_context(|| format!("Failed to read {}", wrapper.display()))?;
                if has_michadame_wrapper_marker(&String::from_utf8_lossy(&orig_bytes)) {
                    anyhow::bail!(
                        "Refusing to arm {}: file is already a Michadame wrapper without a valid backup",
                        wrapper.display()
                    );
                }

                let real_digest = compute_content_digest(&orig_bytes);
                let meta = WrapperOwnershipMeta {
                    session_id: session_id.to_string(),
                    real_digest: real_digest.clone(),
                };
                let script_content = build_wrapper(
                    root,
                    appid,
                    launcher_script,
                    michadame_pid,
                    session_id,
                    &real_digest,
                    deck,
                );

                fs::write(&tmp_path, script_content.as_bytes()).with_context(|| {
                    format!(
                        "Failed to write temporary wrapper to {}",
                        tmp_path.display()
                    )
                })?;
                let mut perms = fs::metadata(&tmp_path)?.permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&tmp_path, perms)?;

                fs::write(&meta_path, format_wrapper_metadata(&meta).as_bytes()).with_context(
                    || {
                        format!(
                            "Failed to write wrapper metadata to {}",
                            meta_path.display()
                        )
                    },
                )?;

                fs::rename(&wrapper, &real).with_context(|| {
                    format!("Failed to move {} to {}", wrapper.display(), real.display())
                })?;

                if let Err(e) = fs::rename(&tmp_path, &wrapper) {
                    let _ = fs::rename(&real, &wrapper);
                    let _ = fs::remove_file(&meta_path);
                    let _ = fs::remove_file(&tmp_path);
                    return Err(e).with_context(|| {
                        format!(
                            "Failed to publish ephemeral wrapper to {}",
                            wrapper.display()
                        )
                    });
                }

                Ok(())
            })();

            if let Err(e) = step_result {
                let _ = fs::remove_file(&tmp_path);
                let _ = fs::remove_file(&meta_path);
                rollback_armed_dirs(&armed_dirs);
                return Err(e);
            }

            armed_dirs.push(dir);
            armed.push(wrapper);
        }
    }
    Ok(armed)
}

pub fn launch_steam_game(game: &SteamGame, launcher_script_str: &str) -> Result<()> {
    launch_game(game, launcher_script_str, false)
}

pub fn launch_steam_deck_game(game: &SteamGame, launcher_script: &str) -> Result<()> {
    launch_game(game, launcher_script, true)
}

fn launch_game(game: &SteamGame, launcher_script_str: &str, deck: bool) -> Result<()> {
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
    let session_id = next_wrapper_session_id(pid);
    let armed = arm_wrapper(&roots, &game.appid, &launcher_path, pid, &session_id, deck)?;
    if armed.is_empty() {
        anyhow::bail!("Could not find steam-launch-wrapper in Steam installation.");
    }

    let args = game.steam_launch_args();
    tracing::info!(
        "Launching Steam game '{}' (AppId={}) with args {:?} via wrapper {} (session={})",
        game.name,
        game.appid,
        args,
        launcher_path.display(),
        session_id
    );

    if let Err(e) = crate::portable::host_command("steam")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        restore_steam_launch_wrappers_for_session(&roots, Some(&session_id));
        anyhow::bail!("Failed to execute 'steam' command: {e}");
    }

    // Fallback cleanup thread in case Steam never invokes steam-launch-wrapper
    // (e.g. user cancels a pre-launch dialog in Steam).
    let roots_clone = roots.clone();
    let _ = std::thread::Builder::new()
        .name("steam-wrapper-restore".to_string())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(if deck { 300 } else { 30 }));
            restore_steam_launch_wrappers_for_session(&roots_clone, Some(&session_id));
        });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deck_hook_survives_unrelated_launches_and_different_pid_namespaces() {
        let root = tempfile::tempdir().unwrap();
        for sub in ["steamrt64", "ubuntu12_32"] {
            let dir = root.path().join(sub);
            fs::create_dir(&dir).unwrap();
            let wrapper = dir.join("steam-launch-wrapper");
            fs::write(&wrapper, "#!/bin/sh\nprintf ORIGINAL_GAME\n").unwrap();
            fs::set_permissions(wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let launcher = root.path().join("launcher.sh");
        fs::write(&launcher, "#!/bin/sh\nprintf NESTED_GAME\n").unwrap();
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
        let lease =
            crate::steam_deck::launch::hold_lease(&root.path().join("viewer.lock")).unwrap();
        // This PID cannot identify the viewer; the shared lease must be used.
        arm_wrapper(
            &[root.path().to_path_buf()],
            "1234",
            &launcher,
            u32::MAX,
            "namespace-test",
            true,
        )
        .unwrap();
        let wrapper = root.path().join("steamrt64/steam-launch-wrapper");
        let unrelated = Command::new(&wrapper)
            .arg("AppId=9999")
            .env("SteamAppId", "1234")
            .output()
            .unwrap();
        assert!(unrelated.status.success());
        assert_eq!(unrelated.stdout, b"ORIGINAL_GAME");
        assert!(fs::read_to_string(&wrapper)
            .unwrap()
            .contains(WRAPPER_MARKER));
        let install = Command::new(&wrapper)
            .args(["AppId=1234", "Install=1"])
            .output()
            .unwrap();
        assert_eq!(install.stdout, b"ORIGINAL_GAME");
        assert!(fs::read_to_string(&wrapper)
            .unwrap()
            .contains(WRAPPER_MARKER));
        // Some Steam paths supply AppID through the environment only. An outer
        // Gamescope/null sink is expected in Game Mode and must not skip nesting.
        let target = Command::new(&wrapper)
            .arg("--")
            .env("SteamAppId", "1234")
            .env("PULSE_SINK", "michadame_gamescope")
            .env("GAMESCOPE_WAYLAND_DISPLAY", "gamescope-outer")
            .output()
            .unwrap();
        assert!(
            target.status.success(),
            "{}",
            String::from_utf8_lossy(&target.stderr)
        );
        assert_eq!(target.stdout, b"NESTED_GAME");
        for sub in ["steamrt64", "ubuntu12_32"] {
            assert!(!root
                .path()
                .join(sub)
                .join("steam-launch-wrapper.real")
                .exists());
        }
        drop(lease);
    }

    #[test]
    fn deck_wrapper_nests_under_steamos_while_desktop_wrapper_keeps_its_guard() {
        for deck in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let runtime = root.path().join("steamrt64");
            fs::create_dir(&runtime).unwrap();
            let launcher = root.path().join("launcher.sh");
            fs::write(&launcher, "#!/bin/sh\nprintf 'NESTED_GAME'\n").unwrap();
            fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
            let wrapper = runtime.join("steam-launch-wrapper");
            let real = runtime.join("steam-launch-wrapper.real");
            fs::write(&real, "#!/bin/sh\nprintf 'ORIGINAL_GAME'\n").unwrap();
            fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
            fs::write(
                &wrapper,
                build_wrapper(
                    root.path(),
                    "1234",
                    &launcher,
                    std::process::id(),
                    "deck-test",
                    "unused",
                    deck,
                ),
            )
            .unwrap();
            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
            let output = Command::new(&wrapper)
                .arg("AppId=1234")
                .env("GAMESCOPE_WAYLAND_DISPLAY", "gamescope-outer")
                .env_remove("PULSE_SINK")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                if deck { "NESTED_GAME" } else { "ORIGINAL_GAME" }
            );
            assert!(!real.exists());
        }
    }

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
        fs::write(
            &launcher,
            "#!/bin/sh\necho \"LAUNCHER_WRAPPED\"\nexec \"$@\"\n",
        )
        .unwrap();
        let mut lp = fs::metadata(&launcher).unwrap().permissions();
        lp.set_mode(0o755);
        fs::set_permissions(&launcher, lp).unwrap();

        let roots = vec![temp_dir.clone()];
        let armed =
            arm_steam_launch_wrapper_in_roots(&roots, "3259780", &launcher, std::process::id())
                .unwrap();
        assert_eq!(armed.len(), 2);
        assert!(rt64.join("steam-launch-wrapper.real").is_file());
        assert!(u32_dir.join("steam-launch-wrapper.real").is_file());

        // 1. Install=1 step should not consume the hook or wrap
        let out_install = Command::new(rt64.join("steam-launch-wrapper"))
            .args([
                "--",
                "reaper",
                "SteamLaunch",
                "AppId=3259780",
                "Install=1",
                "--",
                "setup.exe",
            ])
            .output()
            .unwrap();
        let stdout_install = String::from_utf8_lossy(&out_install.stdout);
        assert!(stdout_install.contains("ORIG_RAN:"));
        assert!(!stdout_install.contains("LAUNCHER_WRAPPED"));
        assert!(rt64.join("steam-launch-wrapper.real").is_file());

        // 2. Actual game launch should wrap with launcher AND immediately restore both binaries
        let out_game = Command::new(rt64.join("steam-launch-wrapper"))
            .args([
                "--",
                "reaper",
                "SteamLaunch",
                "AppId=3259780",
                "--",
                "game.exe",
                "-skipintro",
            ])
            .output()
            .unwrap();
        let stdout_game = String::from_utf8_lossy(&out_game.stdout);
        assert!(stdout_game.contains("LAUNCHER_WRAPPED"));
        assert!(stdout_game
            .contains("ORIG_RAN:-- reaper SteamLaunch AppId=3259780 -- game.exe -skipintro"));
        assert!(!rt64.join("steam-launch-wrapper.real").exists());
        assert!(!u32_dir.join("steam-launch-wrapper.real").exists());
        assert!(!rt64.join(WRAPPER_META_FILENAME).exists());
        assert!(!u32_dir.join(WRAPPER_META_FILENAME).exists());
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

    fn make_temp_steam_test_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "michadame_steam_{tag}_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            next_wrapper_session_id(std::process::id())
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_multi_location_rollback_on_failure() {
        let temp_dir = make_temp_steam_test_dir("rollback");
        let rt64 = temp_dir.join("steamrt64");
        let u32_dir = temp_dir.join("ubuntu12_32");
        fs::create_dir_all(&rt64).unwrap();
        fs::create_dir_all(&u32_dir).unwrap();

        let orig_rt64 = "#!/bin/sh\necho \"ORIG_RT64\"\n";
        let orig_u32 = "#!/bin/sh\necho \"ORIG_U32\"\n";
        fs::write(rt64.join("steam-launch-wrapper"), orig_rt64).unwrap();
        fs::write(u32_dir.join("steam-launch-wrapper"), orig_u32).unwrap();
        for sub in [&rt64, &u32_dir] {
            let w = sub.join("steam-launch-wrapper");
            let mut p = fs::metadata(&w).unwrap().permissions();
            p.set_mode(0o755);
            fs::set_permissions(&w, p).unwrap();
        }

        let launcher = temp_dir.join("launcher.sh");
        fs::write(&launcher, "#!/bin/sh\nexec \"$@\"\n").unwrap();
        let mut lp = fs::metadata(&launcher).unwrap().permissions();
        lp.set_mode(0o755);
        fs::set_permissions(&launcher, lp).unwrap();

        let roots = vec![temp_dir.clone()];

        // Case 1: Second directory has an unowned `.real` backup.
        let unowned_real = u32_dir.join("steam-launch-wrapper.real");
        fs::write(&unowned_real, "USER_CUSTOM_BACKUP").unwrap();

        let res =
            arm_steam_launch_wrapper_in_roots(&roots, "3259780", &launcher, std::process::id());
        assert!(res.is_err(), "arming should fail when unowned .real exists");

        // First directory (`steamrt64`) must be rolled back completely.
        assert_eq!(
            fs::read_to_string(rt64.join("steam-launch-wrapper")).unwrap(),
            orig_rt64
        );
        assert!(!rt64.join("steam-launch-wrapper.real").exists());
        assert!(!rt64.join(WRAPPER_META_FILENAME).exists());
        // Second directory's files must be untouched.
        assert_eq!(
            fs::read_to_string(u32_dir.join("steam-launch-wrapper")).unwrap(),
            orig_u32
        );
        assert_eq!(
            fs::read_to_string(&unowned_real).unwrap(),
            "USER_CUSTOM_BACKUP"
        );

        fs::remove_file(&unowned_real).unwrap();

        // Case 2: Second directory is read-only so writing the wrapper fails.
        let mut ro_perms = fs::metadata(&u32_dir).unwrap().permissions();
        ro_perms.set_mode(0o555);
        fs::set_permissions(&u32_dir, ro_perms).unwrap();

        let res_ro =
            arm_steam_launch_wrapper_in_roots(&roots, "3259780", &launcher, std::process::id());

        let mut rw_perms = fs::metadata(&u32_dir).unwrap().permissions();
        rw_perms.set_mode(0o755);
        fs::set_permissions(&u32_dir, rw_perms).unwrap();

        assert!(
            res_ro.is_err(),
            "arming should fail when second directory is read-only"
        );
        assert_eq!(
            fs::read_to_string(rt64.join("steam-launch-wrapper")).unwrap(),
            orig_rt64
        );
        assert!(!rt64.join("steam-launch-wrapper.real").exists());
        assert!(!rt64.join(WRAPPER_META_FILENAME).exists());
        assert_eq!(
            fs::read_to_string(u32_dir.join("steam-launch-wrapper")).unwrap(),
            orig_u32
        );
        assert!(!u32_dir.join("steam-launch-wrapper.real").exists());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_session_isolation_overlapping_launches() {
        let temp_dir = make_temp_steam_test_dir("session_isolation");
        let rt64 = temp_dir.join("steamrt64");
        let u32_dir = temp_dir.join("ubuntu12_32");
        fs::create_dir_all(&rt64).unwrap();
        fs::create_dir_all(&u32_dir).unwrap();

        let orig_content = "#!/bin/sh\necho \"ORIG_BINARY\"\n";
        for sub in [&rt64, &u32_dir] {
            let w = sub.join("steam-launch-wrapper");
            fs::write(&w, orig_content).unwrap();
            let mut p = fs::metadata(&w).unwrap().permissions();
            p.set_mode(0o755);
            fs::set_permissions(&w, p).unwrap();
        }

        let launcher = temp_dir.join("launcher.sh");
        fs::write(&launcher, "#!/bin/sh\nexec \"$@\"\n").unwrap();
        let mut lp = fs::metadata(&launcher).unwrap().permissions();
        lp.set_mode(0o755);
        fs::set_permissions(&launcher, lp).unwrap();

        let roots = vec![temp_dir.clone()];
        let pid = std::process::id();
        let session_a = next_wrapper_session_id(pid);
        let session_b = next_wrapper_session_id(pid);
        assert_ne!(session_a, session_b);

        // Launch A arms wrappers with session_a
        arm_steam_launch_wrapper_in_roots_with_session(&roots, "1001", &launcher, pid, &session_a)
            .unwrap();
        let wrapper_a = fs::read_to_string(rt64.join("steam-launch-wrapper")).unwrap();
        assert!(wrapper_a.contains(&format!("{WRAPPER_SESSION_PREFIX}{session_a}")));
        assert!(wrapper_a.contains("AppId=1001"));

        // Launch B arms wrappers with session_b before Launch A's 30s cleanup fires
        arm_steam_launch_wrapper_in_roots_with_session(&roots, "2002", &launcher, pid, &session_b)
            .unwrap();
        let wrapper_b = fs::read_to_string(rt64.join("steam-launch-wrapper")).unwrap();
        assert!(wrapper_b.contains(&format!("{WRAPPER_SESSION_PREFIX}{session_b}")));
        assert!(wrapper_b.contains("AppId=2002"));
        assert_eq!(
            fs::read_to_string(rt64.join("steam-launch-wrapper.real")).unwrap(),
            orig_content
        );

        // Delayed cleanup from Launch A runs: must NOT clobber Launch B's armed wrapper!
        restore_steam_launch_wrappers_for_session(&roots, Some(&session_a));
        assert!(rt64.join("steam-launch-wrapper.real").is_file());
        assert!(u32_dir.join("steam-launch-wrapper.real").is_file());
        assert!(rt64.join(WRAPPER_META_FILENAME).is_file());
        let after_a_cleanup = fs::read_to_string(rt64.join("steam-launch-wrapper")).unwrap();
        assert_eq!(after_a_cleanup, wrapper_b);

        // Cleanup for Launch B runs: must restore original wrapper and remove .real + metadata
        restore_steam_launch_wrappers_for_session(&roots, Some(&session_b));
        assert!(!rt64.join("steam-launch-wrapper.real").exists());
        assert!(!u32_dir.join("steam-launch-wrapper.real").exists());
        assert!(!rt64.join(WRAPPER_META_FILENAME).exists());
        assert!(!u32_dir.join(WRAPPER_META_FILENAME).exists());
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

    #[test]
    fn test_unowned_real_is_not_restored_over_steam_wrapper() {
        let temp_dir = make_temp_steam_test_dir("unowned_real");
        let rt64 = temp_dir.join("steamrt64");
        fs::create_dir_all(&rt64).unwrap();

        let wrapper_path = rt64.join("steam-launch-wrapper");
        let real_path = rt64.join("steam-launch-wrapper.real");
        let steam_binary = "#!/bin/sh\necho \"OFFICIAL_STEAM_WRAPPER\"\n";
        let unrelated_real = "#!/bin/sh\necho \"UNOWNED_REAL_FILE\"\n";

        fs::write(&wrapper_path, steam_binary).unwrap();
        fs::write(&real_path, unrelated_real).unwrap();

        let roots = vec![temp_dir.clone()];
        restore_steam_launch_wrappers_in_roots(&roots);

        assert_eq!(fs::read_to_string(&wrapper_path).unwrap(), steam_binary);
        assert_eq!(fs::read_to_string(&real_path).unwrap(), unrelated_real);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_steam_update_during_armed_interval_preserves_new_binary() {
        let temp_dir = make_temp_steam_test_dir("steam_update");
        let rt64 = temp_dir.join("steamrt64");
        fs::create_dir_all(&rt64).unwrap();

        let wrapper_path = rt64.join("steam-launch-wrapper");
        let real_path = rt64.join("steam-launch-wrapper.real");
        let meta_path = rt64.join(WRAPPER_META_FILENAME);

        let old_steam_binary = "#!/bin/sh\necho \"STEAM_WRAPPER_V1\"\n";
        fs::write(&wrapper_path, old_steam_binary).unwrap();
        let mut p = fs::metadata(&wrapper_path).unwrap().permissions();
        p.set_mode(0o755);
        fs::set_permissions(&wrapper_path, p).unwrap();

        let launcher = temp_dir.join("launcher.sh");
        fs::write(&launcher, "#!/bin/sh\nexec \"$@\"\n").unwrap();
        let mut lp = fs::metadata(&launcher).unwrap().permissions();
        lp.set_mode(0o755);
        fs::set_permissions(&launcher, lp).unwrap();

        let roots = vec![temp_dir.clone()];
        let pid = std::process::id();
        let session_id = next_wrapper_session_id(pid);
        arm_steam_launch_wrapper_in_roots_with_session(
            &roots,
            "3259780",
            &launcher,
            pid,
            &session_id,
        )
        .unwrap();
        assert!(real_path.is_file());
        assert!(meta_path.is_file());

        // Simulate Steam updating `steam-launch-wrapper` while the wrapper was armed
        let updated_steam_binary = "#!/bin/sh\necho \"STEAM_WRAPPER_V2_UPDATED\"\n";
        fs::write(&wrapper_path, updated_steam_binary).unwrap();

        // Restoration must NOT overwrite the updated Steam binary with the old `.real` backup
        restore_steam_launch_wrappers_for_session(&roots, Some(&session_id));

        assert_eq!(
            fs::read_to_string(&wrapper_path).unwrap(),
            updated_steam_binary
        );
        assert!(!real_path.exists());
        assert!(!meta_path.exists());

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
