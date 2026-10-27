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

/// Frequency information for a dictionary term.
#[derive(Clone, Debug, PartialEq)]
pub struct TermFrequency {
    /// Name of the frequency dictionary (e.g. "Jiten")
    pub dictionary: String,
    /// Numerical frequency rank (1 = most common word)
    pub rank: i64,
    /// Optional formatted display rank from dictionary (e.g. "1㋕")
    pub display_value: Option<String>,
}

impl TermFrequency {
    pub fn display_text(&self) -> String {
        match &self.display_value {
            Some(dv) => format!("Jiten: {dv}"),
            None => format!("Jiten: #{}", self.rank),
        }
    }

    /// Returns true if this frequency is specifically a kana-only reading frequency (marked with ㋕).
    pub fn is_kana_freq(&self) -> bool {
        self.display_value
            .as_deref()
            .map_or(false, |dv| dv.contains('㋕'))
    }
}

/// How the pitch pattern is specified for a single pitch accent variant.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum PitchPattern {
    /// Mora position of the pitch accent downstep (0 = heiban/flat, 1 = atamadaka, 2+ = nakadaka/odaka).
    Downstep(u32),
    /// Explicit pitch level per mora ('H' or 'L'), e.g. "LHH" or "HLL".
    Contours(String),
}

/// A single pitch accent pattern for a term/reading (a word may have multiple valid patterns).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PitchAccent {
    /// Kana reading associated with this pitch accent (if empty, uses the entry's reading/term).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reading: String,
    /// Downstep position or explicit H/L contour string.
    pub pattern: PitchPattern,
    /// 1-based mora indices with nasalized pronunciation (e.g. か゚ / が行鼻濁音).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nasal: Vec<u32>,
    /// 1-based mora indices with devoiced pronunciation (無声化).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devoice: Vec<u32>,
    /// Optional tags (e.g. part-of-speech or dialect restriction).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
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
    /// Frequency information if available from a frequency dictionary (e.g. Jiten)
    pub frequency: Option<TermFrequency>,
    /// Pitch accent patterns if available from the imported pitch accent dictionary
    pub pitches: Vec<PitchAccent>,
    /// If this entry is from the custom name dictionary, its database ID.
    pub custom_name_id: Option<i64>,
    /// Source tag if this entry is from the custom name dictionary.
    pub custom_tag: Option<String>,
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
    /// Additional highlight rects when the matched word wraps onto other lines of the box
    pub extra_word_rects: Vec<egui::Rect>,
    /// Pixel bounding box of the containing OCR box in screen coordinates
    pub box_rect: egui::Rect,
    /// Matching dictionary entries found for this word
    pub entries: Vec<TermEntry>,
    /// True if the user's cursor is currently inside the popup window
    pub is_popup_hovered: bool,
    /// Screen rect of the popup window as drawn on the last frame (None until first drawn).
    /// Used to "eat" hover events so words underneath the popup don't steal focus.
    pub popup_rect: Option<egui::Rect>,
    /// Timestamp of when this popup/word was last hovered, for persistence
    pub last_hover_time: std::time::Instant,
    /// Timestamp of when the cursor was last over the popup window or the highlighted word
    /// itself (not just the containing OCR box). Switching to a different word is delayed
    /// until `POPUP_SWITCH_DELAY` after this, so the cursor can travel from the word to the
    /// popup across neighboring words without replacing the popup.
    pub last_word_hover_time: std::time::Instant,
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

    FreqUpdateCheckFinished {
        update_available: bool,
        remote_metadata: Option<DictMetadata>,
    },
    FreqSyncProgress {
        message: String,
        progress: f32, // 0.0 to 1.0
    },
    FreqSyncFinished(Result<DictMetadata, String>),

    PitchImportProgress {
        message: String,
        progress: f32, // 0.0 to 1.0
    },
    PitchImportFinished(Result<DictMetadata, String>),
}
