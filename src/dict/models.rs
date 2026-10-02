use eframe::egui;
use serde::{Deserialize, Serialize};

/// Metadata describing a loaded or remote Yomitan dictionary.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DictMetadata {
    pub title: String,
    pub revision: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "downloadUrl", default)]
    pub download_url: Option<String>,
    #[serde(rename = "indexUrl", default)]
    pub index_url: Option<String>,
    #[serde(rename = "sourceLanguage", default)]
    pub source_language: Option<String>,
    #[serde(rename = "targetLanguage", default)]
    pub target_language: Option<String>,
    #[serde(default)]
    pub total_entries: usize,
}

/// A definition element in a term's glossary.
#[derive(Clone, Debug, PartialEq)]
pub enum GlossaryEntry {
    /// Plain string definition or raw HTML snippet
    Text(String),
    /// Yomitan structured-content JSON AST node
    Structured(serde_json::Value),
}

/// A single dictionary entry retrieved for a word or term.
#[derive(Clone, Debug, PartialEq)]
pub struct TermEntry {
    /// The Japanese expression/headword (e.g. "食べる", "行く")
    pub term: String,
    /// Reading in kana (e.g. "たべる", "いく"), or empty if same as term
    pub reading: String,
    /// Definition tags (e.g. "v1 vt")
    pub definition_tags: Option<String>,
    /// Deinflection rules applicable to this term (e.g. "v1", "v5", "adj-i")
    pub rules: String,
    /// Frequency / popularity score
    pub score: f64,
    /// List of definition senses / structured contents
    pub glossary: Vec<GlossaryEntry>,
    /// Sequence number for grouping definitions
    pub sequence: i64,
    /// Additional term tags
    pub term_tags: Option<String>,
    /// Inflection chain reasons that led to this term (e.g. ["past"], ["causative", "passive"])
    pub inflection_reasons: Vec<String>,
}

/// A candidate produced by the Japanese deinflector.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeinflectionCandidate {
    pub term: String,
    pub rules: u32,
    pub reasons: Vec<String>,
}

/// Active popup state for a hovered word over the video feed.
#[derive(Clone, Debug)]
pub struct DictPopupState {
    /// The base/matched headword
    pub matched_term: String,
    /// The full text of the OCR line or block
    pub source_text: String,
    /// Unicode character range [start, end) within source_text
    pub char_range: (usize, usize),
    /// Pixel bounding box of the recognized word in screen coordinates
    pub word_rect: egui::Rect,
    /// Pixel bounding box of the containing OCR box in screen coordinates
    pub box_rect: egui::Rect,
    /// Matching dictionary entries found for this word
    pub entries: Vec<TermEntry>,
    /// True if the user's cursor is currently inside the popup window
    pub is_popup_hovered: bool,
    /// Timestamp of when this popup/word was last hovered, for persistence
    pub last_hover_time: std::time::Instant,
}

/// Background events sent between worker threads and the UI.
#[derive(Debug)]
pub enum DictEvent {
    UpdateCheckFinished {
        update_available: bool,
        remote_metadata: Option<DictMetadata>,
    },
    SyncProgress {
        message: String,
        progress: f32, // 0.0 to 1.0
    },
    SyncFinished(Result<DictMetadata, String>),
}
