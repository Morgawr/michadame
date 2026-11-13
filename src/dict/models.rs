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
    /// Optional recursive child popup opened by hovering a Japanese word inside this popup.
    pub child: Option<Box<DictPopupState>>,
}

impl DictPopupState {
    /// Returns true if `pos` is inside this popup's window rect or any recursive child popup's rect.
    pub fn contains_pointer(&self, pos: egui::Pos2) -> bool {
        self.popup_rect.is_some_and(|r| r.contains(pos))
            || self.child.as_ref().is_some_and(|c| c.contains_pointer(pos))
    }

    /// Returns true if this popup or any recursive child popup is currently hovered.
    pub fn is_any_hovered(&self) -> bool {
        self.is_popup_hovered || self.child.as_ref().is_some_and(|c| c.is_any_hovered())
    }

    /// Returns the 0-based depth of the deepest open popup in this chain (`0` if `self.child` is `None`).
    pub fn deepest_depth(&self) -> usize {
        match &self.child {
            Some(c) => 1 + c.deepest_depth(),
            None => 0,
        }
    }

    /// Returns the 0-based depth of the topmost (deepest) popup layer whose `popup_rect` contains `pos`.
    pub fn topmost_layer_at(&self, pos: egui::Pos2) -> Option<usize> {
        if let Some(child) = &self.child {
            if let Some(d) = child.topmost_layer_at(pos) {
                return Some(d + 1);
            }
        }
        self.popup_rect.filter(|r| r.contains(pos)).map(|_| 0)
    }

    /// Refreshes `last_hover_time` and `last_word_hover_time` on this popup and all recursive children.
    pub fn touch_all_hover_times(&mut self, now: std::time::Instant) {
        self.last_hover_time = now;
        self.last_word_hover_time = now;
        if let Some(child) = &mut self.child {
            child.touch_all_hover_times(now);
        }
    }

    /// Removes entries matching `custom_name_id == Some(id)` from this popup and all recursive children.
    /// If any child popup becomes empty, it (and its descendants) are closed.
    /// Returns `true` if this popup itself is now empty.
    pub fn remove_custom_name(&mut self, id: i64) -> bool {
        self.entries.retain(|e| e.custom_name_id != Some(id));
        if let Some(child) = &mut self.child {
            if child.remove_custom_name(id) {
                self.child = None;
            }
        }
        self.entries.is_empty()
    }

    /// Returns cached `(sentence, word_char_offset)` for `(self.source_text, self.char_range)`
    /// without re-running `extract_sentence` on every frame while the popup remains open.
    pub fn cached_sentence_rc(&self) -> (std::rc::Rc<str>, (usize, usize)) {
        struct CachedPopupSentence {
            source_text: String,
            char_range: (usize, usize),
            sentence: std::rc::Rc<str>,
            word_range: (usize, usize),
        }
        thread_local! {
            static SENTENCE_CACHE: std::cell::RefCell<Option<CachedPopupSentence>> =
                const { std::cell::RefCell::new(None) };
        }
        SENTENCE_CACHE.with(|cache| {
            if let Some(hit) = cache.borrow().as_ref() {
                if hit.char_range == self.char_range && hit.source_text == self.source_text {
                    return (hit.sentence.clone(), hit.word_range);
                }
            }
            let (sentence_str, word_range) =
                crate::bank::sentence::extract_sentence(&self.source_text, self.char_range);
            let sentence: std::rc::Rc<str> = std::rc::Rc::from(sentence_str);
            *cache.borrow_mut() = Some(CachedPopupSentence {
                source_text: self.source_text.clone(),
                char_range: self.char_range,
                sentence: sentence.clone(),
                word_range,
            });
            (sentence, word_range)
        })
    }

    /// Invokes `f(sentence, word_range)` using the cached extracted sentence for this popup.
    #[inline]
    pub fn with_cached_sentence<R>(&self, f: impl FnOnce(&str, (usize, usize)) -> R) -> R {
        let (sentence, word_range) = self.cached_sentence_rc();
        f(&sentence, word_range)
    }

    /// Convenience helper returning `(sentence, word_range)` as an owned `(String, (usize, usize))`.
    #[allow(dead_code)]
    pub fn cached_sentence(&self) -> (String, (usize, usize)) {
        let (sentence, word_range) = self.cached_sentence_rc();
        (sentence.to_string(), word_range)
    }
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
