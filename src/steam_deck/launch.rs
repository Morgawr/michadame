use super::*;
use anyhow::{Context, Result};
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
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

#[repr(C)]
struct MichadameDeckFocusCtx {
    _private: [u8; 0],
}

extern "C" {
    fn michadame_deck_focus_ctx_create() -> *mut MichadameDeckFocusCtx;
    fn michadame_deck_focus_ctx_sync(
        ctx: *mut MichadameDeckFocusCtx,
        viewer_display: *const std::ffi::c_char,
        control_display: *const std::ffi::c_char,
        pid: u32,
        shortcut: u32,
        game: u32,
        restore: i32,
        handoff: i32,
        menu_override: *mut u32,
    ) -> i32;
    fn michadame_deck_focus_ctx_destroy(ctx: *mut MichadameDeckFocusCtx);
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

struct FocusSyncParams {
    viewer_display: std::ffi::CString,
    control_display: std::ffi::CString,
    pid: u32,
    shortcut: u32,
    game: u32,
}

enum FocusWorkerCmd {
    SetVideoReady(bool),
    AttachCursorMonitor(String),
    RestoreAndStop { restore_appid: Option<u32> },
}

struct DeckFocusShared {
    handoff_completed: AtomicBool,
    menu_focus_override: AtomicU32,
    cursor_ready: AtomicBool,
    cursor_in_flight: AtomicBool,
    pending_cursor: Mutex<Option<crate::gamescope::x11_cursor::GamescopeX11CursorMonitor>>,
}

/// Persistent background X11 focus & cursor-attach worker (`"deck-focus-sync"`).
/// Keeps persistent X11 `Display*` handles off the 60 FPS UI thread and publishes
/// focus status and cursor monitors back via atomics / `try_lock`.
pub struct DeckFocusWorker {
    cmd_tx: Option<crossbeam_channel::Sender<FocusWorkerCmd>>,
    shared: Arc<DeckFocusShared>,
    last_video_ready: bool,
    last_cursor_display: Option<String>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl DeckFocusWorker {
    pub fn start(focus_display: Option<&str>, shortcut: Option<u32>, game: Option<u32>) -> Self {
        let params = match (focus_display, shortcut, game) {
            (Some(ctrl), Some(shortcut), Some(game)) => {
                let ctrl_c = std::ffi::CString::new(ctrl).ok();
                let viewer_c =
                    std::ffi::CString::new(std::env::var("DISPLAY").unwrap_or_default()).ok();
                match (viewer_c, ctrl_c) {
                    (Some(viewer_display), Some(control_display)) => Some(FocusSyncParams {
                        viewer_display,
                        control_display,
                        pid: std::process::id(),
                        shortcut,
                        game,
                    }),
                    _ => None,
                }
            }
            _ => None,
        };

        let shared = Arc::new(DeckFocusShared {
            handoff_completed: AtomicBool::new(false),
            menu_focus_override: AtomicU32::new(0),
            cursor_ready: AtomicBool::new(false),
            cursor_in_flight: AtomicBool::new(false),
            pending_cursor: Mutex::new(None),
        });
        let worker_shared = Arc::clone(&shared);
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<FocusWorkerCmd>();

        let handle = std::thread::Builder::new()
            .name("deck-focus-sync".to_string())
            .spawn(move || {
                let focus_ctx = unsafe { michadame_deck_focus_ctx_create() };
                let mut video_ready = false;
                let mut handoff_pending = true;
                let mut menu_override = 0u32;
                let mut restore_on_exit: Option<Option<u32>> = None;

                Self::run_sync_step(
                    focus_ctx,
                    params.as_ref(),
                    false,
                    video_ready,
                    &mut handoff_pending,
                    &mut menu_override,
                    &worker_shared,
                );

                loop {
                    let mut cursor_req: Option<String> = None;
                    match cmd_rx.recv_timeout(Duration::from_millis(200)) {
                        Ok(first_cmd) => {
                            let mut cmds = vec![first_cmd];
                            while let Ok(next_cmd) = cmd_rx.try_recv() {
                                cmds.push(next_cmd);
                            }
                            for cmd in cmds {
                                match cmd {
                                    FocusWorkerCmd::SetVideoReady(ready) => {
                                        video_ready = ready;
                                    }
                                    FocusWorkerCmd::AttachCursorMonitor(disp) => {
                                        cursor_req = Some(disp);
                                    }
                                    FocusWorkerCmd::RestoreAndStop { restore_appid } => {
                                        restore_on_exit = Some(restore_appid);
                                    }
                                }
                            }
                            if restore_on_exit.is_some() {
                                break;
                            }
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                            break;
                        }
                    }

                    if let Some(disp) = cursor_req {
                        if let Some(mon) =
                            crate::gamescope::x11_cursor::GamescopeX11CursorMonitor::start(&disp)
                        {
                            if let Ok(mut slot) = worker_shared.pending_cursor.lock() {
                                *slot = Some(mon);
                                worker_shared.cursor_ready.store(true, Ordering::Release);
                            }
                        }
                        worker_shared
                            .cursor_in_flight
                            .store(false, Ordering::Release);
                    }

                    Self::run_sync_step(
                        focus_ctx,
                        params.as_ref(),
                        false,
                        video_ready,
                        &mut handoff_pending,
                        &mut menu_override,
                        &worker_shared,
                    );
                }

                if let Some(restore_appid) = restore_on_exit {
                    Self::run_sync_step(
                        focus_ctx,
                        params.as_ref(),
                        true,
                        false,
                        &mut handoff_pending,
                        &mut menu_override,
                        &worker_shared,
                    );
                    if let Some(appid) = restore_appid {
                        let _ = tag_viewer(appid);
                    }
                }

                if !focus_ctx.is_null() {
                    unsafe {
                        michadame_deck_focus_ctx_destroy(focus_ctx);
                    }
                }
            })
            .ok();

        Self {
            cmd_tx: handle.is_some().then_some(cmd_tx),
            shared,
            last_video_ready: false,
            last_cursor_display: None,
            handle,
        }
    }

    fn run_sync_step(
        focus_ctx: *mut MichadameDeckFocusCtx,
        params: Option<&FocusSyncParams>,
        restore: bool,
        video_ready: bool,
        handoff_pending: &mut bool,
        menu_override: &mut u32,
        shared: &DeckFocusShared,
    ) {
        let Some(p) = params else {
            return;
        };
        if focus_ctx.is_null() {
            return;
        }
        let prev_menu = *menu_override;
        let rc = unsafe {
            michadame_deck_focus_ctx_sync(
                focus_ctx,
                p.viewer_display.as_ptr(),
                p.control_display.as_ptr(),
                p.pid,
                p.shortcut,
                p.game,
                restore.into(),
                (video_ready && *handoff_pending).into(),
                menu_override,
            )
        };
        if rc == 2 {
            *handoff_pending = false;
            shared.handoff_completed.store(true, Ordering::Release);
            tracing::info!(
                "Steam Deck launch handed back to viewer AppId={}",
                p.shortcut
            );
        }
        shared
            .menu_focus_override
            .store(*menu_override, Ordering::Release);
        if prev_menu == 0 && *menu_override != 0 {
            tracing::info!("Steam Deck yielded focus to Steam's system menu");
        } else if prev_menu != 0 && *menu_override == 0 {
            tracing::info!("Steam Deck released its system-menu focus override");
        }
    }

    pub fn request_cursor_monitor(&mut self, display: &str) {
        if self.shared.cursor_in_flight.load(Ordering::Acquire) {
            return;
        }
        if self.last_cursor_display.as_deref() == Some(display)
            && self.shared.cursor_ready.load(Ordering::Acquire)
        {
            return;
        }
        if let Some(ref tx) = self.cmd_tx {
            self.last_cursor_display = Some(display.to_owned());
            self.shared.cursor_in_flight.store(true, Ordering::Release);
            if tx
                .send(FocusWorkerCmd::AttachCursorMonitor(display.to_owned()))
                .is_err()
            {
                self.shared.cursor_in_flight.store(false, Ordering::Release);
            }
        }
    }

    pub fn restore_and_stop(mut self, restore_appid: Option<u32>) -> (bool, u32) {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(FocusWorkerCmd::RestoreAndStop { restore_appid });
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        (
            self.shared.handoff_completed.load(Ordering::Acquire),
            self.shared.menu_focus_override.load(Ordering::Acquire),
        )
    }
}

impl Drop for DeckFocusWorker {
    fn drop(&mut self) {
        drop(self.cmd_tx.take());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Non-blocking (< 1 µs) UI-thread focus state & cursor-monitor synchronization.
pub fn poll_focus_management(
    deck: &mut super::SteamDeckState,
    hook: Option<&mut crate::gamescope::GamescopeHookState>,
    video_ready: bool,
) {
    let Some(worker) = deck.focus_worker.as_mut() else {
        return;
    };
    if worker.last_video_ready != video_ready {
        worker.last_video_ready = video_ready;
        if let Some(ref tx) = worker.cmd_tx {
            let _ = tx.send(FocusWorkerCmd::SetVideoReady(video_ready));
        }
    }
    if worker.shared.handoff_completed.load(Ordering::Acquire) {
        deck.focus_handoff_pending = false;
    }
    deck.menu_focus_override = worker.shared.menu_focus_override.load(Ordering::Acquire);

    if let Some(hook) = hook {
        if worker.shared.cursor_ready.load(Ordering::Acquire) {
            if let Ok(mut slot) = worker.shared.pending_cursor.try_lock() {
                worker.shared.cursor_ready.store(false, Ordering::Release);
                if let Some(mon) = slot.take() {
                    if hook.x11_cursor_monitor.is_none() {
                        hook.x11_cursor_monitor = Some(mon);
                    }
                }
            }
        }
    }
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
    let Some(deck) = &mut state.steam_deck else {
        return;
    };
    let restore_appid = deck.original_appid.take();
    if let Some(worker) = deck.focus_worker.take() {
        let (handed_off, menu_override) = worker.restore_and_stop(restore_appid);
        if handed_off {
            deck.focus_handoff_pending = false;
        }
        deck.menu_focus_override = menu_override;
    } else {
        sync_viewer(deck, true, false);
        if let Some(appid) = restore_appid {
            tag_viewer(appid);
        }
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
    deck.forwarded_modifiers_mask = 0;
    deck.focus_worker = None;
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
    deck.menu_focus_override = 0;
    deck.focus_worker = Some(DeckFocusWorker::start(
        deck.focus_display.as_deref(),
        deck.original_appid,
        deck.game_appid,
    ));
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
    let video_ready = state.latest_frame.is_some();
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
        poll_focus_management(deck, None, video_ready);
        return;
    };
    poll_focus_management(deck, Some(hook), video_ready);
    if deck
        .last_launch_poll
        .is_some_and(|last| last.elapsed() < Duration::from_millis(500))
    {
        return;
    }
    deck.last_launch_poll = Some(Instant::now());
    let session = deck.session.as_ref().unwrap();
    let pid = fs::read_to_string(session.join("gamescope.pid"))
        .ok()
        .and_then(|pid| pid.trim().parse::<u32>().ok());
    if pid.is_some_and(|pid| !live_compositor(pid, session)) {
        let had_capture = video_ready;
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
                    deck.forwarded_modifiers_mask = 0;
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
                    if let Some(ref mut worker) = deck.focus_worker {
                        worker.request_cursor_monitor(display);
                    } else {
                        let mut worker = DeckFocusWorker::start(
                            deck.focus_display.as_deref(),
                            deck.original_appid,
                            deck.game_appid,
                        );
                        worker.request_cursor_monitor(display);
                        deck.focus_worker = Some(worker);
                    }
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
