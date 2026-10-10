//! Mining bank: a persistent collection of words mined from the dictionary popup, each with
//! its definition, source sentence and a screenshot of the rendered video surface.

pub mod database;
pub mod sentence;
pub mod tags;

pub use database::{BankDatabase, BankEntryMeta, CustomNameEntry, NewBankEntry, NewCustomName};

use crate::dict::TermEntry;
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Maximum dimensions of list thumbnails.
const THUMB_MAX: (u32, u32) = (320, 180);
/// If the video paint callback hasn't picked up a mining request within this time (e.g. no
/// video is being rendered), the entry is saved without a screenshot.
const CAPTURE_TIMEOUT: Duration = Duration::from_millis(1500);

/// Identifies a mined word in a specific sentence, used to avoid duplicates and to show the
/// "already mined" state in the dictionary popup: (term, reading, sentence).
pub type MineKey = (String, String, String);

/// A request to mine a word, waiting for its screenshot to be captured.
#[derive(Clone, Debug)]
pub struct MineRequest {
    pub created_at: i64,
    pub term: String,
    pub reading: String,
    pub definition_text: String,
    pub definition_json: String,
    pub sentence: String,
    pub word_range: (usize, usize),
    pub source_text: String,
    /// Tag to store with the entry (already normalized/canonicalized).
    pub tag: Option<String>,
    pub requested_at: Instant,
}

impl MineRequest {
    /// Builds a request for `entry`, matched at `char_range` within the OCR box `source_text`.
    pub fn from_lookup(
        entry: &TermEntry,
        source_text: &str,
        char_range: (usize, usize),
        tag: Option<String>,
    ) -> Self {
        let (sentence, word_range) = sentence::extract_sentence(source_text, char_range);
        Self {
            created_at: unix_millis(),
            term: entry.term.clone(),
            reading: entry.reading.clone(),
            definition_text: crate::dict::render::glossary_plain_text(entry),
            definition_json: entry_to_json(entry),
            sentence,
            word_range,
            source_text: source_text.to_string(),
            tag,
            requested_at: Instant::now(),
        }
    }

    pub fn key(&self) -> MineKey {
        (self.term.clone(), self.reading.clone(), self.sentence.clone())
    }
}

/// Recursively removes Jitendex example-sentence nodes from structured glossary content.
pub fn strip_example_sentences(value: &mut serde_json::Value) {
    fn is_example(v: &serde_json::Value) -> bool {
        v.get("data")
            .and_then(|d| d.get("content"))
            .and_then(|c| c.as_str())
            == Some("example-sentence")
    }
    match value {
        serde_json::Value::Array(items) => {
            items.retain(|item| !is_example(item));
            items.iter_mut().for_each(strip_example_sentences);
        }
        serde_json::Value::Object(map) => {
            if let Some(content) = map.get_mut("content") {
                if is_example(content) {
                    *content = serde_json::Value::Array(Vec::new());
                } else {
                    strip_example_sentences(content);
                }
            }
        }
        _ => {}
    }
}

/// Serializes the dictionary entry as shown in the popup (headword, reading, tags,
/// frequency, deinflection trail and senses), without example sentences.
fn entry_to_json(entry: &TermEntry) -> String {
    let glossary: Vec<serde_json::Value> = entry
        .glossary
        .iter()
        .map(|g| match g {
            crate::dict::GlossaryEntry::Text(s) => serde_json::Value::String(s.clone()),
            crate::dict::GlossaryEntry::Structured(v) => {
                let mut v = v.clone();
                strip_example_sentences(&mut v);
                v
            }
        })
        .collect();
    serde_json::json!({
        "term": entry.term,
        "reading": entry.reading,
        "definition_tags": entry.definition_tags,
        "rules": entry.rules,
        "score": entry.score,
        "sequence": entry.sequence,
        "term_tags": entry.term_tags,
        "inflection_reasons": entry.inflection_reasons,
        "frequency": entry.frequency.as_ref().map(|f| serde_json::json!({
            "dictionary": f.dictionary,
            "rank": f.rank,
            "display_value": f.display_value,
        })),
        "glossary": glossary,
    })
    .to_string()
}

/// Extracts the sequence number from a stored `definition_json`, or 0 if missing.
pub fn extract_sequence_from_json(json: &str) -> i64 {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| v.get("sequence").and_then(|s| s.as_i64()))
        .unwrap_or(0)
}

/// Reconstructs a renderable dictionary entry from a stored `definition_json`.
/// Missing fields (e.g. entries mined by older versions) fall back to defaults.
pub fn entry_from_json(term: &str, reading: &str, json: &str) -> Option<TermEntry> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let str_field = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let glossary = v
        .get("glossary")
        .and_then(|g| g.as_array())
        .map(|items| {
            items
                .iter()
                .map(|item| match item {
                    serde_json::Value::String(s) => crate::dict::GlossaryEntry::Text(s.clone()),
                    other => {
                        let mut other = other.clone();
                        strip_example_sentences(&mut other);
                        crate::dict::GlossaryEntry::Structured(other)
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let frequency = v.get("frequency").filter(|f| !f.is_null()).and_then(|f| {
        Some(crate::dict::TermFrequency {
            dictionary: f.get("dictionary")?.as_str()?.to_string(),
            rank: f.get("rank")?.as_i64()?,
            display_value: f.get("display_value").and_then(|d| d.as_str()).map(str::to_string),
        })
    });
    Some(TermEntry {
        term: str_field("term").unwrap_or_else(|| term.to_string()),
        reading: str_field("reading").unwrap_or_else(|| reading.to_string()),
        definition_tags: str_field("definition_tags"),
        rules: str_field("rules").unwrap_or_default(),
        score: v.get("score").and_then(|x| x.as_f64()).unwrap_or(0.0),
        glossary,
        sequence: v.get("sequence").and_then(|x| x.as_i64()).unwrap_or(0),
        term_tags: str_field("term_tags"),
        inflection_reasons: v
            .get("inflection_reasons")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|r| r.as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
        frequency,
        custom_name_id: None,
        custom_tag: None,
    })
}

/// Result of a background save, delivered to the UI thread.
pub enum BankEvent {
    Saved(BankEntryMeta),
    Failed { key: MineKey, error: String },
    ClipboardCopied,
    ClipboardFailed(String),
}

/// Cloneable handle passed into the video paint callback, which captures the screenshot
/// right after the video (with shaders) is drawn and before any egui overlay is painted.
#[derive(Clone)]
pub struct BankCaptureHandle {
    pending: Arc<Mutex<Option<MineRequest>>>,
    db: Arc<Mutex<Option<BankDatabase>>>,
    tx: Sender<BankEvent>,
}

impl BankCaptureHandle {
    /// Takes the pending mining request, if any.
    pub fn take_pending(&self) -> Option<MineRequest> {
        self.pending.lock().ok()?.take()
    }

    /// Saves `request` in a background thread. `shot` is the raw RGBA rendered area
    /// (top-left origin) as `(pixels, width, height)`.
    pub fn save(&self, request: MineRequest, shot: Option<(Vec<u8>, u32, u32)>) {
        let db = self.db.clone();
        let tx = self.tx.clone();
        let _ = std::thread::Builder::new()
            .name("mining-bank-save".into())
            .spawn(move || {
                let key = request.key();
                let event = match save_entry(&db, request, shot) {
                    Ok(meta) => BankEvent::Saved(meta),
                    Err(e) => BankEvent::Failed { key, error: e.to_string() },
                };
                let _ = tx.send(event);
            });
    }
}

fn save_entry(
    db: &Mutex<Option<BankDatabase>>,
    request: MineRequest,
    shot: Option<(Vec<u8>, u32, u32)>,
) -> anyhow::Result<BankEntryMeta> {
    let (screenshot, thumbnail) = match shot {
        Some((pixels, w, h)) => match encode_screenshot(pixels, w, h) {
            Ok((s, t)) => (Some(s), Some(t)),
            Err(e) => {
                tracing::warn!("Mining bank: failed to encode screenshot: {e}");
                (None, None)
            }
        },
        None => (None, None),
    };
    let entry = NewBankEntry {
        created_at: request.created_at,
        term: request.term,
        reading: request.reading,
        definition_text: request.definition_text,
        definition_json: request.definition_json,
        sentence: request.sentence,
        word_range: request.word_range,
        source_text: request.source_text,
        screenshot,
        thumbnail,
        tag: request.tag,
    };
    let guard = db.lock().map_err(|_| anyhow::anyhow!("bank database lock poisoned"))?;
    let db = guard
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("mining bank database is not available"))?;
    db.insert(&entry)
}

/// Scales `max` to fit inside `bound` while preserving aspect ratio (never upscales).
fn fit_within(size: (u32, u32), bound: (u32, u32)) -> (u32, u32) {
    if size.0 == 0 || size.1 == 0 {
        return size;
    }
    let scale = (bound.0 as f32 / size.0 as f32)
        .min(bound.1 as f32 / size.1 as f32)
        .min(1.0);
    (
        ((size.0 as f32 * scale).round() as u32).max(1),
        ((size.1 as f32 * scale).round() as u32).max(1),
    )
}

fn encode_jpeg(img: &image::RgbImage, quality: u8) -> anyhow::Result<Vec<u8>> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(out)
}

/// Downscales the raw rendered-area capture to at most 720p (or the equivalent for its
/// aspect ratio) and encodes it plus a small thumbnail as JPEG.
pub fn encode_screenshot(pixels: Vec<u8>, w: u32, h: u32) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    use image::imageops::{resize, FilterType};
    let rgba = image::RgbaImage::from_raw(w, h, pixels)
        .ok_or_else(|| anyhow::anyhow!("invalid screenshot buffer ({w}x{h})"))?;
    let rgb = image::DynamicImage::ImageRgba8(rgba).to_rgb8();

    let (tw, th) = crate::ocr::lens::calculate_720p_target_dimensions(w, h);
    let full = if (tw, th) != (w, h) {
        resize(&rgb, tw, th, FilterType::CatmullRom)
    } else {
        rgb
    };
    let (thumb_w, thumb_h) = fit_within((tw, th), THUMB_MAX);
    let thumb = resize(&full, thumb_w, thumb_h, FilterType::Triangle);

    Ok((encode_jpeg(&full, 90)?, encode_jpeg(&thumb, 85)?))
}

/// Decodes an encoded image into an egui texture.
pub fn load_texture(ctx: &egui::Context, name: String, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture(name, color, egui::TextureOptions::LINEAR))
}

pub struct BankState {
    pub db: Arc<Mutex<Option<BankDatabase>>>,
    /// All entries, most recent first.
    pub entries: Vec<BankEntryMeta>,
    mined_keys: HashSet<MineKey>,
    mined_definitions: HashSet<(String, String, i64)>,
    mined_terms: HashSet<(String, String)>,
    in_flight: HashSet<MineKey>,
    pending: Arc<Mutex<Option<MineRequest>>>,
    event_tx: Sender<BankEvent>,
    event_rx: Receiver<BankEvent>,
    /// Whether the bank window is open.
    pub window_open: bool,
    /// Cached thumbnails by entry id (`None` = no thumbnail available).
    pub thumbnails: HashMap<i64, Option<egui::TextureHandle>>,
    /// Parsed dictionary entries by entry id (`None` = could not be parsed).
    dict_entries: HashMap<i64, Option<Arc<TermEntry>>>,
    /// Currently enlarged screenshot.
    pub enlarged: Option<(i64, egui::TextureHandle)>,
    /// Entry awaiting delete confirmation.
    pub pending_delete: Option<i64>,
    /// Error opening the database, if any.
    pub load_error: Option<String>,
    /// Tag applied to newly mined words (set in the settings panel, persisted in the config).
    pub current_tag: String,
    /// Last `current_tag` value written to the config, to avoid redundant saves.
    pub saved_current_tag: String,
    /// Tag filter typed at the top of the bank window (empty = show everything).
    pub filter_tag: String,
    /// Distinct tags in use, most recently used first. Used for suggestions.
    pub known_tags: Vec<String>,
    /// Inline tag editor state for a bank row.
    pub editing_tag: Option<TagEdit>,
    /// Entry currently having its screenshot copied to the clipboard.
    pub copying_screenshot: Option<i64>,
    /// Whether compact view is active in the bank window.
    pub compact_mode: bool,
    /// Mined word entry IDs expanded while in compact mode.
    pub expanded_entries: HashSet<i64>,
    /// Custom name dictionary entries, most recent first.
    pub custom_names: Vec<CustomNameEntry>,
    /// Generation counter incremented when custom names are added or deleted (for cache invalidation).
    pub custom_names_generation: u64,
    /// Modal dialog state for adding a custom name in the word bank window.
    pub add_name_dialog: AddNameDialogState,
}

/// Modal dialog state for adding a custom name in the word bank window.
#[derive(Clone, Debug, Default)]
pub struct AddNameDialogState {
    pub open: bool,
    pub kanji: String,
    pub furigana: String,
    pub source_tag: String,
    pub notes: String,
    pub focus: bool,
    pub error_msg: Option<String>,
}

/// In-progress edit of a bank entry's tag.
#[derive(Clone, Debug)]
pub struct TagEdit {
    pub id: i64,
    pub text: String,
    /// Whether keyboard focus has already been requested for the editor.
    pub focused: bool,
}

/// Popup button state for a dictionary entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MineStatus {
    Available,
    Pending,
    Mined,
}

impl BankState {
    pub fn new() -> Self {
        #[cfg(test)]
        let opened = BankDatabase::open_in_memory();
        #[cfg(not(test))]
        let opened = BankDatabase::open(resolve_bank_directory().join("bank.db"));
        match opened {
            Ok(db) => Self::with_database(Some(db), None),
            Err(e) => {
                tracing::warn!("Failed to open mining bank database: {e:#}");
                Self::with_database(None, Some(format!("{e:#}")))
            }
        }
    }

    fn with_database(db: Option<BankDatabase>, load_error: Option<String>) -> Self {
        let entries = db
            .as_ref()
            .and_then(|db| db.list_meta().map_err(|e| tracing::warn!("Failed to list bank: {e}")).ok())
            .unwrap_or_default();
        let custom_names = db
            .as_ref()
            .and_then(|db| db.list_custom_names().map_err(|e| tracing::warn!("Failed to list custom names: {e}")).ok())
            .unwrap_or_default();
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let mut state = Self {
            db: Arc::new(Mutex::new(db)),
            entries,
            mined_keys: HashSet::new(),
            mined_definitions: HashSet::new(),
            mined_terms: HashSet::new(),
            in_flight: HashSet::new(),
            pending: Arc::new(Mutex::new(None)),
            event_tx,
            event_rx,
            window_open: false,
            thumbnails: HashMap::new(),
            dict_entries: HashMap::new(),
            enlarged: None,
            pending_delete: None,
            load_error,
            current_tag: String::new(),
            saved_current_tag: String::new(),
            filter_tag: String::new(),
            known_tags: Vec::new(),
            editing_tag: None,
            copying_screenshot: None,
            compact_mode: false,
            expanded_entries: HashSet::new(),
            custom_names,
            custom_names_generation: 1,
            add_name_dialog: AddNameDialogState::default(),
        };
        state.rebuild_keys();
        state
    }

    fn rebuild_keys(&mut self) {
        self.mined_keys = self
            .entries
            .iter()
            .map(|e| (e.term.clone(), e.reading.clone(), e.sentence.clone()))
            .collect();
        self.mined_definitions = self
            .entries
            .iter()
            .map(|e| (e.term.clone(), e.reading.clone(), extract_sequence_from_json(&e.definition_json)))
            .collect();
        self.mined_terms = self
            .entries
            .iter()
            .map(|e| (e.term.clone(), e.reading.clone()))
            .collect();
        self.rebuild_known_tags();
    }

    /// Recomputes the distinct tags in use, ordered by most recent use (entries are kept
    /// most-recent-first).
    fn rebuild_known_tags(&mut self) {
        let mut seen = HashSet::new();
        let mut tags = Vec::new();
        for entry in &self.entries {
            if let Some(t) = &entry.tag {
                if seen.insert(t.as_str()) {
                    tags.push(t.clone());
                }
            }
        }
        for name in &self.custom_names {
            if let Some(t) = &name.source_tag {
                if seen.insert(t.as_str()) {
                    tags.push(t.clone());
                }
            }
        }
        self.known_tags = tags;
    }

    /// Opens the Add Custom Name dialog in the Word Bank window.
    pub fn open_add_name_dialog(&mut self) {
        self.add_name_dialog = AddNameDialogState {
            open: true,
            kanji: String::new(),
            furigana: String::new(),
            source_tag: self.mining_tag().unwrap_or_else(|| self.current_tag.clone()),
            notes: String::new(),
            focus: true,
            error_msg: None,
        };
    }

    /// Adds a new custom name entry.
    pub fn add_custom_name(
        &mut self,
        kanji: &str,
        furigana: &str,
        source_tag: Option<&str>,
        notes: Option<&str>,
    ) -> anyhow::Result<CustomNameEntry> {
        let tag = source_tag.and_then(|t| tags::canonicalize_tag(t, &self.known_tags));
        let new_entry = NewCustomName {
            created_at: unix_millis(),
            kanji: kanji.trim().to_string(),
            furigana: furigana.trim().to_string(),
            source_tag: tag,
            notes: notes.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
        };
        let entry = {
            let guard = self.db.lock().map_err(|_| anyhow::anyhow!("bank database lock poisoned"))?;
            let db = guard
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("mining bank database is not available"))?;
            db.insert_custom_name(&new_entry)?
        };
        self.custom_names.insert(0, entry.clone());
        self.custom_names_generation = self.custom_names_generation.wrapping_add(1);
        self.rebuild_known_tags();
        Ok(entry)
    }

    /// Deletes a custom name entry by id.
    pub fn delete_custom_name(&mut self, id: i64) -> anyhow::Result<()> {
        {
            let guard = self.db.lock().map_err(|_| anyhow::anyhow!("lock poisoned"))?;
            if let Some(db) = guard.as_ref() {
                db.delete_custom_name(id)?;
            }
        }
        self.custom_names.retain(|e| e.id != id);
        self.custom_names_generation = self.custom_names_generation.wrapping_add(1);
        self.rebuild_known_tags();
        Ok(())
    }

    /// The tag to stamp on a newly mined word: the settings tag, trimmed and matched to an
    /// existing tag's capitalization if one exists.
    pub fn mining_tag(&self) -> Option<String> {
        tags::canonicalize_tag(&self.current_tag, &self.known_tags)
    }

    /// Indices into `entries` of the words matching the bank-window tag filter, most
    /// recent first.
    pub fn visible_indices(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| tags::tag_matches_filter(e.tag.as_deref(), &self.filter_tag))
            .map(|(i, _)| i)
            .collect()
    }

    /// Sets (or removes, if empty) the tag of an entry. The tag is trimmed and matched to an
    /// existing tag's capitalization. Returns the stored tag.
    pub fn set_tag(&mut self, id: i64, tag: &str) -> anyhow::Result<Option<String>> {
        let tag = tags::canonicalize_tag(tag, &self.known_tags);
        if let Some(db) = self.db.lock().map_err(|_| anyhow::anyhow!("lock poisoned"))?.as_ref() {
            db.set_tag(id, tag.as_deref())?;
        }
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.tag = tag.clone();
        }
        self.rebuild_known_tags();
        Ok(tag)
    }

    pub fn capture_handle(&self) -> BankCaptureHandle {
        BankCaptureHandle {
            pending: self.pending.clone(),
            db: self.db.clone(),
            tx: self.event_tx.clone(),
        }
    }

    pub fn status(&self, term: &str, reading: &str, sentence: &str) -> MineStatus {
        let key = (term.to_string(), reading.to_string(), sentence.to_string());
        if self.mined_keys.contains(&key) {
            MineStatus::Mined
        } else if self.in_flight.contains(&key) {
            MineStatus::Pending
        } else {
            MineStatus::Available
        }
    }

    /// Returns true if this specific dictionary entry is already in the mined bank.
    pub fn is_entry_mined(&self, term: &str, reading: &str, sequence: i64) -> bool {
        if sequence != 0
            && self
                .mined_definitions
                .contains(&(term.to_string(), reading.to_string(), sequence))
        {
            return true;
        }
        if sequence == 0 && self.mined_terms.contains(&(term.to_string(), reading.to_string())) {
            return true;
        }
        // Fallback: match if bank contains (term, reading, 0)
        if sequence != 0
            && self
                .mined_definitions
                .contains(&(term.to_string(), reading.to_string(), 0))
        {
            return true;
        }
        // Also check if any in-flight requests match
        if self.in_flight.iter().any(|(t, r, _)| t == term && r == reading) {
            return true;
        }
        false
    }

    /// Queues a mining request. The screenshot is captured by the video paint callback at
    /// the end of the current frame. Returns false if it was rejected (duplicate or busy).
    pub fn request_mine(&mut self, request: MineRequest) -> bool {
        let key = request.key();
        if self.mined_keys.contains(&key) || self.in_flight.contains(&key) {
            return false;
        }
        let Ok(mut pending) = self.pending.lock() else {
            return false;
        };
        if pending.is_some() {
            return false;
        }
        *pending = Some(request);
        self.in_flight.insert(key);
        true
    }

    /// Processes finished saves and capture timeouts. Returns toast messages:
    /// `Ok(info)` or `Err(error)`.
    pub fn poll(&mut self) -> Vec<Result<String, String>> {
        // Fallback: save without a screenshot if the paint callback never ran.
        let timed_out = self.pending.lock().ok().and_then(|mut p| {
            if p.as_ref().map_or(false, |r| r.requested_at.elapsed() > CAPTURE_TIMEOUT) {
                p.take()
            } else {
                None
            }
        });
        if let Some(request) = timed_out {
            tracing::warn!("Mining bank: no video frame rendered, saving without screenshot");
            self.capture_handle().save(request, None);
        }

        let mut messages = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                BankEvent::Saved(meta) => {
                    let key = (meta.term.clone(), meta.reading.clone(), meta.sentence.clone());
                    self.in_flight.remove(&key);
                    self.mined_keys.insert(key);
                    let seq = extract_sequence_from_json(&meta.definition_json);
                    self.mined_definitions
                        .insert((meta.term.clone(), meta.reading.clone(), seq));
                    self.mined_terms
                        .insert((meta.term.clone(), meta.reading.clone()));
                    messages.push(Ok(format!("Mined: {}", meta.term)));
                    let pos = self
                        .entries
                        .iter()
                        .position(|e| e.created_at <= meta.created_at)
                        .unwrap_or(self.entries.len());
                    self.entries.insert(pos, meta);
                    self.rebuild_known_tags();
                }
                BankEvent::Failed { key, error } => {
                    self.in_flight.remove(&key);
                    messages.push(Err(format!("Failed to mine word: {error}")));
                }
                BankEvent::ClipboardCopied => {
                    self.copying_screenshot = None;
                }
                BankEvent::ClipboardFailed(error) => {
                    self.copying_screenshot = None;
                    messages.push(Err(format!("Failed to copy screenshot: {error}")));
                }
            }
        }
        messages
    }

    /// True while a mining request is waiting for a capture or being saved, or copying a screenshot.
    pub fn is_busy(&self) -> bool {
        !self.in_flight.is_empty() || self.copying_screenshot.is_some()
    }

    pub fn delete(&mut self, id: i64) -> anyhow::Result<()> {
        if let Some(db) = self.db.lock().map_err(|_| anyhow::anyhow!("lock poisoned"))?.as_ref() {
            db.delete(id)?;
        }
        self.entries.retain(|e| e.id != id);
        self.thumbnails.remove(&id);
        self.dict_entries.remove(&id);
        if self.enlarged.as_ref().map_or(false, |(eid, _)| *eid == id) {
            self.enlarged = None;
        }
        if self.copying_screenshot == Some(id) {
            self.copying_screenshot = None;
        }
        self.expanded_entries.remove(&id);
        self.rebuild_keys();
        Ok(())
    }

    /// Toggles the expanded state of an entry in compact mode.
    pub fn toggle_entry_expanded(&mut self, id: i64) {
        if !self.expanded_entries.remove(&id) {
            self.expanded_entries.insert(id);
        }
    }

    /// Returns the (cached) renderable dictionary entry stored with a bank entry.
    pub fn dict_entry(&mut self, entry: &BankEntryMeta) -> Option<Arc<TermEntry>> {
        self.dict_entries
            .entry(entry.id)
            .or_insert_with(|| {
                entry_from_json(&entry.term, &entry.reading, &entry.definition_json).map(Arc::new)
            })
            .clone()
    }

    /// Loads (and caches) the thumbnail texture for an entry.
    pub fn thumbnail(&mut self, ctx: &egui::Context, entry: &BankEntryMeta) -> Option<egui::TextureHandle> {
        if let Some(cached) = self.thumbnails.get(&entry.id) {
            return cached.clone();
        }
        let texture = if entry.has_screenshot {
            let bytes = self
                .db
                .lock()
                .ok()
                .and_then(|g| g.as_ref().and_then(|db| db.thumbnail(entry.id).ok().flatten()));
            bytes.and_then(|b| load_texture(ctx, format!("bank_thumb_{}", entry.id), &b))
        } else {
            None
        };
        self.thumbnails.insert(entry.id, texture.clone());
        texture
    }

    /// Loads the full screenshot of an entry and shows it enlarged.
    pub fn enlarge(&mut self, ctx: &egui::Context, id: i64) {
        let bytes = self
            .db
            .lock()
            .ok()
            .and_then(|g| g.as_ref().and_then(|db| db.screenshot(id).ok().flatten()));
        self.enlarged = bytes
            .and_then(|b| load_texture(ctx, format!("bank_full_{id}"), &b))
            .map(|t| (id, t));
    }

    /// Returns the raw JPEG screenshot bytes for an entry, if available.
    pub fn screenshot_bytes(&self, id: i64) -> Option<Vec<u8>> {
        self.db
            .lock()
            .ok()
            .and_then(|g| g.as_ref().and_then(|db| db.screenshot(id).ok().flatten()))
    }

    /// Clone of the event sender for background tasks.
    pub fn event_sender(&self) -> Sender<BankEvent> {
        self.event_tx.clone()
    }
}

impl Default for BankState {
    fn default() -> Self {
        Self::new()
    }
}

/// Directory where the bank database is stored (~/.config/michadame/bank).
pub fn resolve_bank_directory() -> PathBuf {
    crate::dict::resolve_dict_directory()
        .parent()
        .map(|p| p.join("bank"))
        .unwrap_or_else(|| PathBuf::from("./bank"))
}

fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Formats a unix-millisecond timestamp in local time as `YYYY-MM-DD HH:MM`.
pub fn format_timestamp(ms: i64) -> String {
    let secs = (ms / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&secs, &mut tm).is_null() };
    if !ok {
        return String::new();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_is_capped_to_720p_with_small_thumbnail() {
        let (w, h) = (1920u32, 1080u32);
        let pixels = vec![128u8; (w * h * 4) as usize];
        let (full, thumb) = encode_screenshot(pixels, w, h).unwrap();
        let full = image::load_from_memory(&full).unwrap();
        assert_eq!((full.width(), full.height()), (1280, 720));
        let thumb = image::load_from_memory(&thumb).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (320, 180));
    }

    #[test]
    fn small_screenshots_are_not_upscaled() {
        let (w, h) = (640u32, 480u32);
        let (full, thumb) = encode_screenshot(vec![0u8; (w * h * 4) as usize], w, h).unwrap();
        let full = image::load_from_memory(&full).unwrap();
        assert_eq!((full.width(), full.height()), (640, 480));
        let thumb = image::load_from_memory(&thumb).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (240, 180));
    }

    #[test]
    fn entry_json_round_trip_keeps_tags_and_frequency_but_drops_examples() {
        let sense = serde_json::json!({
            "type": "structured-content",
            "content": [{
                "tag": "li",
                "data": { "content": "sense" },
                "content": [
                    { "tag": "ul", "data": { "content": "glossary" }, "content": [{ "tag": "li", "content": "hot" }] },
                    { "tag": "div", "data": { "content": "extra-info" }, "content": {
                        "tag": "div", "data": { "content": "example-sentence" }, "content": "今日は暑い。"
                    }},
                    { "tag": "div", "data": { "content": "example-sentence" }, "content": "暑いね。" }
                ]
            }]
        });
        let entry = TermEntry {
            term: "暑い".into(),
            reading: "あつい".into(),
            definition_tags: Some("adj-i".into()),
            rules: "adj-i".into(),
            score: 3.0,
            glossary: vec![crate::dict::GlossaryEntry::Structured(sense)],
            sequence: 42,
            term_tags: Some("P".into()),
            inflection_reasons: vec!["past".into()],
            frequency: Some(crate::dict::TermFrequency {
                dictionary: "Jiten".into(),
                rank: 1234,
                display_value: Some("1234㋕".into()),
            }),
            custom_name_id: None,
            custom_tag: None,
        };
        let json = entry_to_json(&entry);
        assert!(!json.contains("example-sentence"));
        assert!(json.contains("hot"));

        let parsed = entry_from_json("x", "y", &json).unwrap();
        assert_eq!(parsed.term, "暑い");
        assert_eq!(parsed.reading, "あつい");
        assert_eq!(parsed.definition_tags.as_deref(), Some("adj-i"));
        assert_eq!(parsed.inflection_reasons, vec!["past".to_string()]);
        assert_eq!(parsed.frequency, entry.frequency);
        assert_eq!(crate::dict::render::glossary_plain_text(&parsed), "hot");
    }

    #[test]
    fn entries_from_older_format_still_parse() {
        let old = r#"{"definition_tags":"n","term_tags":null,"inflection_reasons":[],"glossary":["cat"]}"#;
        let parsed = entry_from_json("猫", "ねこ", old).unwrap();
        assert_eq!(parsed.term, "猫");
        assert_eq!(parsed.reading, "ねこ");
        assert!(parsed.frequency.is_none());
        assert_eq!(crate::dict::render::glossary_plain_text(&parsed), "cat");
    }

    #[test]
    fn mining_flow_saves_and_marks_entry_as_mined() {
        let mut bank = BankState::with_database(Some(BankDatabase::open_in_memory().unwrap()), None);
        let entry = TermEntry {
            term: "暑い".into(),
            reading: "あつい".into(),
            definition_tags: None,
            rules: String::new(),
            score: 0.0,
            glossary: vec![crate::dict::GlossaryEntry::Text("hot".into())],
            sequence: 1,
            term_tags: None,
            inflection_reasons: vec![],
            frequency: None,
            custom_name_id: None,
            custom_tag: None,
        };
        let text = "おはよう。今日は暑いね。";
        let request = MineRequest::from_lookup(&entry, text, (8, 10), bank.mining_tag());
        assert_eq!(request.sentence, "今日は暑いね。");
        assert!(bank.request_mine(request.clone()));
        assert_eq!(bank.status("暑い", "あつい", "今日は暑いね。"), MineStatus::Pending);
        assert!(!bank.request_mine(request), "duplicates are rejected while in flight");

        let handle = bank.capture_handle();
        let req = handle.take_pending().unwrap();
        handle.save(req, Some((vec![255u8; 64 * 48 * 4], 64, 48)));

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut messages = Vec::new();
        while messages.is_empty() && Instant::now() < deadline {
            messages = bank.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(messages, vec![Ok("Mined: 暑い".to_string())]);
        assert_eq!(bank.entries.len(), 1);
        assert!(bank.entries[0].has_screenshot);
        assert_eq!(bank.entries[0].definition_text, "hot");
        assert_eq!(bank.status("暑い", "あつい", "今日は暑いね。"), MineStatus::Mined);
        assert_eq!(bank.entries[0].tag, None);

        let id = bank.entries[0].id;
        bank.delete(id).unwrap();
        assert!(bank.entries.is_empty());
        assert_eq!(bank.status("暑い", "あつい", "今日は暑いね。"), MineStatus::Available);
    }

    fn mine_and_wait(bank: &mut BankState, term: &str, sentence: &str) -> i64 {
        let entry = TermEntry {
            term: term.into(),
            reading: "よみ".into(),
            definition_tags: None,
            rules: String::new(),
            score: 0.0,
            glossary: vec![crate::dict::GlossaryEntry::Text("meaning".into())],
            sequence: 1,
            term_tags: None,
            inflection_reasons: vec![],
            frequency: None,
            custom_name_id: None,
            custom_tag: None,
        };
        let len = term.chars().count();
        let request = MineRequest::from_lookup(&entry, sentence, (0, len), bank.mining_tag());
        assert!(bank.request_mine(request));
        let handle = bank.capture_handle();
        handle.save(handle.take_pending().unwrap(), None);
        let deadline = Instant::now() + Duration::from_secs(10);
        while bank.is_busy() && Instant::now() < deadline {
            bank.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        bank.entries.iter().find(|e| e.term == term).unwrap().id
    }

    #[test]
    fn mined_words_get_current_tag_with_canonical_casing() {
        let mut bank = BankState::with_database(Some(BankDatabase::open_in_memory().unwrap()), None);
        bank.current_tag = "  Final Fantasy 7 ".into();
        let a = mine_and_wait(&mut bank, "猫", "猫だ。");
        assert_eq!(bank.known_tags, vec!["Final Fantasy 7"]);

        // A differently-capitalized tag reuses the existing spelling.
        bank.current_tag = "final fantasy 7".into();
        let b = mine_and_wait(&mut bank, "犬", "犬だ。");
        // A brand new tag is stored as typed.
        bank.current_tag = "final fantasy 8".into();
        let c = mine_and_wait(&mut bank, "鳥", "鳥だ。");
        bank.current_tag.clear();
        let d = mine_and_wait(&mut bank, "魚", "魚だ。");

        let tag_of = |bank: &BankState, id| bank.entries.iter().find(|e| e.id == id).unwrap().tag.clone();
        assert_eq!(tag_of(&bank, a).as_deref(), Some("Final Fantasy 7"));
        assert_eq!(tag_of(&bank, b).as_deref(), Some("Final Fantasy 7"));
        assert_eq!(tag_of(&bank, c).as_deref(), Some("final fantasy 8"));
        assert_eq!(tag_of(&bank, d), None);
        assert_eq!(bank.known_tags, vec!["final fantasy 8", "Final Fantasy 7"]);

        // Filtering by a partial tag shows matching words, most recent first.
        bank.filter_tag = "Final Fantasy".into();
        let visible: Vec<i64> = bank.visible_indices().iter().map(|&i| bank.entries[i].id).collect();
        assert_eq!(visible, vec![c, b, a]);
        bank.filter_tag = "fantasy 8".into();
        let visible: Vec<i64> = bank.visible_indices().iter().map(|&i| bank.entries[i].id).collect();
        assert_eq!(visible, vec![c]);
        bank.filter_tag.clear();
        assert_eq!(bank.visible_indices().len(), 4);

        // Editing tags: canonicalize, add, and remove.
        assert_eq!(bank.set_tag(d, "FINAL FANTASY 7").unwrap().as_deref(), Some("Final Fantasy 7"));
        assert_eq!(bank.set_tag(c, "  ").unwrap(), None);
        assert_eq!(tag_of(&bank, c), None);
        assert_eq!(bank.known_tags, vec!["Final Fantasy 7"]);
        let reloaded = bank.db.lock().unwrap().as_ref().unwrap().list_meta().unwrap();
        assert_eq!(reloaded.iter().find(|e| e.id == d).unwrap().tag.as_deref(), Some("Final Fantasy 7"));
    }

    #[test]
    fn is_entry_mined_identifies_specific_mined_definitions() {
        let mut bank = BankState::with_database(Some(BankDatabase::open_in_memory().unwrap()), None);
        let entry1 = TermEntry {
            term: "熱い".into(),
            reading: "あつい".into(),
            definition_tags: None,
            rules: String::new(),
            score: 0.0,
            glossary: vec![crate::dict::GlossaryEntry::Text("hot to touch".into())],
            sequence: 101,
            term_tags: None,
            inflection_reasons: vec![],
            frequency: None,
            custom_name_id: None,
            custom_tag: None,
        };
        let request = MineRequest::from_lookup(&entry1, "お茶が熱い。", (3, 5), bank.mining_tag());
        assert!(bank.request_mine(request));
        let handle = bank.capture_handle();
        handle.save(handle.take_pending().unwrap(), None);
        let deadline = Instant::now() + Duration::from_secs(10);
        while bank.is_busy() && Instant::now() < deadline {
            bank.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        let id = bank.entries.iter().find(|e| e.term == "熱い").unwrap().id;

        // Exact match with sequence 101
        assert!(bank.is_entry_mined("熱い", "あつい", 101));
        // Different sequence (e.g. another definition of the same word) does not match
        assert!(!bank.is_entry_mined("熱い", "あつい", 102));
        // Different headword does not match
        assert!(!bank.is_entry_mined("厚い", "あつい", 101));
        assert!(!bank.is_entry_mined("暑い", "あつい", 101));

        // When looked up in another sentence, status() for the sentence is Available, but is_entry_mined is true!
        assert_eq!(bank.status("熱い", "あつい", "別の文。"), MineStatus::Available);
        assert!(bank.is_entry_mined("熱い", "あつい", 101));

        // Delete entry clears is_entry_mined
        bank.delete(id).unwrap();
        assert!(!bank.is_entry_mined("熱い", "あつい", 101));

        // Test fallback when sequence is 0
        let entry_zero = TermEntry {
            term: "林檎".into(),
            reading: "りんご".into(),
            definition_tags: None,
            rules: String::new(),
            score: 0.0,
            glossary: vec![crate::dict::GlossaryEntry::Text("apple".into())],
            sequence: 0,
            term_tags: None,
            inflection_reasons: vec![],
            frequency: None,
            custom_name_id: None,
            custom_tag: None,
        };
        let req = MineRequest::from_lookup(&entry_zero, "林檎を食べた。", (0, 2), None);
        assert!(bank.request_mine(req));
        let handle = bank.capture_handle();
        handle.save(handle.take_pending().unwrap(), None);
        let deadline = Instant::now() + Duration::from_secs(10);
        while bank.is_busy() && Instant::now() < deadline {
            bank.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        // sequence 0 matches when sequence is 0
        assert!(bank.is_entry_mined("林檎", "りんご", 0));
        // sequence 0 in bank also matches if queried with non-zero sequence as fallback
        assert!(bank.is_entry_mined("林檎", "りんご", 999));
        // different reading does not match
        assert!(!bank.is_entry_mined("林檎", "みかん", 0));
    }

    #[test]
    fn test_clipboard_copy_events_and_screenshot_bytes() {
        let mut bank = BankState::new();
        let entry = TermEntry {
            term: "本".into(),
            reading: "ほん".into(),
            definition_tags: None,
            rules: String::new(),
            score: 0.0,
            glossary: vec![crate::dict::GlossaryEntry::Text("book".into())],
            sequence: 42,
            term_tags: None,
            inflection_reasons: vec![],
            frequency: None,
            custom_name_id: None,
            custom_tag: None,
        };
        let req = MineRequest::from_lookup(&entry, "本を読む。", (0, 1), None);
        assert!(bank.request_mine(req));
        let handle = bank.capture_handle();
        // Save with a fake screenshot (10x10 raw RGBA)
        let pixels = vec![255; 10 * 10 * 4];
        handle.save(handle.take_pending().unwrap(), Some((pixels, 10, 10)));

        let deadline = Instant::now() + Duration::from_secs(10);
        while bank.is_busy() && Instant::now() < deadline {
            bank.poll();
            std::thread::sleep(Duration::from_millis(5));
        }

        let mined = bank.entries.iter().find(|e| e.term == "本").unwrap();
        let id = mined.id;
        assert!(mined.has_screenshot);

        // Verify screenshot_bytes returns Some JPEG data
        let bytes = bank.screenshot_bytes(id).expect("screenshot should exist");
        assert!(!bytes.is_empty());

        // Verify copying_screenshot tracking and is_busy()
        bank.copying_screenshot = Some(id);
        assert!(bank.is_busy());

        // Test BankEvent::ClipboardCopied: silently clears copying_screenshot
        let tx = bank.event_sender();
        tx.send(BankEvent::ClipboardCopied).unwrap();
        let msgs = bank.poll();
        assert_eq!(bank.copying_screenshot, None);
        assert!(msgs.is_empty(), "success should produce no notification");
        assert!(!bank.is_busy());

        // Test BankEvent::ClipboardFailed: clears copying_screenshot and produces error toast
        bank.copying_screenshot = Some(id);
        tx.send(BankEvent::ClipboardFailed("test error".into())).unwrap();
        let msgs = bank.poll();
        assert_eq!(bank.copying_screenshot, None);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].is_err());
        assert_eq!(
            msgs[0].as_ref().unwrap_err(),
            "Failed to copy screenshot: test error"
        );

        // Test delete clears copying_screenshot if matching
        bank.copying_screenshot = Some(id);
        bank.delete(id).unwrap();
        assert_eq!(bank.copying_screenshot, None);
    }

    #[test]
    fn test_compact_mode_toggle_and_expansion() {
        let mut bank = BankState::new();
        assert!(!bank.compact_mode);
        assert!(bank.expanded_entries.is_empty());

        // Toggle entry expansion
        bank.toggle_entry_expanded(10);
        assert!(bank.expanded_entries.contains(&10));
        bank.toggle_entry_expanded(20);
        assert!(bank.expanded_entries.contains(&10));
        assert!(bank.expanded_entries.contains(&20));

        // Toggle again collapses
        bank.toggle_entry_expanded(10);
        assert!(!bank.expanded_entries.contains(&10));
        assert!(bank.expanded_entries.contains(&20));

        // Delete cleans up expanded_entries
        bank.delete(20).unwrap();
        assert!(!bank.expanded_entries.contains(&20));
    }
}
