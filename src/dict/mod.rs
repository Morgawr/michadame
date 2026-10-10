pub mod database;
pub mod deinflect;
pub mod elongation;
pub mod frequency;
pub mod lookup;
pub mod long_vowel;
pub mod models;
pub mod particles;
pub mod popup;
pub mod render;
pub mod sync;

pub use database::DictDatabase;
pub use deinflect::{global_deinflector, Deinflector};
pub use frequency::FreqDatabase;
pub use models::*;

use crossbeam_channel::{Receiver, Sender};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Global state of the Yomitan / Jitendex dictionary subsystem in michadame.
pub struct DictState {
    /// Active SQLite dictionary connection, wrapped for cross-thread access.
    pub db: Arc<Mutex<Option<DictDatabase>>>,
    /// Currently installed dictionary metadata (from local SQLite db).
    pub installed_metadata: Option<DictMetadata>,
    /// Latest remote metadata checked from Jitendex.
    pub remote_metadata: Option<DictMetadata>,
    /// Whether an update is available (or dictionary is not yet installed).
    pub update_available: bool,
    /// Atomic flag indicating an update check is running.
    pub is_checking_update: Arc<AtomicBool>,
    /// Atomic flag indicating a dictionary download/sync is running.
    pub is_syncing: Arc<AtomicBool>,
    /// Current sync progress: (Status message, fraction 0.0..1.0).
    pub sync_progress: Arc<Mutex<Option<(String, f32)>>>,

    /// Active SQLite frequency dictionary connection.
    pub freq_db: Arc<Mutex<Option<FreqDatabase>>>,
    /// Currently installed frequency dictionary metadata.
    pub installed_freq_metadata: Option<DictMetadata>,
    /// Latest remote frequency metadata checked from Jiten.
    pub remote_freq_metadata: Option<DictMetadata>,
    /// Whether a frequency update is available (or frequency dictionary is not yet installed).
    pub freq_update_available: bool,
    /// Atomic flag indicating a frequency update check is running.
    pub is_checking_freq_update: Arc<AtomicBool>,
    /// Atomic flag indicating a frequency download/sync is running.
    pub is_freq_syncing: Arc<AtomicBool>,
    /// Current frequency sync progress: (Status message, fraction 0.0..1.0).
    pub freq_sync_progress: Arc<Mutex<Option<(String, f32)>>>,

    /// Local directory where dictionary database and temporary downloads are stored.
    pub dict_dir: PathBuf,
    /// Channel sender for worker events.
    pub event_tx: Sender<DictEvent>,
    /// Channel receiver for completed worker events.
    pub event_rx: Option<Receiver<DictEvent>>,
    /// Active popup state when user hovers over an OCR word.
    pub popup: Option<DictPopupState>,
    /// Whether the OCR dictionary popup appears underneath CRT and glass filters.
    pub popup_under_crt: bool,
}

impl DictState {
    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();

        let dict_dir = resolve_dict_directory();
        let db_path = dict_dir.join("jitendex.db");
        let freq_db_path = dict_dir.join("jiten_freq.db");

        let mut installed_metadata = None;
        let mut db_handle = None;

        if db_path.exists() {
            match DictDatabase::open(&db_path) {
                Ok(database) => match database.get_metadata() {
                    Ok(meta) => {
                        tracing::info!(
                            "Loaded Jitendex dictionary '{}' (rev {}) with {} entries",
                            meta.title,
                            meta.revision,
                            meta.total_entries
                        );
                        installed_metadata = Some(meta);
                        db_handle = Some(database);
                    }
                    Err(e) => tracing::warn!("Failed to read metadata from {}: {e}", db_path.display()),
                },
                Err(e) => tracing::warn!("Failed to open dictionary at {}: {e}", db_path.display()),
            }
        }

        let mut installed_freq_metadata = None;
        let mut freq_db_handle = None;

        if freq_db_path.exists() {
            match FreqDatabase::open(&freq_db_path) {
                Ok(database) => match database.get_metadata() {
                    Ok(meta) => {
                        tracing::info!(
                            "Loaded Jiten frequency dictionary '{}' (rev {}) with {} entries",
                            meta.title,
                            meta.revision,
                            meta.total_entries
                        );
                        installed_freq_metadata = Some(meta);
                        freq_db_handle = Some(database);
                    }
                    Err(e) => tracing::warn!("Failed to read freq metadata from {}: {e}", freq_db_path.display()),
                },
                Err(e) => tracing::warn!("Failed to open freq dictionary at {}: {e}", freq_db_path.display()),
            }
        }

        let is_checking_update = Arc::new(AtomicBool::new(false));
        let is_syncing = Arc::new(AtomicBool::new(false));
        let sync_progress = Arc::new(Mutex::new(None));

        let is_checking_freq_update = Arc::new(AtomicBool::new(false));
        let is_freq_syncing = Arc::new(AtomicBool::new(false));
        let freq_sync_progress = Arc::new(Mutex::new(None));

        // If not installed, mark update_available as true immediately
        let update_available = installed_metadata.is_none();
        let freq_update_available = installed_freq_metadata.is_none();

        // Trigger background check for remote version
        let current_rev = installed_metadata.as_ref().map(|m| m.revision.clone());
        sync::spawn_check_version(current_rev, is_checking_update.clone(), tx.clone());

        let current_freq_rev = installed_freq_metadata.as_ref().map(|m| m.revision.clone());
        sync::spawn_check_freq_version(current_freq_rev, is_checking_freq_update.clone(), tx.clone());

        Self {
            db: Arc::new(Mutex::new(db_handle)),
            installed_metadata,
            remote_metadata: None,
            update_available,
            is_checking_update,
            is_syncing,
            sync_progress,
            freq_db: Arc::new(Mutex::new(freq_db_handle)),
            installed_freq_metadata,
            remote_freq_metadata: None,
            freq_update_available,
            is_checking_freq_update,
            is_freq_syncing,
            freq_sync_progress,
            dict_dir,
            event_tx: tx,
            event_rx: Some(rx),
            popup: None,
            popup_under_crt: false,
        }
    }

    /// Triggers remote sync / download and indexing in a background thread.
    pub fn trigger_sync(&mut self) {
        if self.is_syncing.load(Ordering::Relaxed) {
            return;
        }

        let download_url = self
            .remote_metadata
            .as_ref()
            .and_then(|m| m.download_url.clone());

        sync::spawn_sync(
            download_url,
            self.dict_dir.clone(),
            self.is_syncing.clone(),
            self.event_tx.clone(),
        );
    }

    /// Triggers a version check against jitendex.org.
    pub fn trigger_check_version(&mut self) {
        let current_rev = self.installed_metadata.as_ref().map(|m| m.revision.clone());
        sync::spawn_check_version(
            current_rev,
            self.is_checking_update.clone(),
            self.event_tx.clone(),
        );
    }

    /// Triggers remote sync / download of Jiten frequency dictionary.
    pub fn trigger_freq_sync(&mut self) {
        if self.is_freq_syncing.load(Ordering::Relaxed) {
            return;
        }

        let download_url = self
            .remote_freq_metadata
            .as_ref()
            .and_then(|m| m.download_url.clone());

        sync::spawn_freq_sync(
            download_url,
            self.dict_dir.clone(),
            self.is_freq_syncing.clone(),
            self.event_tx.clone(),
        );
    }

    /// Triggers a version check against jiten.moe frequency index.
    pub fn trigger_check_freq_version(&mut self) {
        let current_rev = self.installed_freq_metadata.as_ref().map(|m| m.revision.clone());
        sync::spawn_check_freq_version(
            current_rev,
            self.is_checking_freq_update.clone(),
            self.event_tx.clone(),
        );
    }
}

impl Default for DictState {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolves standard directory path for storing dictionary files (~/.config/michadame/dict).
pub fn resolve_dict_directory() -> PathBuf {
    if let Ok(config_path) = confy::get_configuration_file_path("michadame", None) {
        if let Some(parent) = config_path.parent() {
            return parent.join("dict");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("michadame").join("dict")
    } else {
        PathBuf::from("./dict")
    }
}
