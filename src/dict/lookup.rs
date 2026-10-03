use super::database::DictDatabase;
use super::deinflect::Deinflector;
use super::long_vowel::long_vowel_variants;
use super::models::DictPopupState;
use super::particles::ParticleIndex;
use crate::ocr::OcrBox;
use eframe::egui::{self, Rect};

/// Scans OCR boxes under the mouse pointer and performs Yomitan-style Japanese word recognition.
pub fn lookup_word_at_pointer(
    pointer_pos: egui::Pos2,
    ocr_boxes: &[OcrBox],
    video_rect: egui::Rect,
    db: &DictDatabase,
    freq_db: Option<&super::frequency::FreqDatabase>,
    deinflector: &Deinflector,
) -> Option<DictPopupState> {
    if video_rect.width() <= 10.0 || video_rect.height() <= 10.0 || ocr_boxes.is_empty() {
        return None;
    }

    for ocr_box in ocr_boxes {
        let min_x = video_rect.min.x + (ocr_box.center_x - ocr_box.width / 2.0) * video_rect.width();
        let max_x = video_rect.min.x + (ocr_box.center_x + ocr_box.width / 2.0) * video_rect.width();
        let min_y = video_rect.min.y + (ocr_box.center_y - ocr_box.height / 2.0) * video_rect.height();
        let max_y = video_rect.min.y + (ocr_box.center_y + ocr_box.height / 2.0) * video_rect.height();

        let box_rect = Rect::from_min_max(egui::pos2(min_x, min_y), egui::pos2(max_x, max_y));

        if !box_rect.contains(pointer_pos) {
            continue;
        }

        // Break the box into lines with their own geometry. Matching is done over the
        // concatenation of ALL lines (in OCR reading order) so that words/expressions that
        // wrap across a line break still match, e.g. "ちょーしが⏎わるかった".
        let segments: Vec<LineSegment> = if ocr_box.lines.is_empty() {
            vec![LineSegment {
                text: ocr_box.text.as_str(),
                min_x,
                max_x,
                min_y,
                max_y,
                char_start: 0,
                char_len: 0,
            }]
        } else {
            ocr_box
                .lines
                .iter()
                .map(|line| LineSegment {
                    text: line.text.as_str(),
                    min_x: video_rect.min.x + (line.center_x - line.width / 2.0) * video_rect.width(),
                    max_x: video_rect.min.x + (line.center_x + line.width / 2.0) * video_rect.width(),
                    min_y: video_rect.min.y + (line.center_y - line.height / 2.0) * video_rect.height(),
                    max_y: video_rect.min.y + (line.center_y + line.height / 2.0) * video_rect.height(),
                    char_start: 0,
                    char_len: 0,
                })
                .collect()
        };

        let mut segments = segments;
        let mut chars: Vec<char> = Vec::new();
        for seg in &mut segments {
            seg.char_start = chars.len();
            chars.extend(seg.text.chars());
            seg.char_len = chars.len() - seg.char_start;
        }
        if chars.is_empty() {
            continue;
        }

        // Find the line closest to or containing pointer_pos.y
        let mut target_idx = None;
        let mut min_dist_y = f32::MAX;
        for (i, seg) in segments.iter().enumerate() {
            if seg.char_len == 0 {
                continue;
            }
            if pointer_pos.y >= seg.min_y && pointer_pos.y <= seg.max_y {
                target_idx = Some(i);
                break;
            }
            let dist = if pointer_pos.y < seg.min_y {
                seg.min_y - pointer_pos.y
            } else {
                pointer_pos.y - seg.max_y
            };
            if dist < min_dist_y {
                min_dist_y = dist;
                target_idx = Some(i);
            }
        }

        let Some(target_idx) = target_idx else {
            continue;
        };
        let target = &segments[target_idx];

        // Relative horizontal position [0.0, 1.0] along THIS specific line,
        // converted to an index into the concatenated character stream.
        let line_w = (target.max_x - target.min_x).max(1.0);
        let t = ((pointer_pos.x - target.min_x) / line_w).clamp(0.0, 0.999);
        let local_idx = ((t * target.char_len as f32).floor() as usize).min(target.char_len - 1);
        let char_idx = target.char_start + local_idx;
        let full_text: String = chars.iter().collect();

        // Scan for word matches Yomitan-style:
        //
        // Phase 1 (Direct hover match):
        // Candidate spans starting EXACTLY at char_idx under the pointer.
        // We test from longest span (up to 16 characters) down to 1 character.
        // Spans may continue onto the following line(s) of the same box.
        // E.g. hovering over 'よ' in "のような気がする" matches "のような気がする".
        // E.g. hovering over '気' in "のような気がする" matches "気がする" (never "のような気がする"!).
        // E.g. hovering over 'な' in "なにか" matches "何か" (reading: "なにか").
        let max_len = 16.min(chars.len() - char_idx);
        let particle_index = db.particle_index();
        let cache_key = ScanCacheKey {
            db_generation: db.generation,
            particle_index_ready: particle_index.is_some(),
            line_text: full_text.clone(),
            char_idx,
        };

        let cached = SCAN_CACHE.with(|c| {
            c.borrow()
                .as_ref()
                .filter(|(k, _)| *k == cache_key)
                .map(|(_, v)| v.clone())
        });

        let matched_entry: Option<(usize, usize, Vec<super::models::TermEntry>)> = if let Some(hit) = cached {
            hit
        } else {
            let mut matched_entry = None;

            for len in (1..=max_len).rev() {
                let start = char_idx;
                let end = char_idx + len;
                if is_punct_char(chars[start]) || is_punct_char(chars[end - 1]) {
                    continue;
                }
                let sub_str: String = chars[start..end].iter().collect();
                if is_ignorable_token(&sub_str) {
                    continue;
                }
                let entries = find_span_entries(&sub_str, db, deinflector, particle_index.as_deref());
                if !entries.is_empty() {
                    matched_entry = Some((start, end, entries));
                    break;
                }
            }

            // Phase 2 (Fallback lookbehind):
            // Only if ZERO dictionary entries start at char_idx, check if the cursor is
            // in the middle of a word whose starting character is up to 6 characters earlier
            // (possibly on the previous line).
            // We search from the closest start (char_idx - 1) backwards to min_start.
            if matched_entry.is_none() {
                let min_start = char_idx.saturating_sub(6);
                'lookbehind: for start in (min_start..char_idx).rev() {
                    if is_punct_char(chars[start]) {
                        break;
                    }
                    let max_end = (start + 16).min(chars.len());
                    for end in ((char_idx + 1)..=max_end).rev() {
                        if is_punct_char(chars[end - 1]) {
                            continue;
                        }
                        let sub_str: String = chars[start..end].iter().collect();
                        if is_ignorable_token(&sub_str) {
                            continue;
                        }
                        let entries =
                            find_span_entries(&sub_str, db, deinflector, particle_index.as_deref());
                        if !entries.is_empty() {
                            matched_entry = Some((start, end, entries));
                            break 'lookbehind;
                        }
                    }
                }
            }

            SCAN_CACHE.with(|c| *c.borrow_mut() = Some((cache_key, matched_entry.clone())));
            matched_entry
        };

        if let Some((start_offset, end_offset, mut entries)) = matched_entry {
            // Enrich entries with frequency if available and sort by frequency
            if let Some(fdb) = freq_db {
                for entry in &mut entries {
                    entry.frequency = fdb.get_frequency(&entry.term, &entry.reading);
                }
                // Sort in order of frequency: lowest rank first, unranked last
                entries.sort_by(|a, b| {
                    match (&a.frequency, &b.frequency) {
                        (Some(fa), Some(fb)) => fa.rank.cmp(&fb.rank),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => std::cmp::Ordering::Equal,
                    }
                });
            }

            // Calculate precise per-line sub-bounding boxes for the recognized word.
            // The segment on the hovered line becomes `word_rect` (popup anchor); segments on
            // other lines (when the word wraps) are kept as extra highlight rects.
            let mut word_rect = None;
            let mut extra_word_rects = Vec::new();
            for (i, seg) in segments.iter().enumerate() {
                if let Some(rect) = seg.sub_rect(start_offset, end_offset) {
                    if i == target_idx {
                        word_rect = Some(rect);
                    } else {
                        extra_word_rects.push(rect);
                    }
                }
            }
            let Some(word_rect) = word_rect.or_else(|| extra_word_rects.first().copied()) else {
                continue;
            };

            return Some(DictPopupState {
                matched_term: entries[0].term.clone(),
                source_text: full_text,
                char_range: (start_offset, end_offset),
                word_rect,
                extra_word_rects,
                box_rect,
                entries,
                is_popup_hovered: false,
                popup_rect: None,
                last_hover_time: std::time::Instant::now(),
            });
        }
    }

    None
}

/// A single OCR line within a box, with its screen geometry and its character range
/// inside the box's concatenated text.
struct LineSegment<'a> {
    text: &'a str,
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
    char_start: usize,
    char_len: usize,
}

impl LineSegment<'_> {
    /// Screen rect covering the part of global char range [start, end) on this line, if any.
    fn sub_rect(&self, start: usize, end: usize) -> Option<Rect> {
        let seg_end = self.char_start + self.char_len;
        let s = start.max(self.char_start);
        let e = end.min(seg_end);
        if s >= e || self.char_len == 0 {
            return None;
        }
        let w = self.max_x - self.min_x;
        let n = self.char_len as f32;
        let x0 = self.min_x + ((s - self.char_start) as f32 / n) * w;
        let x1 = self.min_x + ((e - self.char_start) as f32 / n) * w;
        Some(Rect::from_min_max(egui::pos2(x0, self.min_y), egui::pos2(x1, self.max_y)))
    }
}

/// Cache key for the last word scan. Repeated mouse-move frames over the same character
/// of the same line reuse the previous result instead of re-querying SQLite.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ScanCacheKey {
    db_generation: u64,
    particle_index_ready: bool,
    line_text: String,
    char_idx: usize,
}

type ScanResult = Option<(usize, usize, Vec<super::models::TermEntry>)>;

thread_local! {
    static SCAN_CACHE: std::cell::RefCell<Option<(ScanCacheKey, ScanResult)>> =
        const { std::cell::RefCell::new(None) };
}

/// Looks up dictionary entries for a single candidate span, in tiers:
///
/// 1. Exact / deinflected match (as before).
/// 2. Long-vowel normalized match for any word (e.g. `どーせ` → `どうせ`,
///    `せんせー` → `せんせい`), combined with deinflection.
/// 3. Particle-swapped forms of set expressions over the candidates of 1 and 2
///    (e.g. `突拍子がない` → `突拍子もない`). Only entries tagged as expressions are kept.
///
/// Because this is evaluated per span length (longest first), a longer normalized/swapped
/// match beats a shorter exact match, while at equal length an exact match always wins.
fn find_span_entries(
    sub_str: &str,
    db: &DictDatabase,
    deinflector: &Deinflector,
    particle_index: Option<&ParticleIndex>,
) -> Vec<super::models::TermEntry> {
    let mut candidates = deinflector.deinflect(sub_str);
    if let Ok(entries) = db.find_terms(&candidates) {
        if !entries.is_empty() {
            return entries;
        }
    }

    // Tier 2: long-vowel (ー) normalization. Only does work if the span contains ー.
    let variants = long_vowel_variants(sub_str);
    if !variants.is_empty() {
        let mut lv_candidates = Vec::new();
        // Variants are in priority order (e.g. せー → せい before せえ); first hit wins.
        for (variant, reason) in &variants {
            let mut cands = deinflector.deinflect(variant);
            for cand in &mut cands {
                // Normalization is applied to the surface text first, so it leads the trail.
                cand.reasons.insert(0, reason.clone());
            }
            if let Ok(entries) = db.find_terms(&cands) {
                if !entries.is_empty() {
                    return entries;
                }
            }
            lv_candidates.extend(cands);
        }
        candidates.extend(lv_candidates);
    }

    // Tier 3: particle swap for expressions.
    let Some(index) = particle_index else {
        return Vec::new();
    };
    let swapped = index.swapped_candidates(&candidates);
    if swapped.is_empty() {
        return Vec::new();
    }
    match db.find_terms(&swapped) {
        Ok(mut entries) => {
            entries.retain(|e| index.is_allowed(&e.term, &e.reading));
            entries
        }
        Err(_) => Vec::new(),
    }
}

/// Checks if a character is punctuation or whitespace that cannot start or end a word.
fn is_punct_char(c: char) -> bool {
    matches!(c,
        ' ' | '\t' | '\n' | '\r'
        | '、' | '。' | '，' | '．' | '！' | '？'
        | '「' | '」' | '『' | '』' | '（' | '）'
        | '(' | ')' | '[' | ']' | '{' | '}'
        | '・' | '：' | '；' | ':' | ';'
        | '—' | '-' | '…' | '~' | '～'
    )
}

/// Checks if a string is punctuation or whitespace that shouldn't trigger dictionary lookup.
fn is_ignorable_token(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return true;
    }
    trimmed.chars().all(|c| {
        matches!(c,
            ' ' | '\t' | '\n' | '\r'
            | '、' | '。' | '，' | '．' | '！' | '？'
            | '「' | '」' | '『' | '』' | '（' | '）'
            | '(' | ')' | '[' | ']' | '{' | '}'
            | '・' | '：' | '；' | ':' | ';'
            | '—' | 'ー' | '-' | '…' | '~' | '～'
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::database::DictDatabase;
    use crate::dict::deinflect::global_deinflector;
    use crate::ocr::ParsedLine;
    use tempfile::tempdir;

    #[test]
    fn test_lookup_word_at_pointer_multi_line() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = DictDatabase::open_or_create(&db_path).unwrap();

        db.conn
            .execute(
                "INSERT INTO terms VALUES ('食べる', 'たべる', 'v1', 'v1', 200.0, '[\"to eat\"]', 1, '')",
                [],
            )
            .unwrap();

        db.conn
            .execute(
                "INSERT INTO terms VALUES ('林檎', 'りんご', 'n', '', 150.0, '[\"apple\"]', 2, '')",
                [],
            )
            .unwrap();

        let line1 = ParsedLine {
            text: "林檎を食べた".to_string(),
            center_x: 0.5,
            center_y: 0.45,
            width: 0.6,
            height: 0.05,
            paragraph_idx: 0,
        };

        let line2 = ParsedLine {
            text: "美味しい林檎".to_string(),
            center_x: 0.5,
            center_y: 0.55,
            width: 0.6,
            height: 0.05,
            paragraph_idx: 0,
        };

        let ocr_box = OcrBox {
            text: "林檎を食べた 美味しい林檎".to_string(),
            center_x: 0.5,
            center_y: 0.50,
            width: 0.6,
            height: 0.15,
            lines: vec![line1, line2],
        };

        let video_rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 600.0));

        // Hover over line 1 (y = 270, center_y = 0.45 * 600 = 270)
        // '林檎' at x = 250
        let popup = lookup_word_at_pointer(
            egui::pos2(250.0, 270.0),
            &[ocr_box.clone()],
            video_rect,
            &db,
            None,
            global_deinflector(),
        );
        assert!(popup.is_some());
        let p = popup.unwrap();
        assert_eq!(p.matched_term, "林檎");
        assert_eq!(p.char_range, (0, 2));
        // Verify word_rect height matches line 1 height (0.05 * 600 = 30px) rather than whole box (90px)
        assert!((p.word_rect.height() - 30.0).abs() < 1.0);

        // Hover over line 1 over '食べた' (x = 550)
        let popup = lookup_word_at_pointer(
            egui::pos2(550.0, 270.0),
            &[ocr_box.clone()],
            video_rect,
            &db,
            None,
            global_deinflector(),
        );
        assert!(popup.is_some());
        let p = popup.unwrap();
        assert_eq!(p.matched_term, "食べる");
        assert_eq!(p.char_range, (3, 6));
        assert_eq!(p.entries[0].inflection_reasons, vec!["past"]);
        assert!((p.word_rect.height() - 30.0).abs() < 1.0);

        // Hover over line 2 (y = 330, center_y = 0.55 * 600 = 330)
        // '林檎' at x = 600
        let popup = lookup_word_at_pointer(
            egui::pos2(600.0, 330.0),
            &[ocr_box.clone()],
            video_rect,
            &db,
            None,
            global_deinflector(),
        );
        assert!(popup.is_some());
        let p = popup.unwrap();
        assert_eq!(p.matched_term, "林檎");
        assert!((p.word_rect.height() - 30.0).abs() < 1.0);
    }

    #[test]
    fn test_lookup_word_kana_matching_and_compound_prioritization() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = DictDatabase::open_or_create(&db_path).unwrap();

        // Entry with Kanji term and Hiragana reading
        db.conn
            .execute(
                "INSERT INTO terms VALUES ('何か', 'なにか', 'n', '', 200.0, '[\"something\"]', 1, '')",
                [],
            )
            .unwrap();

        // Single character entry 'な' that could shadow "なにか" if prefix length priority was broken
        db.conn
            .execute(
                "INSERT INTO terms VALUES ('な', 'な', 'prt', '', 100.0, '[\"particle na\"]', 2, '')",
                [],
            )
            .unwrap();

        let ocr_box = OcrBox {
            text: "なにか".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.3,
            height: 0.1,
            lines: Vec::new(),
        };

        let video_rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 600.0));
        // Box spans x: 350..650 (width 300, 3 chars -> each char is 100px wide)
        // 'な': 350..450, 'に': 450..550, 'か': 550..650

        // 1. Hover on 'な' (x = 400) -> should match full "何か" (range 0..3) because len 3 beats len 1 'な'
        let popup1 = lookup_word_at_pointer(
            egui::pos2(400.0, 300.0),
            &[ocr_box.clone()],
            video_rect,
            &db,
            None,
            global_deinflector(),
        );
        assert!(popup1.is_some());
        let p1 = popup1.unwrap();
        assert_eq!(p1.matched_term, "何か");
        assert_eq!(p1.char_range, (0, 3));

        // 2. Hover on 'に' (x = 500) when neither 'に' nor 'にか' is in the DB -> Phase 2 fallback lookbehind matches "何か"
        let popup2 = lookup_word_at_pointer(
            egui::pos2(500.0, 300.0),
            &[ocr_box.clone()],
            video_rect,
            &db,
            None,
            global_deinflector(),
        );
        assert!(popup2.is_some());
        let p2 = popup2.unwrap();
        assert_eq!(p2.matched_term, "何か");
        assert_eq!(p2.char_range, (0, 3));
    }

    #[test]
    fn test_lookup_subword_hover_prioritization() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_subword.db");
        let db = DictDatabase::open_or_create(&db_path).unwrap();

        // 1. "のような気がする" (long compound phrase)
        db.conn
            .execute(
                "INSERT INTO terms VALUES ('ような気がする', 'ようなきがする', 'exp', '', 250.0, '[\"to have a feeling that\"]', 1, '')",
                [],
            )
            .unwrap();

        // 2. "気がする" (sub-expression starting at '気')
        db.conn
            .execute(
                "INSERT INTO terms VALUES ('気がする', 'きがする', 'exp', '', 200.0, '[\"to have a hunch\"]', 2, '')",
                [],
            )
            .unwrap();

        // 3. "する" (verb starting at 'す')
        db.conn
            .execute(
                "INSERT INTO terms VALUES ('する', 'する', 'vs-i', '', 180.0, '[\"to do\"]', 3, '')",
                [],
            )
            .unwrap();

        // String: "のような気がする" (7 chars: よ=0, う=1, な=2, 気=3, が=4, す=5, る=6)
        let ocr_box = OcrBox {
            text: "ような気がする".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.7,
            height: 0.1,
            lines: Vec::new(),
        };

        let video_rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 600.0));
        // Box spans x: 150..850 (width 700, 7 chars -> each char is 100px wide)
        // 'よ': 150..250, 'う': 250..350, 'な': 350..450, '気': 450..550, 'が': 550..650, 'す': 650..750, 'る': 750..850

        // Hover over 'よ' (x = 200): must match "ような気がする" (range 0..7)
        let popup_yo = lookup_word_at_pointer(
            egui::pos2(200.0, 300.0),
            &[ocr_box.clone()],
            video_rect,
            &db,
            None,
            global_deinflector(),
        );
        assert!(popup_yo.is_some());
        let p_yo = popup_yo.unwrap();
        assert_eq!(p_yo.matched_term, "ような気がする");
        assert_eq!(p_yo.char_range, (0, 7));

        // Hover over '気' (x = 500): MUST match "気がする" (range 3..7), NOT "ような気がする"!
        let popup_ki = lookup_word_at_pointer(
            egui::pos2(500.0, 300.0),
            &[ocr_box.clone()],
            video_rect,
            &db,
            None,
            global_deinflector(),
        );
        assert!(popup_ki.is_some());
        let p_ki = popup_ki.unwrap();
        assert_eq!(p_ki.matched_term, "気がする");
        assert_eq!(p_ki.char_range, (3, 7));

        // Hover over 'す' (x = 700): MUST match "する" (range 5..7)
        let popup_su = lookup_word_at_pointer(
            egui::pos2(700.0, 300.0),
            &[ocr_box.clone()],
            video_rect,
            &db,
            None,
            global_deinflector(),
        );
        assert!(popup_su.is_some());
        let p_su = popup_su.unwrap();
        assert_eq!(p_su.matched_term, "する");
        assert_eq!(p_su.char_range, (5, 7));
    }

    #[test]
    fn test_lookup_sorts_by_frequency() {
        let dir = tempdir().unwrap();
        let dict_path = dir.path().join("test_dict.db");
        let freq_path = dir.path().join("test_freq.db");

        let dict_db = DictDatabase::open_or_create(&dict_path).unwrap();
        // Insert multiple homophones or entries matching 'き'
        dict_db
            .conn
            .execute_batch(
                "
                INSERT INTO terms VALUES ('生', 'き', 'n', '', 100.0, '[\"pure\"]', 1, '');
                INSERT INTO terms VALUES ('気', 'き', 'n', '', 100.0, '[\"spirit\"]', 2, '');
                INSERT INTO terms VALUES ('木', 'き', 'n', '', 100.0, '[\"tree\"]', 3, '');
                INSERT INTO terms VALUES ('奇', 'き', 'n', '', 100.0, '[\"strange\"]', 4, '');
                ",
            )
            .unwrap();

        let freq_db = crate::dict::FreqDatabase::open_or_create(&freq_path).unwrap();
        // Rank '気' as #150, '木' as #800, '生' as #3500. '奇' is unranked.
        freq_db
            .conn
            .execute_batch(
                "
                INSERT INTO frequencies VALUES ('気', 'き', 150, '150㋕');
                INSERT INTO frequencies VALUES ('木', 'き', 800, '800㋕');
                INSERT INTO frequencies VALUES ('生', 'き', 3500, '3500㋕');
                ",
            )
            .unwrap();

        let ocr_box = OcrBox {
            text: "き".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.1,
            height: 0.1,
            lines: Vec::new(),
        };

        let video_rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 600.0));
        let popup = lookup_word_at_pointer(
            egui::pos2(500.0, 300.0),
            &[ocr_box],
            video_rect,
            &dict_db,
            Some(&freq_db),
            global_deinflector(),
        )
        .unwrap();

        let terms: Vec<&str> = popup.entries.iter().map(|e| e.term.as_str()).collect();
        // Must be sorted in order of frequency: 気 (150) -> 木 (800) -> 生 (3500) -> 奇 (unranked)
        assert_eq!(terms, vec!["気", "木", "生", "奇"]);
        assert_eq!(popup.entries[0].frequency.as_ref().unwrap().rank, 150);
        assert_eq!(popup.entries[1].frequency.as_ref().unwrap().rank, 800);
        assert_eq!(popup.entries[2].frequency.as_ref().unwrap().rank, 3500);
        assert!(popup.entries[3].frequency.is_none());
    }

    /// Hovers over the first character of `text` (single-line box) and returns the popup.
    fn hover_first_char(text: &str, db: &DictDatabase) -> Option<DictPopupState> {
        let n = text.chars().count() as f32;
        let ocr_box = OcrBox {
            text: text.to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.8,
            height: 0.1,
            lines: Vec::new(),
        };
        let video_rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 600.0));
        // Box spans x: 100..900; first char center
        let x = 100.0 + (800.0 / n) * 0.5;
        lookup_word_at_pointer(egui::pos2(x, 300.0), &[ocr_box], video_rect, db, None, global_deinflector())
    }

    #[test]
    fn test_particle_swap_matches_expressions() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_particles.db");
        let db = DictDatabase::open_or_create(&db_path).unwrap();
        let exp = r#"[{"content":"exp","data":{"class":"tag","code":"exp"}},"gloss"]"#;
        let plain = r#"["gloss"]"#;
        let rows: &[(&str, &str, &str, &str, i64)] = &[
            ("突拍子もない", "とっぴょうしもない", "adj-i", exp, 1),
            ("突拍子", "とっぴょうし", "", plain, 2),
            ("気がする", "きがする", "vs", exp, 3),
            ("気もする", "きもする", "vs", exp, 4),
            ("目の無い", "めのない", "adj-i", exp, 5),
            // Not an expression: must never be reached via particle swap
            ("花の木", "はなのき", "", plain, 6),
        ];
        for (term, reading, rules, gloss, seq) in rows {
            db.conn
                .execute(
                    "INSERT INTO terms VALUES (?1, ?2, '', ?3, 100.0, ?4, ?5, '')",
                    rusqlite::params![term, reading, rules, gloss, seq],
                )
                .unwrap();
        }
        db.rebuild_particle_index().unwrap();

        // Kanji text, は→も
        let p = hover_first_char("突拍子はない", &db).unwrap();
        assert_eq!(p.matched_term, "突拍子もない");
        assert_eq!(p.char_range, (0, 6));
        assert_eq!(p.entries[0].inflection_reasons, vec!["は→も"]);

        // Kanji text, が→も
        let p = hover_first_char("突拍子がない", &db).unwrap();
        assert_eq!(p.matched_term, "突拍子もない");

        // Kana-only text must match via the reading
        let p = hover_first_char("とっぴょうしがない", &db).unwrap();
        assert_eq!(p.matched_term, "突拍子もない");
        assert_eq!(p.char_range, (0, 9));
        assert_eq!(p.entries[0].inflection_reasons, vec!["が→も"]);

        // Combined with deinflection
        let p = hover_first_char("突拍子がなかった", &db).unwrap();
        assert_eq!(p.matched_term, "突拍子もない");
        assert_eq!(p.char_range, (0, 8));
        assert_eq!(p.entries[0].inflection_reasons.last().unwrap(), "が→も");

        // の↔が
        let p = hover_first_char("目が無い", &db).unwrap();
        assert_eq!(p.matched_term, "目の無い");

        // Exact match wins at equal length
        let p = hover_first_char("気がする", &db).unwrap();
        assert_eq!(p.matched_term, "気がする");
        assert!(p.entries.iter().all(|e| e.term != "気もする"));

        // Shorter exact match still works on its own
        let p = hover_first_char("突拍子だ", &db).unwrap();
        assert_eq!(p.matched_term, "突拍子");

        // Non-expression entries are never particle-swapped
        assert!(hover_first_char("花が木", &db).is_none());
    }

    #[test]
    fn test_long_vowel_normalization() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_long_vowel.db");
        let db = DictDatabase::open_or_create(&db_path).unwrap();
        let exp = r#"[{"content":"exp","data":{"class":"tag","code":"exp"}},"gloss"]"#;
        let plain = r#"["gloss"]"#;
        let rows: &[(&str, &str, &str, &str, i64)] = &[
            ("どうせ", "どうせ", "", plain, 1),
            ("先生", "せんせい", "", plain, 2),
            ("ねえ", "ねえ", "", plain, 3),
            ("通り", "とおり", "", plain, 4),
            ("突拍子もない", "とっぴょうしもない", "adj-i", exp, 5),
        ];
        for (term, reading, rules, gloss, seq) in rows {
            db.conn
                .execute(
                    "INSERT INTO terms VALUES (?1, ?2, '', ?3, 100.0, ?4, ?5, '')",
                    rusqlite::params![term, reading, rules, gloss, seq],
                )
                .unwrap();
        }
        db.rebuild_particle_index().unwrap();

        let p = hover_first_char("どーせ", &db).unwrap();
        assert_eq!(p.matched_term, "どうせ");
        assert_eq!(p.char_range, (0, 3));
        assert_eq!(p.entries[0].inflection_reasons, vec!["ー→う"]);

        // e-row: い preferred, matched via reading
        let p = hover_first_char("せんせー", &db).unwrap();
        assert_eq!(p.matched_term, "先生");
        assert_eq!(p.char_range, (0, 4));

        // e-row fallback to え
        let p = hover_first_char("ねー", &db).unwrap();
        assert_eq!(p.matched_term, "ねえ");

        // o-row fallback to お
        let p = hover_first_char("とーり", &db).unwrap();
        assert_eq!(p.matched_term, "通り");

        // Combined with particle swap for expressions
        let p = hover_first_char("とっぴょーしがない", &db).unwrap();
        assert_eq!(p.matched_term, "突拍子もない");
        assert_eq!(p.entries[0].inflection_reasons, vec!["ー→う", "が→も"]);
    }

    #[test]
    fn test_match_across_line_boundary() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_multiline.db");
        let db = DictDatabase::open_or_create(&db_path).unwrap();
        let exp = r#"[{"content":"exp","data":{"class":"tag","code":"exp"}},"to be out of sorts"]"#;
        db.conn
            .execute(
                "INSERT INTO terms VALUES ('調子が悪い', 'ちょうしがわるい', '', 'adj-i', 100.0, ?1, 1, '')",
                rusqlite::params![exp],
            )
            .unwrap();
        db.rebuild_particle_index().unwrap();

        let l1 = "「マイ、バカじゃないもー。ちみっとちょーしが";
        let l2 = "わるかっただけでもー」";
        let line1 = ParsedLine {
            text: l1.to_string(),
            center_x: 0.5,
            center_y: 0.45,
            width: 0.8,
            height: 0.05,
            paragraph_idx: 0,
        };
        let line2 = ParsedLine {
            text: l2.to_string(),
            center_x: 0.3,
            center_y: 0.55,
            width: 0.4,
            height: 0.05,
            paragraph_idx: 0,
        };
        let ocr_box = OcrBox {
            text: format!("{l1}{l2}"),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.8,
            height: 0.15,
            lines: vec![line1, line2],
        };
        let video_rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 600.0));

        let l1_chars: Vec<char> = l1.chars().collect();
        let n1 = l1_chars.len();
        // Index of "ちょ" (the second ち in the line)
        let cho_idx = (0..n1 - 1)
            .find(|&i| l1_chars[i] == 'ち' && l1_chars[i + 1] == 'ょ')
            .unwrap();

        // Line 1 spans x: 100..900 at y = 270; hover over ち of ちょーしが
        let x = 100.0 + 800.0 * (cho_idx as f32 + 0.5) / n1 as f32;
        let p = lookup_word_at_pointer(
            egui::pos2(x, 270.0),
            &[ocr_box.clone()],
            video_rect,
            &db,
            None,
            global_deinflector(),
        )
        .expect("should match across the line break");
        assert_eq!(p.matched_term, "調子が悪い");
        // ちょーしが (6, rest of line 1) + わるかった (5, line 2) = 10 chars
        assert_eq!(p.char_range, (cho_idx, cho_idx + 10));
        assert_eq!(p.entries[0].inflection_reasons[0], "ー→う");
        // Highlight split across both lines; anchor on hovered line 1
        assert!((p.word_rect.center().y - 270.0).abs() < 1.0);
        assert_eq!(p.extra_word_rects.len(), 1);
        assert!((p.extra_word_rects[0].center().y - 330.0).abs() < 1.0);

        // Hovering わ on line 2 (x: 100..500, y = 330) finds the expression via lookbehind
        // into line 1, since nothing in the DB starts at わ.
        let n2 = l2.chars().count() as f32;
        let x2 = 100.0 + 400.0 * 0.5 / n2;
        let p = lookup_word_at_pointer(
            egui::pos2(x2, 330.0),
            &[ocr_box],
            video_rect,
            &db,
            None,
            global_deinflector(),
        )
        .expect("lookbehind should cross the line break");
        assert_eq!(p.matched_term, "調子が悪い");
        assert!((p.word_rect.center().y - 330.0).abs() < 1.0);
        assert_eq!(p.extra_word_rects.len(), 1);
    }
}
