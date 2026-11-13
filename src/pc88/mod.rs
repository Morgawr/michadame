pub mod core_sync;
pub mod library;
pub mod libretro;

use crate::pc98::{savestate, Pc98InputState};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use library::Pc88GameEntry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Which retro PC system is currently selected in the shared PC-98 / PC-88 settings tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RetroPcSystem {
    #[default]
    Pc98,
    Pc88,
}

/// Identifies a media slot on the PC-88 system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pc88MediaSlot {
    Fdd1,
    Fdd2,
    Tape,
    GamesDir,
}

/// Persistent configuration for the embedded NEC PC-88 (Quasi88) Libretro emulator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pc88Config {
    /// Root folder where PC-88 game folders are stored (e.g. `/home/morg/Games/PC88/GameList`).
    #[serde(default)]
    pub games_dir: String,
    /// Relative folder ID of the currently selected game in the library dropdown.
    #[serde(default)]
    pub selected_game: String,
    /// Path to Floppy Drive 1 image (.d88, .88d, .2hd, .fdi, .dsk).
    #[serde(default)]
    pub fdd1_path: String,
    /// Path to Floppy Drive 2 image (.d88, .88d, .2hd, .fdi, .dsk).
    #[serde(default)]
    pub fdd2_path: String,
    /// Path to Cassette Tape image (.t88, .cmt) or snapshot.
    #[serde(default)]
    pub tape_path: String,
    /// PC-88 BASIC / System Mode (`"N88 V2"`, `"N88 V1H"`, `"N88 V1S"`, `"N"`).
    #[serde(default = "default_pc88_basic_mode")]
    pub basic_mode: String,
    /// CPU Clock in MHz (`"4"`, `"8"`, `"16"`, `"32"`, `"64"`, `"2.53"`, `"2"`, `"1"`).
    #[serde(default = "default_pc88_cpu_clock")]
    pub cpu_clock: String,
    /// Sound board (`"OPNA"` = Sound Board II YM2608, `"OPN"` = YM2203).
    #[serde(default = "default_pc88_sound_board")]
    pub sound_board: String,
    /// Sub-CPU drive mode (`0` = Run during disk access, `1` = Run both during access, `2` = Always run both).
    #[serde(default = "default_pc88_sub_cpu_mode")]
    pub sub_cpu_mode: u32,
    /// Emulate accurate FDC wait timing (`true` recommended for compatibility).
    #[serde(default = "default_true")]
    pub fdc_wait: bool,
    /// Enable PCG-8100 programmable character generator (for older PC-8001 / V1 games).
    #[serde(default)]
    pub pcg_8100: bool,
    /// Screen size (`"full"` = 640x400, `"half"` = 320x200).
    #[serde(default = "default_pc88_screen_size")]
    pub screen_size: String,
    /// Allow games to write save data directly to mounted disk images.
    #[serde(default = "default_true")]
    pub save_to_disk_image: bool,
    /// Also send Numpad 8/2/4/6 when pressing Arrow keys (many PC-88 games only read numpad for movement).
    #[serde(default = "default_true")]
    pub numpad_arrow_keys: bool,
    /// Emulate mechanical floppy drive stepper motor seek sounds on FDC access.
    #[serde(default = "default_true")]
    pub floppy_seek_sound: bool,
    /// Volume (0..=100) for mechanical floppy drive seek sounds.
    #[serde(default = "default_pc88_floppy_seek_volume")]
    pub floppy_seek_volume: u32,
    /// Mouse speed multiplier when passthrough is active (`0.05..=2.0`, default `0.40`).
    #[serde(default = "default_pc88_mouse_sensitivity")]
    pub mouse_sensitivity: f32,
}

fn default_pc88_basic_mode() -> String {
    "N88 V2".to_string()
}

fn default_pc88_cpu_clock() -> String {
    "4".to_string()
}

fn default_pc88_sound_board() -> String {
    "OPNA".to_string()
}

fn default_pc88_sub_cpu_mode() -> u32 {
    0
}

fn default_pc88_screen_size() -> String {
    "full".to_string()
}

fn default_true() -> bool {
    true
}

fn default_pc88_floppy_seek_volume() -> u32 {
    80
}

fn default_pc88_mouse_sensitivity() -> f32 {
    0.40
}

impl Default for Pc88Config {
    fn default() -> Self {
        Self {
            games_dir: String::new(),
            selected_game: String::new(),
            fdd1_path: String::new(),
            fdd2_path: String::new(),
            tape_path: String::new(),
            basic_mode: default_pc88_basic_mode(),
            cpu_clock: default_pc88_cpu_clock(),
            sound_board: default_pc88_sound_board(),
            sub_cpu_mode: default_pc88_sub_cpu_mode(),
            fdc_wait: true,
            pcg_8100: false,
            screen_size: default_pc88_screen_size(),
            save_to_disk_image: true,
            numpad_arrow_keys: true,
            floppy_seek_sound: true,
            floppy_seek_volume: default_pc88_floppy_seek_volume(),
            mouse_sensitivity: default_pc88_mouse_sensitivity(),
        }
    }
}

/// Commands sent from the UI thread to the running PC-88 Libretro worker thread.
#[derive(Debug, Clone)]
pub enum Pc88Command {
    /// Gracefully stop the emulator and save SRAM/NVRAM + sync multi-image disks.
    Stop,
    /// Warm-reset the PC-88 system (`retro_reset`).
    Reset,
    /// Toggle the internal Quasi88 OSD menu (`F12`).
    TriggerMenu,
    /// Update master audio volume (`0.0..=1.0`).
    SetVolume(f32),
    /// Swap or eject a floppy disk image live (`drive`: 0 for FDD1, 1 for FDD2).
    MountFdd { drive: u8, path: Option<PathBuf> },
    /// Push updated core options to Quasi88.
    UpdateOptions(Pc88Config),
    /// Save the current emulator state + visual thumbnail into slot (`1..=8`).
    SaveState { slot: u8 },
    /// Load a previously saved emulator state from slot (`1..=8`).
    LoadState { slot: u8 },
}

/// Status and lifecycle events emitted by the PC-88 worker thread back to the UI.
#[derive(Debug, Clone)]
pub enum Pc88Event {
    /// Core download / BIOS setup progress message.
    SyncProgress(String),
    /// Core initialized and started running at the given resolution and frame rate.
    Started { width: u32, height: u32, fps: f64 },
    /// Video resolution changed dynamically (e.g. 640x400 <-> 640x200).
    ResolutionChanged { width: u32, height: u32 },
    /// Save state was written to disk along with its thumbnail and metadata.
    StateSaved {
        slot: u8,
        meta: savestate::SaveStateMeta,
    },
    /// Save state was restored from disk.
    StateLoaded {
        slot: u8,
        meta: savestate::SaveStateMeta,
    },
    /// Informational toast message.
    Info(String),
    /// Fatal error message (core failed to download/load/boot).
    Failed(String),
    /// Worker loop exited cleanly.
    Stopped,
}

pub struct Pc88LibraryScanResult {
    pub generation: u64,
    pub games_dir: String,
    pub selected_game_floppies: Option<(String, Vec<PathBuf>, Vec<String>)>,
    pub games: Result<Vec<Pc88GameEntry>, String>,
}

type CachedPc88GameKey = ([String; 5], (String, String));

/// Runtime state of the PC-88 emulator subsystem stored on `AppState`.
pub struct Pc88State {
    pub config: Pc88Config,
    pub is_active: bool,
    pub passthrough_enabled: bool,
    pub status_message: Option<String>,
    /// Discovered games inside `config.games_dir`.
    pub games_list: Vec<Pc88GameEntry>,
    pub library_revision: u64,
    pub is_scanning_library: bool,
    pub scan_generation: u64,
    pub library_scan_rx: Option<Receiver<Pc88LibraryScanResult>>,
    /// Live search filter string for the game library selector.
    pub game_filter: String,
    cached_filter_result: Option<(u64, usize, String, Arc<[usize]>)>,
    cached_game_key: Mutex<Option<CachedPc88GameKey>>,
    /// Floppy disk images belonging to the currently selected game (for quick live disk swapping).
    /// When a game uses multi-image `.d88` container(s), these are the materialized per-disk `.d88` slices.
    pub active_game_floppies: Vec<PathBuf>,
    /// Human-readable labels for `active_game_floppies` (e.g. `1: DiskA (GUIN.D88)`).
    pub active_game_disk_labels: Vec<String>,
    pub command_tx: Option<Sender<Pc88Command>>,
    pub event_rx: Option<Receiver<Pc88Event>>,
    pub worker_thread: Option<JoinHandle<()>>,
    pub stop_requested: Arc<AtomicBool>,
    pub input_state: Arc<Mutex<Pc98InputState>>,
    pub resolution: (u32, u32),
    /// Receiver for async native file dialog selections (`(slot, path)`).
    pub file_picker_rx: Option<Receiver<(Pc88MediaSlot, PathBuf)>>,
    pub active_slot: u8,
    pub slots_cache: [Option<savestate::SaveStateMeta>; savestate::SAVE_SLOT_COUNT],
    pub slots_cache_game_key: String,
    pub slot_textures: HashMap<(String, u8), Option<egui::TextureHandle>>,
    pub pending_boot_load_slot: Option<u8>,
}

impl Default for Pc88State {
    fn default() -> Self {
        Self {
            config: Pc88Config::default(),
            is_active: false,
            passthrough_enabled: true,
            status_message: None,
            games_list: Vec::new(),
            library_revision: 0,
            is_scanning_library: false,
            scan_generation: 0,
            library_scan_rx: None,
            game_filter: String::new(),
            cached_filter_result: None,
            cached_game_key: Mutex::new(None),
            active_game_floppies: Vec::new(),
            active_game_disk_labels: Vec::new(),
            command_tx: None,
            event_rx: None,
            worker_thread: None,
            stop_requested: Arc::new(AtomicBool::new(false)),
            input_state: Arc::new(Mutex::new(Pc98InputState::default())),
            resolution: (640, 400),
            file_picker_rx: None,
            active_slot: 1,
            slots_cache: Default::default(),
            slots_cache_game_key: String::new(),
            slot_textures: HashMap::new(),
            pending_boot_load_slot: None,
        }
    }
}

fn resolve_entry_floppies(entry: &Pc88GameEntry) -> (Vec<PathBuf>, Vec<String>) {
    library::materialize_game_floppies(entry).unwrap_or_else(|_| {
        let names = entry
            .floppies
            .iter()
            .enumerate()
            .map(|(i, p)| {
                format!(
                    "[{}] {}",
                    i + 1,
                    p.file_name().and_then(|s| s.to_str()).unwrap_or("disk")
                )
            })
            .collect();
        (entry.floppies.clone(), names)
    })
}

impl Pc88State {
    /// Rescans `config.games_dir` and populates `games_list`.
    pub fn refresh_games_list(&mut self) -> anyhow::Result<usize> {
        self.is_scanning_library = false;
        self.library_scan_rx = None;
        let dir = self.config.games_dir.trim();
        if dir.is_empty() {
            self.games_list.clear();
            self.active_game_floppies.clear();
            self.active_game_disk_labels.clear();
            self.library_revision = self.library_revision.wrapping_add(1);
            return Ok(0);
        }
        let entries = library::scan_games_directory(std::path::Path::new(dir))?;
        let count = entries.len();
        self.games_list = entries;
        self.library_revision = self.library_revision.wrapping_add(1);

        if !self.config.selected_game.is_empty() {
            if let Some(entry) = self
                .games_list
                .iter()
                .find(|g| g.id == self.config.selected_game)
                .cloned()
            {
                let (floppies, labels) = resolve_entry_floppies(&entry);
                self.active_game_floppies = floppies;
                self.active_game_disk_labels = labels;
            }
        }
        Ok(count)
    }

    /// Spawns a background `"pc88-library-scan"` thread to scan `config.games_dir` without
    /// blocking the UI thread. Poll completion via `poll_library_scan()`.
    pub fn start_background_library_scan(
        &mut self,
        notify_ctx: Option<egui::Context>,
    ) -> Result<(), String> {
        let dir_str = self.config.games_dir.trim().to_string();
        if dir_str.is_empty() {
            self.games_list.clear();
            self.active_game_floppies.clear();
            self.active_game_disk_labels.clear();
            self.library_revision = self.library_revision.wrapping_add(1);
            self.is_scanning_library = false;
            self.library_scan_rx = None;
            return Err("Please specify a PC-88 root games directory first.".to_string());
        }

        self.scan_generation = self.scan_generation.wrapping_add(1);
        let generation = self.scan_generation;
        let selected_game = self.config.selected_game.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.library_scan_rx = Some(rx);
        self.is_scanning_library = true;

        let games_dir_clone = dir_str.clone();
        let spawn_res = std::thread::Builder::new()
            .name("pc88-library-scan".to_string())
            .spawn(move || {
                let scan_res =
                    library::scan_games_directory(std::path::Path::new(&games_dir_clone))
                        .map_err(|e| format!("{e:#}"));
                let selected_game_floppies = match &scan_res {
                    Ok(entries) if !selected_game.is_empty() => {
                        entries.iter().find(|g| g.id == selected_game).map(|entry| {
                            let (floppies, labels) = resolve_entry_floppies(entry);
                            (selected_game, floppies, labels)
                        })
                    }
                    _ => None,
                };
                let _ = tx.send(Pc88LibraryScanResult {
                    generation,
                    games_dir: games_dir_clone,
                    selected_game_floppies,
                    games: scan_res,
                });
                if let Some(ctx) = notify_ctx {
                    ctx.request_repaint();
                }
            });

        if let Err(e) = spawn_res {
            self.is_scanning_library = false;
            self.library_scan_rx = None;
            return Err(format!("Failed to spawn PC-88 library scanner: {e}"));
        }
        Ok(())
    }

    /// Polls any in-flight background library scan and applies the discovered games if ready.
    pub fn poll_library_scan(&mut self) -> Option<Result<usize, String>> {
        let rx = self.library_scan_rx.as_ref()?;
        match rx.try_recv() {
            Ok(result) => {
                self.library_scan_rx = None;
                if result.generation != self.scan_generation
                    || self.config.games_dir.trim() != result.games_dir
                {
                    return None;
                }
                self.is_scanning_library = false;
                match result.games {
                    Ok(entries) => {
                        let count = entries.len();
                        self.games_list = entries;
                        self.library_revision = self.library_revision.wrapping_add(1);
                        if !self.config.selected_game.is_empty() {
                            if let Some((sel_id, floppies, labels)) = result.selected_game_floppies
                            {
                                if sel_id == self.config.selected_game {
                                    self.active_game_floppies = floppies;
                                    self.active_game_disk_labels = labels;
                                } else if let Some(entry) = self
                                    .games_list
                                    .iter()
                                    .find(|g| g.id == self.config.selected_game)
                                    .cloned()
                                {
                                    let (floppies, labels) = resolve_entry_floppies(&entry);
                                    self.active_game_floppies = floppies;
                                    self.active_game_disk_labels = labels;
                                }
                            } else if let Some(entry) = self
                                .games_list
                                .iter()
                                .find(|g| g.id == self.config.selected_game)
                                .cloned()
                            {
                                let (floppies, labels) = resolve_entry_floppies(&entry);
                                self.active_game_floppies = floppies;
                                self.active_game_disk_labels = labels;
                            }
                        }
                        Some(Ok(count))
                    }
                    Err(e) => Some(Err(e)),
                }
            }
            Err(crossbeam_channel::TryRecvError::Empty) => None,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.library_scan_rx = None;
                self.is_scanning_library = false;
                None
            }
        }
    }

    /// Returns the ranked game indices matching `self.game_filter`, cached by
    /// `(library_revision, games_list.len(), game_filter)` so unchanged search text
    /// does zero re-scoring across steady-state UI frames.
    pub fn filtered_game_indices(&mut self) -> Arc<[usize]> {
        if let Some((rev, len, ref q, ref indices)) = self.cached_filter_result {
            if rev == self.library_revision
                && len == self.games_list.len()
                && q == &self.game_filter
            {
                return Arc::clone(indices);
            }
        }
        let ranked: Arc<[usize]> =
            library::filter_and_rank_games(&self.games_list, &self.game_filter).into();
        self.cached_filter_result = Some((
            self.library_revision,
            self.games_list.len(),
            self.game_filter.clone(),
            Arc::clone(&ranked),
        ));
        ranked
    }

    /// Applies the media files of the selected game entry to `config` (FDD1, FDD2, Tape)
    /// and returns a human-readable summary of what was configured.
    pub fn select_game_by_index(&mut self, index: usize) -> anyhow::Result<String> {
        let Some(entry) = self.games_list.get(index).cloned() else {
            anyhow::bail!("Invalid game index {}", index);
        };

        self.config.selected_game = entry.id.clone();
        let (floppies, labels) = library::materialize_game_floppies(&entry)?;
        self.active_game_floppies = floppies.clone();
        self.active_game_disk_labels = labels;

        self.config.fdd1_path.clear();
        self.config.fdd2_path.clear();
        self.config.tape_path.clear();

        if let Some(d0) = floppies.first() {
            self.config.fdd1_path = d0.to_string_lossy().to_string();
        }
        if let Some(d1) = floppies.get(1) {
            self.config.fdd2_path = d1.to_string_lossy().to_string();
        }
        if let Some(t0) = entry.tapes.first() {
            self.config.tape_path = t0.to_string_lossy().to_string();
        }

        // If the game came from the PC-8001 category or is an N-BASIC tape, auto-select N-BASIC mode if appropriate
        if entry
            .category
            .as_deref()
            .is_some_and(|c| c.eq_ignore_ascii_case("PC-8001"))
        {
            self.config.basic_mode = "N".to_string();
        }

        Ok(format!(
            "Selected '{}' ({})",
            entry.title,
            entry.media_summary_with_resolved_count(self.active_game_floppies.len())
        ))
    }

    /// Toggles keyboard/mouse passthrough and returns the new state.
    pub fn toggle_passthrough(&mut self) -> bool {
        self.passthrough_enabled = !self.passthrough_enabled;
        if !self.passthrough_enabled {
            self.release_all_inputs();
        }
        self.passthrough_enabled
    }

    /// Clears all pressed keys and mouse buttons in the shared input state.
    pub fn release_all_inputs(&self) {
        if let Ok(mut guard) = self.input_state.lock() {
            guard.release_all();
        }
    }

    /// Sends a command to the running PC-88 worker thread if active.
    pub fn send_command(&self, cmd: Pc88Command) {
        if let Some(ref tx) = self.command_tx {
            let _ = tx.send(cmd);
        }
    }

    /// Signals the worker thread to stop.
    pub fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::SeqCst);
        if let Some(ref tx) = self.command_tx {
            let _ = tx.send(Pc88Command::Stop);
        }
    }

    pub fn current_game_key(&self) -> (String, String) {
        if let Ok(mut guard) = self.cached_game_key.lock() {
            if let Some((ref inputs, ref cached)) = *guard {
                if inputs[0] == self.config.games_dir
                    && inputs[1] == self.config.selected_game
                    && inputs[2] == self.config.fdd1_path
                    && inputs[3] == self.config.tape_path
                    && inputs[4] == self.config.fdd2_path
                {
                    return cached.clone();
                }
            }
            let computed = savestate::derive_pc88_game_key(&self.config);
            *guard = Some((
                [
                    self.config.games_dir.clone(),
                    self.config.selected_game.clone(),
                    self.config.fdd1_path.clone(),
                    self.config.tape_path.clone(),
                    self.config.fdd2_path.clone(),
                ],
                computed.clone(),
            ));
            return computed;
        }
        savestate::derive_pc88_game_key(&self.config)
    }

    pub fn ensure_slots_cache(&mut self) -> (String, String) {
        let (key, title) = self.current_game_key();
        if self.slots_cache_game_key != key {
            self.slots_cache = savestate::list_game_slots(&savestate::pc88_states_root(), &key);
            self.slots_cache_game_key = key.clone();
        }
        (key, title)
    }

    pub fn refresh_slots_cache(&mut self) {
        let (key, _) = self.current_game_key();
        self.slots_cache = savestate::list_game_slots(&savestate::pc88_states_root(), &key);
        self.slots_cache_game_key = key;
    }

    pub fn cycle_slot(&mut self, delta: i8) -> u8 {
        let count = savestate::SAVE_SLOT_COUNT as i8;
        let zero_based = (self.active_slot.saturating_sub(1) as i8).clamp(0, count - 1);
        let next = (zero_based + delta).rem_euclid(count);
        self.active_slot = (next + 1) as u8;
        self.active_slot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pc88_current_game_key_and_filtered_indices_caching() {
        let mut state = Pc88State::default();
        let default_key = state.current_game_key();

        state.config.fdd1_path = "/tmp/ys1_disk_a.d88".to_string();
        let ys_key = state.current_game_key();
        assert_ne!(default_key, ys_key);
        assert_eq!(state.current_game_key(), ys_key);

        state.config.fdd1_path = "/tmp/xanadu_disk_a.d88".to_string();
        assert_ne!(state.current_game_key(), ys_key);

        let tmp = tempfile::tempdir().unwrap();
        let gdir = tmp.path().join("Ys");
        std::fs::create_dir_all(&gdir).unwrap();
        std::fs::write(gdir.join("ys_a.d88"), vec![0u8; 1024]).unwrap();

        state.config.games_dir = tmp.path().to_string_lossy().to_string();
        state.start_background_library_scan(None).unwrap();
        assert!(state.is_scanning_library);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut completed = None;
        while std::time::Instant::now() < deadline {
            if let Some(res) = state.poll_library_scan() {
                completed = Some(res);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(completed, Some(Ok(1)));
        assert!(!state.is_scanning_library);
        assert_eq!(state.library_revision, 1);

        let idx1 = state.filtered_game_indices();
        let idx2 = state.filtered_game_indices();
        assert!(Arc::ptr_eq(&idx1, &idx2));

        state.game_filter = "ys".to_string();
        let idx3 = state.filtered_game_indices();
        assert!(!Arc::ptr_eq(&idx1, &idx3));
        assert_eq!(&*idx3, &[0]);
    }
}
