pub mod core_sync;
pub mod input;
pub mod kanji_table;
pub mod library;
pub mod libretro;
pub mod savestate;
pub mod search;

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::JoinHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pc98MediaSlot {
    Fdd1,
    Fdd2,
    Hdd1,
    Hdd2,
    CdRom,
    GamesDir,
    DosHdd,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Pc98Config {
    pub games_dir: String,
    pub dos_hdd_path: String,
    pub selected_game: String,
    pub fdd1_path: String,
    pub fdd2_path: String,
    pub hdd1_path: String,
    pub hdd2_path: String,
    pub cdrom_path: String,
    pub model: String,
    pub cpu_feature: String,
    pub base_clock: String,
    pub clock_mult: u32,
    pub ram_mb: u32,
    pub sound_board: String,
    pub gdc_type: String,
    pub skipline_revisions: bool,
    pub fast_memcheck: bool,
    pub floppy_seek_sound: bool,
    pub floppy_seek_volume: u32,
    pub beep_volume: u32,
    #[serde(default = "default_pc98_mouse_sensitivity")]
    pub mouse_sensitivity: f32,
    #[serde(default = "default_pc98_numpad_arrow_keys")]
    pub numpad_arrow_keys: bool,
}

pub fn default_pc98_mouse_sensitivity() -> f32 {
    0.5
}

pub fn default_pc98_numpad_arrow_keys() -> bool {
    true
}

impl Default for Pc98Config {
    fn default() -> Self {
        Self {
            games_dir: String::new(),
            dos_hdd_path: String::new(),
            selected_game: String::new(),
            fdd1_path: String::new(),
            fdd2_path: String::new(),
            hdd1_path: String::new(),
            hdd2_path: String::new(),
            cdrom_path: String::new(),
            model: "PC-9801VX".to_string(),
            cpu_feature: "Intel 80386".to_string(),
            base_clock: "2.4576 MHz".to_string(),
            clock_mult: 16,
            ram_mb: 16,
            sound_board: "PC9801-26K + 86".to_string(),
            gdc_type: "uPD7220".to_string(),
            skipline_revisions: true,
            fast_memcheck: true,
            floppy_seek_sound: true,
            floppy_seek_volume: 80,
            beep_volume: 3,
            mouse_sensitivity: default_pc98_mouse_sensitivity(),
            numpad_arrow_keys: default_pc98_numpad_arrow_keys(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Pc98Command {
    Stop,
    Reset,
    SetVolume(f32),
    MountFdd { drive: u8, path: Option<PathBuf> },
    TriggerMenu,
    UpdateOptions(Pc98Config),
    SaveState { slot: u8 },
    LoadState { slot: u8 },
}

#[derive(Debug, Clone)]
pub enum Pc98Event {
    SyncProgress(String),
    Started {
        width: u32,
        height: u32,
        fps: f64,
    },
    ResolutionChanged {
        width: u32,
        height: u32,
    },
    StateSaved {
        slot: u8,
        meta: savestate::SaveStateMeta,
    },
    StateLoaded {
        slot: u8,
        meta: savestate::SaveStateMeta,
    },
    Info(String),
    Failed(String),
    Stopped,
}

pub struct Pc98InputState {
    pub keys: [bool; 512],
    pub mouse_dx: i32,
    pub mouse_dy: i32,
    pub mouse_rem_x: f32,
    pub mouse_rem_y: f32,
    pub mouse_left: bool,
    pub mouse_right: bool,
    pub mouse_middle: bool,
    /// Keys pulsed for a small number of emulator frames `(retro_key, remaining_frames)`.
    pub pulse_keys: Vec<(u32, u8)>,
}

impl Default for Pc98InputState {
    fn default() -> Self {
        Self {
            keys: [false; 512],
            mouse_dx: 0,
            mouse_dy: 0,
            mouse_rem_x: 0.0,
            mouse_rem_y: 0.0,
            mouse_left: false,
            mouse_right: false,
            mouse_middle: false,
            pulse_keys: Vec::new(),
        }
    }
}

impl Pc98InputState {
    pub fn release_all(&mut self) {
        self.keys.fill(false);
        self.mouse_dx = 0;
        self.mouse_dy = 0;
        self.mouse_rem_x = 0.0;
        self.mouse_rem_y = 0.0;
        self.mouse_left = false;
        self.mouse_right = false;
        self.mouse_middle = false;
        self.pulse_keys.clear();
    }

    pub fn pulse_key(&mut self, retro_key: u32, frames: u8) {
        if let Some(entry) = self.pulse_keys.iter_mut().find(|(k, _)| *k == retro_key) {
            entry.1 = entry.1.max(frames);
        } else {
            self.pulse_keys.push((retro_key, frames));
        }
    }
}

pub struct Pc98State {
    pub is_active: bool,
    pub is_starting: bool,
    pub status_message: Option<String>,
    pub passthrough_enabled: bool,
    pub config: Pc98Config,
    pub games_list: Vec<library::Pc98GameEntry>,
    pub game_filter: String,
    pub resolution: (u32, u32),
    pub input_state: Arc<Mutex<Pc98InputState>>,
    pub command_tx: Option<crossbeam_channel::Sender<Pc98Command>>,
    pub event_rx: Option<crossbeam_channel::Receiver<Pc98Event>>,
    pub stop_requested: Arc<AtomicBool>,
    pub worker_thread: Option<JoinHandle<()>>,
    pub prev_modifiers: egui::Modifiers,
    pub file_picker_rx: Option<crossbeam_channel::Receiver<(Pc98MediaSlot, PathBuf)>>,
    pub active_slot: u8,
    pub slots_cache: [Option<savestate::SaveStateMeta>; savestate::SAVE_SLOT_COUNT],
    pub slots_cache_game_key: String,
    pub slot_textures: HashMap<(String, u8), egui::TextureHandle>,
    pub pending_boot_load_slot: Option<u8>,
}

impl Default for Pc98State {
    fn default() -> Self {
        Self {
            is_active: false,
            is_starting: false,
            status_message: None,
            passthrough_enabled: true,
            config: Pc98Config::default(),
            games_list: Vec::new(),
            game_filter: String::new(),
            resolution: (640, 400),
            input_state: Arc::new(Mutex::new(Pc98InputState::default())),
            command_tx: None,
            event_rx: None,
            stop_requested: Arc::new(AtomicBool::new(false)),
            worker_thread: None,
            prev_modifiers: egui::Modifiers::default(),
            file_picker_rx: None,
            active_slot: 1,
            slots_cache: Default::default(),
            slots_cache_game_key: String::new(),
            slot_textures: HashMap::new(),
            pending_boot_load_slot: None,
        }
    }
}

impl Pc98State {
    pub fn refresh_games_list(&mut self) -> Result<usize, String> {
        let dir_str = self.config.games_dir.trim().to_string();
        if dir_str.is_empty() {
            self.games_list.clear();
            return Err("Please specify a PC-98 root games directory first.".to_string());
        }
        let root_path = PathBuf::from(&dir_str);
        if self.config.dos_hdd_path.trim().is_empty() {
            if let Some(detected_dos) = library::detect_default_dos_hdd(&root_path) {
                self.config.dos_hdd_path = detected_dos.to_string_lossy().to_string();
            }
        }
        match library::scan_games_directory(&root_path, &self.config.dos_hdd_path) {
            Ok(list) => {
                let count = list.len();
                self.games_list = list;
                Ok(count)
            }
            Err(e) => Err(format!("{e:#}")),
        }
    }

    pub fn select_game_by_index(&mut self, index: usize) -> Result<String, String> {
        let Some(mut game) = self.games_list.get(index).cloned() else {
            return Err("Invalid game selection index.".to_string());
        };
        library::apply_game_to_config(&mut game, &mut self.config).map_err(|e| format!("{e:#}"))?;
        let title = game.title.clone();
        self.games_list[index] = game;

        if self.is_active {
            let f1 = if self.config.fdd1_path.trim().is_empty() {
                None
            } else {
                Some(PathBuf::from(self.config.fdd1_path.trim()))
            };
            let f2 = if self.config.fdd2_path.trim().is_empty() {
                None
            } else {
                Some(PathBuf::from(self.config.fdd2_path.trim()))
            };
            self.send_command(Pc98Command::MountFdd { drive: 0, path: f1 });
            self.send_command(Pc98Command::MountFdd { drive: 1, path: f2 });
        }

        Ok(title)
    }

    pub fn toggle_passthrough(&mut self) -> bool {
        self.passthrough_enabled = !self.passthrough_enabled;
        if !self.passthrough_enabled {
            self.release_all_inputs();
        }
        self.passthrough_enabled
    }

    pub fn release_all_inputs(&mut self) {
        if let Ok(mut input) = self.input_state.lock() {
            input.release_all();
        }
        self.prev_modifiers = egui::Modifiers::default();
    }

    pub fn send_command(&self, cmd: Pc98Command) {
        if let Some(ref tx) = self.command_tx {
            let _ = tx.send(cmd);
        }
    }

    pub fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::Relaxed);
    }

    pub fn current_game_key(&self) -> (String, String) {
        savestate::derive_pc98_game_key(&self.config)
    }

    pub fn ensure_slots_cache(&mut self) -> (String, String) {
        let (key, title) = self.current_game_key();
        if self.slots_cache_game_key != key {
            self.slots_cache =
                savestate::list_game_slots(&savestate::pc98_states_root(), &key);
            self.slots_cache_game_key = key.clone();
        }
        (key, title)
    }

    pub fn refresh_slots_cache(&mut self) {
        let (key, _) = self.current_game_key();
        self.slots_cache = savestate::list_game_slots(&savestate::pc98_states_root(), &key);
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
