use super::database::DictDatabase;
use super::deinflect::Deinflector;
use super::models::DictPopupState;
use crate::ocr::OcrBox;
use eframe::egui::{self, Rect};

/// Scans OCR boxes under the mouse pointer and performs Yomitan-style Japanese word recognition.
pub fn lookup_word_at_pointer(
    pointer_pos: egui::Pos2,
    ocr_boxes: &[OcrBox],
    video_rect: egui::Rect,
    db: &DictDatabase,
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

        // If the box has broken-down lines, match against the specific line under the cursor!
        let lines_to_check: Vec<(&str, f32, f32, f32, f32)> = if ocr_box.lines.is_empty() {
            vec![(
                ocr_box.text.as_str(),
                min_x,
                max_x,
                min_y,
                max_y,
            )]
        } else {
            ocr_box
                .lines
                .iter()
                .map(|line| {
                    let min_x = video_rect.min.x + (line.center_x - line.width / 2.0) * video_rect.width();
                    let max_x = video_rect.min.x + (line.center_x + line.width / 2.0) * video_rect.width();
                    let min_y = video_rect.min.y + (line.center_y - line.height / 2.0) * video_rect.height();
                    let max_y = video_rect.min.y + (line.center_y + line.height / 2.0) * video_rect.height();
                    (line.text.as_str(), min_x, max_x, min_y, max_y)
                })
                .collect()
        };

        // Find the line closest to or containing pointer_pos.y
        let mut target_line = None;
        let mut min_dist_y = f32::MAX;

        for line_info in &lines_to_check {
            let (_, _, _, min_y, max_y) = *line_info;
            if pointer_pos.y >= min_y && pointer_pos.y <= max_y {
                target_line = Some(*line_info);
                break;
            }
            let dist = if pointer_pos.y < min_y {
                min_y - pointer_pos.y
            } else {
                pointer_pos.y - max_y
            };
            if dist < min_dist_y {
                min_dist_y = dist;
                target_line = Some(*line_info);
            }
        }

        let Some((line_text, line_min_x, line_max_x, line_min_y, line_max_y)) = target_line else {
            continue;
        };

        let chars: Vec<char> = line_text.chars().collect();
        if chars.is_empty() {
            continue;
        }

        // Relative horizontal position [0.0, 1.0] along THIS specific line
        let line_w = (line_max_x - line_min_x).max(1.0);
        let t = ((pointer_pos.x - line_min_x) / line_w).clamp(0.0, 0.999);
        let char_idx = ((t * chars.len() as f32).floor() as usize).min(chars.len() - 1);

        // Scan for word matches Yomitan-style:
        //
        // Phase 1 (Direct hover match):
        // Candidate spans starting EXACTLY at char_idx under the pointer.
        // We test from longest span (up to 16 characters) down to 1 character.
        // E.g. hovering over 'よ' in "のような気がする" matches "のような気がする".
        // E.g. hovering over '気' in "のような気がする" matches "気がする" (never "のような気がする"!).
        // E.g. hovering over 'な' in "なにか" matches "何か" (reading: "なにか").
        let max_len = 16.min(chars.len() - char_idx);
        let mut matched_entry: Option<(usize, usize, Vec<super::models::TermEntry>)> = None;

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
            let candidates = deinflector.deinflect(&sub_str);
            if let Ok(entries) = db.find_terms(&candidates) {
                if !entries.is_empty() {
                    matched_entry = Some((start, end, entries));
                    break;
                }
            }
        }

        // Phase 2 (Fallback lookbehind):
        // Only if ZERO dictionary entries start at char_idx, check if the cursor is
        // in the middle of a word whose starting character is up to 6 characters earlier.
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
                    let candidates = deinflector.deinflect(&sub_str);
                    if let Ok(entries) = db.find_terms(&candidates) {
                        if !entries.is_empty() {
                            matched_entry = Some((start, end, entries));
                            break 'lookbehind;
                        }
                    }
                }
            }
        }

        if let Some((start_offset, end_offset, entries)) = matched_entry {
            // Calculate precise sub-bounding box for the recognized word on this single line
            let word_min_x = line_min_x
                + (start_offset as f32 / chars.len() as f32) * (line_max_x - line_min_x);
            let word_max_x =
                line_min_x + (end_offset as f32 / chars.len() as f32) * (line_max_x - line_min_x);
            let word_rect = Rect::from_min_max(
                egui::pos2(word_min_x, line_min_y),
                egui::pos2(word_max_x, line_max_y),
            );

            return Some(DictPopupState {
                matched_term: entries[0].term.clone(),
                source_text: line_text.to_string(),
                char_range: (start_offset, end_offset),
                word_rect,
                box_rect,
                entries,
                is_popup_hovered: false,
                last_hover_time: std::time::Instant::now(),
            });
        }
    }

    None
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
            global_deinflector(),
        );
        assert!(popup_su.is_some());
        let p_su = popup_su.unwrap();
        assert_eq!(p_su.matched_term, "する");
        assert_eq!(p_su.char_range, (5, 7));
    }
}
