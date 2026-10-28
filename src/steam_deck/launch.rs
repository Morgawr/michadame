use super::*;
use anyhow::{Context, Result};
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Retain private runtime metadata while the external game is running, so a
/// restarted viewer can reconnect instead of asking Steam to launch it twice.
pub fn reusable_session(
    runtime: &Path,
    appid: &str,
    alive: impl Fn(u32, &Path) -> bool,
) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(runtime).ok()?.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("michadame-deck-")
            || !entry.file_type().ok().is_some_and(|t| t.is_dir())
        {
            continue;
        }
        let path = entry.path();
        let Some(pid) = fs::read_to_string(path.join("gamescope.pid"))
            .ok()
            .and_then(|p| p.trim().parse().ok())
        else {
            continue;
        };
        if !alive(pid, &path) || !path.join("endpoints").is_file() {
            continue;
        }
        let matches = match fs::read_to_string(path.join("appid")) {
            Ok(id) => id.trim() == appid,
            // Compatibility with sessions created before explicit AppID metadata.
            Err(_) => fs::read_to_string(path.join("launch.log"))
                .ok()
                .is_some_and(|log| {
                    log.lines().any(|line| {
                        line.trim()
                            .strip_prefix("steam app id:")
                            .is_some_and(|id| id.trim() == appid)
                    })
                }),
        };
        if matches {
            candidates.push((
                entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH),
                path,
            ));
        }
    }
    candidates.sort_by_key(|(time, _)| *time);
    candidates.pop().map(|(_, path)| path)
}

fn live_compositor(pid: u32, session: &Path) -> bool {
    if session.join("compositor.lock").is_file() {
        return file_is_locked(&session.join("compositor.lock"));
    }
    fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|name| name.trim() == "gamescope")
        && fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|cmd| {
            String::from_utf8_lossy(&cmd).contains(session.to_string_lossy().as_ref())
        })
}

pub(crate) fn hold_lease(path: &Path) -> Result<fs::File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(std::io::Error::last_os_error())
            .context("Another viewer already owns this launch session");
    }
    Ok(file)
}

pub(crate) fn file_is_locked(path: &Path) -> bool {
    let Ok(file) = fs::OpenOptions::new().read(true).write(true).open(path) else {
        return false;
    };
    (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }) != 0
        && std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock
}

extern "C" {
    fn michadame_deck_sync_focus(
        viewer_display: *const std::ffi::c_char,
        control_display: *const std::ffi::c_char,
        pid: u32,
        shortcut: u32,
        game: u32,
        restore: i32,
        handoff: i32,
        menu_override: *mut u32,
    ) -> i32;
    fn michadame_deck_tag_window(
        display: *const std::ffi::c_char,
        pid: u32,
        appid: u32,
        previous: *mut u32,
    ) -> i32;
}

pub fn tag_viewer(appid: u32) -> Option<u32> {
    let display = std::ffi::CString::new(std::env::var("DISPLAY").ok()?).ok()?;
    let mut previous = 0;
    (unsafe {
        michadame_deck_tag_window(display.as_ptr(), std::process::id(), appid, &mut previous)
    } != 0)
        .then_some(previous)
}

pub fn restore_viewer(state: &mut AppState) {
    if let Some(deck) = &mut state.steam_deck {
        sync_viewer(deck, true, false);
    }
    if let Some(appid) = state
        .steam_deck
        .as_mut()
        .and_then(|d| d.original_appid.take())
    {
        tag_viewer(appid);
    }
}

fn session_focus_display(runtime: &Path) -> Option<String> {
    // This file belongs to the outer Game Mode session. DISPLAY in the
    // viewer's environment instead names Steam's secondary Xwayland server.
    fs::read_to_string(runtime.join("gamescope-environment"))
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("DISPLAY=").map(str::to_owned))
        .filter(|display| !display.is_empty())
}

fn sync_viewer(deck: &mut super::SteamDeckState, restore: bool, video_ready: bool) {
    let (Some(display), Some(shortcut), Some(game)) =
        (&deck.focus_display, deck.original_appid, deck.game_appid)
    else {
        return;
    };
    let Ok(display) = std::ffi::CString::new(display.as_str()) else {
        return;
    };
    let Ok(viewer_display) = std::ffi::CString::new(std::env::var("DISPLAY").unwrap_or_default())
    else {
        return;
    };
    let previous_menu = deck.menu_focus_override;
    let result = unsafe {
        michadame_deck_sync_focus(
            viewer_display.as_ptr(),
            display.as_ptr(),
            std::process::id(),
            shortcut,
            game,
            restore.into(),
            (video_ready && deck.focus_handoff_pending).into(),
            &mut deck.menu_focus_override,
        )
    };
    if result == 2 {
        deck.focus_handoff_pending = false;
        tracing::info!("Steam Deck launch handed back to viewer AppId={shortcut}");
    }
    if previous_menu == 0 && deck.menu_focus_override != 0 {
        tracing::info!("Steam Deck yielded focus to Steam's system menu");
    } else if previous_menu != 0 && deck.menu_focus_override == 0 {
        tracing::info!("Steam Deck released its system-menu focus override");
    }
}

pub fn prepare(state: &mut AppState, game: &crate::gamescope::steam::SteamGame) -> Result<()> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let sessions = super::logging::state_dir().join("sessions");
    fs::create_dir_all(&sessions)?;
    fs::set_permissions(&sessions, fs::Permissions::from_mode(0o700))?;
    if let Some(session) = reusable_session(&sessions, &game.appid, live_compositor)
        .or_else(|| reusable_session(&runtime, &game.appid, live_compositor))
    {
        tracing::info!(
            "Reconnecting to running Steam game '{}' using {}",
            game.name,
            session.display()
        );
        let lease = hold_lease(&session.join("viewer.lock"))?;
        attach_session(state, game, session, lease);
        return Ok(());
    }
    let session = tempfile::Builder::new()
        .prefix("michadame-deck-")
        .tempdir_in(sessions)?;
    let lease = hold_lease(&session.path().join("viewer.lock"))?;
    fs::write(
        session.path().join("capture-tag"),
        session.path().file_name().unwrap().as_encoded_bytes(),
    )?;
    fs::write(
        session.path().join("launch.log"),
        format!("Preparing Steam AppId={} ({})\n", game.appid, game.name),
    )?;
    let launcher = session.path().join("launcher.sh");
    fs::write(
        &launcher,
        include_str!("../../scripts/michadame-steam-deck.sh"),
    )?;
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700))?;
    // Steam is already running outside the portable viewer's environment.
    // Publish the stable helper path rather than changing Steam's library path.
    let pactl = crate::portable::helper_path("pactl");
    if pactl.is_absolute() {
        fs::write(
            session.path().join("pactl.path"),
            pactl.as_os_str().as_encoded_bytes(),
        )?;
    }
    let flock = crate::portable::helper_path("flock");
    if flock.is_absolute() {
        fs::write(
            session.path().join("flock.path"),
            flock.as_os_str().as_encoded_bytes(),
        )?;
    }
    // The endpoint is published from *inside* the launched Gamescope, so input
    // never guesses between the outer SteamOS compositor and the game compositor.
    fs::write(
        session.path().join("game.sh"),
        include_str!("../../scripts/michadame-steam-deck-game.sh"),
    )?;
    fs::write(session.path().join("appid"), &game.appid)?;
    crate::gamescope::steam::launch_steam_deck_game(game, &launcher.to_string_lossy())
        .context("Could not launch Steam game")?;
    attach_session(state, game, session.keep(), lease);
    Ok(())
}

fn attach_session(
    state: &mut AppState,
    game: &crate::gamescope::steam::SteamGame,
    session: PathBuf,
    lease: fs::File,
) {
    let deck = state.steam_deck.as_mut().unwrap();
    deck.session = Some(session);
    deck.viewer_lease = Some(lease);
    deck.capture_target = None;
    deck.game_display = None;
    deck.launch_started = Some(Instant::now());
    deck.last_launch_poll = None;
    deck.capture_scan = None;
    deck.video_capture = None;
    deck.capture_error = None;
    deck.capture_failures = 0;
    deck.gamepad = None;
    // STEAM_GAME is normally absent on the original window: Gamescope derives
    // its AppID from the process. Saving that missing property as zero cannot
    // restore the shortcut's identity after the game exits.
    deck.original_appid = std::env::var("SteamAppId")
        .ok()
        .and_then(|appid| appid.parse().ok())
        .filter(|appid| *appid != 0);
    deck.game_appid = game.appid.parse().ok();
    deck.focus_display = std::env::var_os("XDG_RUNTIME_DIR")
        .and_then(|runtime| session_focus_display(Path::new(&runtime)));
    if let Some(shortcut) = deck.original_appid {
        tag_viewer(shortcut);
        tracing::info!(
            "Steam Deck viewer retains Steam AppId={shortcut} and its controller layout"
        );
    }
    deck.focus_handoff_pending = true;
    sync_viewer(deck, false, false);
}

fn prop_u64(value: &serde_json::Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

/// Resolve the video source by its owning compositor PID, following client.id
/// when PipeWire puts process properties on the client instead of the node.
#[cfg(test)]
pub fn capture_target(bytes: &[u8], pid: u32) -> Option<String> {
    capture_target_for_session(bytes, Some(pid), None)
}

pub(crate) fn capture_target_for_session(
    bytes: &[u8],
    pid: Option<u32>,
    tag: Option<&str>,
) -> Option<String> {
    let objects: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let objects = objects.as_array()?;
    objects.iter().find_map(|node| {
        let props = &node["info"]["props"];
        if props["node.name"].as_str() != Some("gamescope")
            || props["media.class"].as_str() != Some("Video/Source")
        {
            return None;
        }
        let client = prop_u64(&props["client.id"])
            .and_then(|id| objects.iter().find(|o| prop_u64(&o["id"]) == Some(id)));
        let owner = prop_u64(&props["application.process.id"]).or_else(|| {
            client.and_then(|c| prop_u64(&c["info"]["props"]["application.process.id"]))
        });
        let matches = if let Some(tag) = tag {
            props["michadame.deck.session"].as_str().or_else(|| {
                client.and_then(|c| c["info"]["props"]["michadame.deck.session"].as_str())
            }) == Some(tag)
        } else {
            pid.is_some_and(|pid| owner == Some(pid as u64))
        };
        if !matches {
            return None;
        }
        prop_u64(&props["object.serial"]).map(|serial| serial.to_string())
    })
}

pub fn poll(state: &mut AppState, ctx: &egui::Context) {
    let Some(deck) = &mut state.steam_deck else {
        return;
    };
    if deck.session.is_none() {
        return;
    }
    if let Some(rx) = &deck.capture_scan {
        if let Ok(target) = rx.try_recv() {
            deck.capture_target = target;
            deck.capture_scan = None;
        }
    }
    let Some(hook) = &mut state.gamescope_hook else {
        return;
    };
    if deck
        .last_launch_poll
        .is_some_and(|last| last.elapsed() < Duration::from_millis(500))
    {
        return;
    }
    deck.last_launch_poll = Some(Instant::now());
    sync_viewer(deck, false, state.latest_frame.is_some());
    let session = deck.session.as_ref().unwrap();
    let pid = fs::read_to_string(session.join("gamescope.pid"))
        .ok()
        .and_then(|pid| pid.trim().parse::<u32>().ok());
    if pid.is_some_and(|pid| !live_compositor(pid, session)) {
        let had_capture = state.latest_frame.is_some();
        let log = session.join("launch.log");
        state.stop_stream(ctx);
        state.steam_deck.as_mut().unwrap().tab = Tab::Game;
        if had_capture {
            state.info("Steam game closed.");
        } else {
            state.error(format!(
                "Gamescope exited before capture started. Launch log: {}",
                log.display()
            ));
        }
        return;
    }
    // Wait for the launched game, never connect to SteamOS's EIS socket.
    if let Ok(endpoints) = fs::read_to_string(session.join("endpoints")) {
        let mut lines = endpoints.lines();
        if let (Some(wayland), Some(display)) = (lines.next(), lines.next()) {
            let wayland_path = std::path::Path::new(wayland);
            let private_socket =
                wayland_path.is_absolute() && wayland_path.starts_with(session.join("runtime"));
            if private_socket || wayland.starts_with("gamescope-") {
                if hook.input_client.is_none() {
                    if private_socket {
                        hook.input_client =
                            crate::gamescope::input::GamescopeInputClient::connect_to(
                                &std::path::PathBuf::from(format!("{wayland}-ei")),
                            )
                            .ok();
                    } else if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
                        hook.input_client =
                            crate::gamescope::input::GamescopeInputClient::connect_to(
                                &std::path::PathBuf::from(runtime).join(format!("{wayland}-ei")),
                            )
                            .ok();
                    }
                }
                if hook.x11_cursor_monitor.is_none() {
                    hook.x11_cursor_monitor =
                        crate::gamescope::x11_cursor::GamescopeX11CursorMonitor::start(display);
                }
                deck.game_display = Some(display.to_owned());
            }
        }
    }
    if deck.capture_target.is_none() && deck.capture_scan.is_none() {
        if pid.is_some() {
            let tag = fs::read_to_string(session.join("capture-tag")).ok();
            let (tx, rx) = crossbeam_channel::bounded(1);
            deck.capture_scan = Some(rx);
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let target =
                    crate::gamescope::video::run_pw_dump_with_timeout(None, Duration::from_secs(1))
                        .and_then(|bytes| capture_target_for_session(&bytes, pid, tag.as_deref()));
                let _ = tx.send(target);
                repaint.request_repaint();
            });
        }
    }
    ctx.request_repaint_after(Duration::from_millis(250));
}
