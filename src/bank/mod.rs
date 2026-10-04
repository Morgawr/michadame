//! Mining bank: a persistent collection of words mined from the dictionary popup, each with
//! its definition, source sentence and a screenshot of the rendered video surface.

pub mod database;
pub mod sentence;

pub use database::{BankDatabase, BankEntryMeta, NewBankEntry};

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
    pub requested_at: Instant,
}

impl MineRequest {
    /// Builds a request for `entry`, matched at `char_range` within the OCR box `source_text`.
    pub fn from_lookup(entry: &TermEntry, source_text: &str, char_range: (usize, usize)) -> Self {
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
    })
}

/// Result of a background save, delivered to the UI thread.
pub enum BankEvent {
    Saved(BankEntryMeta),
    Failed { key: MineKey, error: String },
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
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let mut state = Self {
            db: Arc::new(Mutex::new(db)),
            entries,
            mined_keys: HashSet::new(),
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
                    messages.push(Ok(format!("Mined: {}", meta.term)));
                    let pos = self
                        .entries
                        .iter()
                        .position(|e| e.created_at <= meta.created_at)
                        .unwrap_or(self.entries.len());
                    self.entries.insert(pos, meta);
                }
                BankEvent::Failed { key, error } => {
                    self.in_flight.remove(&key);
                    messages.push(Err(format!("Failed to mine word: {error}")));
                }
            }
        }
        messages
    }

    /// True while a mining request is waiting for a capture or being saved.
    pub fn is_busy(&self) -> bool {
        !self.in_flight.is_empty()
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
        self.rebuild_keys();
        Ok(())
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
        };
        let text = "おはよう。今日は暑いね。";
        let request = MineRequest::from_lookup(&entry, text, (8, 10));
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

        let id = bank.entries[0].id;
        bank.delete(id).unwrap();
        assert!(bank.entries.is_empty());
        assert_eq!(bank.status("暑い", "あつい", "今日は暑いね。"), MineStatus::Available);
    }
}
