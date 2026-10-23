pub mod core_sync;
pub mod library;
pub mod libretro;

use crate::pc98::Pc98InputState;
use crossbeam_channel::{Receiver, Sender};
use library::Pc88GameEntry;
use serde::{Deserialize, Serialize};
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
    MountFdd {
        drive: u8,
        path: Option<PathBuf>,
    },
    /// Push updated core options to Quasi88.
    UpdateOptions(Pc88Config),
}

/// Status and lifecycle events emitted by the PC-88 worker thread back to the UI.
#[derive(Debug, Clone)]
pub enum Pc88Event {
    /// Core download / BIOS setup progress message.
    SyncProgress(String),
    /// Core initialized and started running at the given resolution and frame rate.
    Started {
        width: u32,
        height: u32,
        fps: f64,
    },
    /// Video resolution changed dynamically (e.g. 640x400 <-> 640x200).
    ResolutionChanged {
        width: u32,
        height: u32,
    },
    /// Informational toast message.
    Info(String),
    /// Fatal error message (core failed to download/load/boot).
    Failed(String),
    /// Worker loop exited cleanly.
    Stopped,
}

/// Runtime state of the PC-88 emulator subsystem stored on `AppState`.
pub struct Pc88State {
    pub config: Pc88Config,
    pub is_active: bool,
    pub passthrough_enabled: bool,
    pub status_message: Option<String>,
    /// Discovered games inside `config.games_dir`.
    pub games_list: Vec<Pc88GameEntry>,
    /// Live search filter string for the game library selector.
    pub game_filter: String,
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
}

impl Default for Pc88State {
    fn default() -> Self {
        Self {
            config: Pc88Config::default(),
            is_active: false,
            passthrough_enabled: true,
            status_message: None,
            games_list: Vec::new(),
            game_filter: String::new(),
            active_game_floppies: Vec::new(),
            active_game_disk_labels: Vec::new(),
            command_tx: None,
            event_rx: None,
            worker_thread: None,
            stop_requested: Arc::new(AtomicBool::new(false)),
            input_state: Arc::new(Mutex::new(Pc98InputState::default())),
            resolution: (640, 400),
            file_picker_rx: None,
        }
    }
}

impl Pc88State {
    /// Rescans `config.games_dir` and populates `games_list`.
    pub fn refresh_games_list(&mut self) -> anyhow::Result<usize> {
        let dir = self.config.games_dir.trim();
        if dir.is_empty() {
            self.games_list.clear();
            self.active_game_floppies.clear();
            self.active_game_disk_labels.clear();
            return Ok(0);
        }
        let entries = library::scan_games_directory(std::path::Path::new(dir))?;
        let count = entries.len();
        self.games_list = entries;

        if !self.config.selected_game.is_empty() {
            if let Some(entry) = self
                .games_list
                .iter()
                .find(|g| g.id == self.config.selected_game)
                .cloned()
            {
                let (floppies, labels) = library::materialize_game_floppies(&entry)
                    .unwrap_or_else(|_| {
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
                    });
                self.active_game_floppies = floppies;
                self.active_game_disk_labels = labels;
            }
        }
        Ok(count)
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
}
