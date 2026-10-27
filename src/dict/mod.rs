pub mod database;
pub mod deinflect;
pub mod elongation;
pub mod frequency;
pub mod long_vowel;
pub mod lookup;
pub mod models;
pub mod particles;
pub mod pitch;
pub mod popup;
pub mod render;
pub mod sync;

pub use database::DictDatabase;
pub use deinflect::{global_deinflector, Deinflector};
pub use frequency::FreqDatabase;
pub use models::*;
pub use pitch::PitchDatabase;

use crossbeam_channel::{Receiver, Sender};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static OFFLINE_DICT_COUNTER: AtomicU64 = AtomicU64::new(1);

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

    /// Active SQLite pitch accent dictionary connection (imported from a local Yomitan .zip).
    pub pitch_db: Arc<Mutex<Option<PitchDatabase>>>,
    /// Currently installed pitch accent dictionary metadata.
    pub installed_pitch_metadata: Option<DictMetadata>,
    /// Atomic flag indicating a pitch dictionary import is running.
    pub is_pitch_importing: Arc<AtomicBool>,
    /// Current pitch dictionary import progress: (Status message, fraction 0.0..1.0).
    pub pitch_import_progress: Arc<Mutex<Option<(String, f32)>>>,

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
        #[cfg(test)]
        {
            Self::new_offline()
        }
        #[cfg(not(test))]
        {
            Self::with_dir(resolve_dict_directory(), true)
        }
    }

    /// Creates an isolated, offline `DictState` backed by a temporary directory without
    /// spawning background HTTP version-check threads or touching the user's XDG config.
    pub fn new_offline() -> Self {
        let seq = OFFLINE_DICT_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dict_dir = std::env::temp_dir().join(format!(
            "michadame-offline-dict-{}-{seq}",
            std::process::id()
        ));
        Self::with_dir(dict_dir, false)
    }

    /// Creates a `DictState` rooted at `dict_dir`, optionally spawning remote version-check threads.
    pub fn with_dir(dict_dir: PathBuf, spawn_remote_checks: bool) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();

        let db_path = dict_dir.join("jitendex.db");
        let freq_db_path = dict_dir.join("jiten_freq.db");
        let pitch_db_path = dict_dir.join("pitch.db");

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
                    Err(e) => {
                        tracing::warn!("Failed to read metadata from {}: {e}", db_path.display())
                    }
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
                    Err(e) => tracing::warn!(
                        "Failed to read freq metadata from {}: {e}",
                        freq_db_path.display()
                    ),
                },
                Err(e) => tracing::warn!(
                    "Failed to open freq dictionary at {}: {e}",
                    freq_db_path.display()
                ),
            }
        }

        let mut installed_pitch_metadata = None;
        let mut pitch_db_handle = None;

        if pitch_db_path.exists() {
            match PitchDatabase::open(&pitch_db_path) {
                Ok(database) => match database.get_metadata() {
                    Ok(meta) => {
                        tracing::info!(
                            "Loaded pitch accent dictionary '{}' (rev {}) with {} entries",
                            meta.title,
                            meta.revision,
                            meta.total_entries
                        );
                        installed_pitch_metadata = Some(meta);
                        pitch_db_handle = Some(database);
                    }
                    Err(e) => tracing::warn!(
                        "Failed to read pitch metadata from {}: {e}",
                        pitch_db_path.display()
                    ),
                },
                Err(e) => tracing::warn!(
                    "Failed to open pitch dictionary at {}: {e}",
                    pitch_db_path.display()
                ),
            }
        }

        let is_checking_update = Arc::new(AtomicBool::new(false));
        let is_syncing = Arc::new(AtomicBool::new(false));
        let sync_progress = Arc::new(Mutex::new(None));

        let is_checking_freq_update = Arc::new(AtomicBool::new(false));
        let is_freq_syncing = Arc::new(AtomicBool::new(false));
        let freq_sync_progress = Arc::new(Mutex::new(None));

        let is_pitch_importing = Arc::new(AtomicBool::new(false));
        let pitch_import_progress = Arc::new(Mutex::new(None));

        // If not installed, mark update_available as true immediately
        let update_available = installed_metadata.is_none();
        let freq_update_available = installed_freq_metadata.is_none();

        if spawn_remote_checks {
            // Trigger background check for remote version
            let current_rev = installed_metadata.as_ref().map(|m| m.revision.clone());
            sync::spawn_check_version(current_rev, is_checking_update.clone(), tx.clone());

            let current_freq_rev = installed_freq_metadata.as_ref().map(|m| m.revision.clone());
            sync::spawn_check_freq_version(
                current_freq_rev,
                is_checking_freq_update.clone(),
                tx.clone(),
            );
        }

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
            pitch_db: Arc::new(Mutex::new(pitch_db_handle)),
            installed_pitch_metadata,
            is_pitch_importing,
            pitch_import_progress,
            dict_dir,
            event_tx: tx,
            event_rx: Some(rx),
            popup: None,
            popup_under_crt: false,
        }
    }

    /// Reopens the primary Jitendex database from `dict_dir`, validates its metadata, swaps the
    /// active handle, and invalidates the lookup scan cache. Returns `Err` without overwriting
    /// existing state if opening or reading metadata fails.
    pub fn reload_dict_db(
        &mut self,
        fallback_meta: Option<DictMetadata>,
    ) -> anyhow::Result<DictMetadata> {
        let db_path = self.dict_dir.join("jitendex.db");
        let db = DictDatabase::open(&db_path)?;
        let db_meta = db.get_metadata()?;
        if db_meta.total_entries == 0 {
            anyhow::bail!(
                "Refusing to activate empty Jitendex database at {}",
                db_path.display()
            );
        }
        let final_meta = fallback_meta.unwrap_or(db_meta);
        if let Ok(mut guard) = self.db.lock() {
            *guard = Some(db);
        }
        self.installed_metadata = Some(final_meta.clone());
        self.update_available = false;
        lookup::invalidate_scan_cache();
        Ok(final_meta)
    }

    /// Reopens the Jiten frequency database from `dict_dir`, validates its metadata, swaps the
    /// active handle, and invalidates the lookup scan cache. Returns `Err` without overwriting
    /// existing state if opening or reading metadata fails.
    pub fn reload_freq_db(
        &mut self,
        fallback_meta: Option<DictMetadata>,
    ) -> anyhow::Result<DictMetadata> {
        let db_path = self.dict_dir.join("jiten_freq.db");
        let db = FreqDatabase::open(&db_path)?;
        let db_meta = db.get_metadata()?;
        if db_meta.total_entries == 0 {
            anyhow::bail!(
                "Refusing to activate empty frequency database at {}",
                db_path.display()
            );
        }
        let final_meta = fallback_meta.unwrap_or(db_meta);
        if let Ok(mut guard) = self.freq_db.lock() {
            *guard = Some(db);
        }
        self.installed_freq_metadata = Some(final_meta.clone());
        self.freq_update_available = false;
        lookup::invalidate_scan_cache();
        Ok(final_meta)
    }

    /// Reopens the pitch accent database from `dict_dir`, validates its metadata, swaps the
    /// active handle, and invalidates the lookup scan cache. Returns `Err` without overwriting
    /// existing state if opening or reading metadata fails.
    pub fn reload_pitch_db(
        &mut self,
        fallback_meta: Option<DictMetadata>,
    ) -> anyhow::Result<DictMetadata> {
        let db_path = self.dict_dir.join("pitch.db");
        let db = PitchDatabase::open(&db_path)?;
        let db_meta = db.get_metadata()?;
        if db_meta.total_entries == 0 {
            anyhow::bail!(
                "Refusing to activate empty pitch database at {}",
                db_path.display()
            );
        }
        let final_meta = fallback_meta.unwrap_or(db_meta);
        if let Ok(mut guard) = self.pitch_db.lock() {
            *guard = Some(db);
        }
        self.installed_pitch_metadata = Some(final_meta.clone());
        lookup::invalidate_scan_cache();
        Ok(final_meta)
    }

    /// Triggers local import of a Yomitan pitch accent dictionary `.zip` file.
    pub fn trigger_pitch_import(&mut self, zip_path: PathBuf) {
        if self.is_pitch_importing.load(Ordering::Relaxed) {
            return;
        }

        pitch::spawn_pitch_import(
            zip_path,
            self.dict_dir.clone(),
            self.is_pitch_importing.clone(),
            self.event_tx.clone(),
        );
    }

    /// Removes the installed pitch accent dictionary.
    pub fn remove_pitch_dict(&mut self) -> anyhow::Result<()> {
        if let Ok(mut guard) = self.pitch_db.lock() {
            *guard = None;
        }
        self.installed_pitch_metadata = None;
        lookup::invalidate_scan_cache();
        let pitch_db_path = self.dict_dir.join("pitch.db");
        if pitch_db_path.exists() {
            std::fs::remove_file(&pitch_db_path)?;
        }
        Ok(())
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
        let current_rev = self
            .installed_freq_metadata
            .as_ref()
            .map(|m| m.revision.clone());
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
        PathBuf::from(home)
            .join(".config")
            .join("michadame")
            .join("dict")
    } else {
        PathBuf::from("./dict")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dict_state_new_in_tests_is_offline_and_isolated() {
        let state = DictState::new();
        assert!(
            !state.is_checking_update.load(Ordering::Relaxed),
            "DictState::new() under test must not spawn remote dictionary version check"
        );
        assert!(
            !state.is_checking_freq_update.load(Ordering::Relaxed),
            "DictState::new() under test must not spawn remote frequency version check"
        );
        assert!(
            state.dict_dir.starts_with(std::env::temp_dir()),
            "DictState::new() under test must use an isolated temp directory"
        );
    }

    #[test]
    fn test_reload_freq_db_validates_before_replacing_active_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = DictState::with_dir(dir.path().to_path_buf(), false);

        // Reloading when jiten_freq.db does not exist must fail and not mark installed
        assert!(state.reload_freq_db(None).is_err());
        assert!(state.installed_freq_metadata.is_none());
        assert!(state.freq_update_available);

        // Seed a valid frequency DB
        let freq_db_path = dir.path().join("jiten_freq.db");
        {
            let db = FreqDatabase::open_or_create(&freq_db_path).unwrap();
            db.conn
                .execute_batch(
                    "
                    INSERT INTO metadata (key, value) VALUES ('title', 'Jiten v1'), ('revision', 'rev1');
                    INSERT INTO frequencies (term, reading, rank, display_value) VALUES ('私', 'わたし', 23, '23㋕');
                    ",
                )
                .unwrap();
        }

        let meta1 = state.reload_freq_db(None).unwrap();
        assert_eq!(meta1.title, "Jiten v1");
        assert_eq!(meta1.revision, "rev1");
        assert_eq!(meta1.total_entries, 1);
        assert!(!state.freq_update_available);
        let gen1 = state.freq_db.lock().unwrap().as_ref().unwrap().generation;

        // Corrupt / remove the file on disk and verify reload_freq_db fails without wiping active handle
        std::fs::remove_file(&freq_db_path).unwrap();
        assert!(state.reload_freq_db(None).is_err());
        assert_eq!(
            state
                .installed_freq_metadata
                .as_ref()
                .map(|m| m.revision.as_str()),
            Some("rev1")
        );
        assert_eq!(
            state.freq_db.lock().unwrap().as_ref().unwrap().generation,
            gen1
        );
    }
}
