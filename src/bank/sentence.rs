//! Sentence boundary extraction for mined words.
//!
//! An OCR box may contain several sentences. When a word is mined, only the sentence that
//! contains the word is stored. Sentences are split on Japanese/ASCII sentence terminators;
//! closing brackets immediately following a terminator (e.g. `。」`) belong to the sentence
//! they close. Unbalanced brackets left at the edges of the extracted sentence are trimmed.

/// Characters that end a sentence.
///
/// Ellipses (`…`, `‥`) are intentionally *not* terminators: in game/anime dialogue they are
/// far more often used as mid-sentence pauses ("あの…私は") than as sentence ends.
fn is_terminator(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '!' | '?' | '．' | '｡' | '\n')
}

/// Bracket pairs that may wrap dialogue or asides.
const BRACKET_PAIRS: &[(char, char)] = &[
    ('「', '」'),
    ('『', '』'),
    ('（', '）'),
    ('(', ')'),
    ('【', '】'),
    ('“', '”'),
    ('〈', '〉'),
    ('《', '》'),
    ('［', '］'),
    ('〔', '〕'),
    ('｢', '｣'),
];

fn is_closing_bracket(c: char) -> bool {
    BRACKET_PAIRS.iter().any(|&(_, close)| close == c) || c == '"'
}

fn closer_for(open: char) -> Option<char> {
    BRACKET_PAIRS.iter().find(|&&(o, _)| o == open).map(|&(_, c)| c)
}

fn opener_for(close: char) -> Option<char> {
    BRACKET_PAIRS.iter().find(|&&(_, c)| c == close).map(|&(o, _)| o)
}

/// Splits `chars` into sentence spans `[start, end)` covering the whole input.
fn sentence_spans(chars: &[char]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < chars.len() {
        if is_terminator(chars[i]) {
            // Absorb runs of terminators ("！？", "。。") and closing brackets ("。」").
            let mut end = i + 1;
            while end < chars.len() && (is_terminator(chars[end]) || is_closing_bracket(chars[end])) {
                end += 1;
            }
            spans.push((start, end));
            start = end;
            i = end;
        } else {
            i += 1;
        }
    }
    if start < chars.len() {
        spans.push((start, chars.len()));
    }
    spans
}

/// Extracts the sentence containing the character range `char_range` (in Unicode scalar
/// indices) from `text`.
///
/// Returns the sentence and the position of the word within it (char indices, `[start, end)`).
/// If the word straddles a sentence boundary, all touched sentences are included.
pub fn extract_sentence(text: &str, char_range: (usize, usize)) -> (String, (usize, usize)) {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return (String::new(), (0, 0));
    }
    let word_start = char_range.0.min(chars.len());
    let word_end = char_range.1.clamp(word_start, chars.len());

    let spans = sentence_spans(&chars);
    let last_word_char = word_end.saturating_sub(1).max(word_start).min(chars.len() - 1);
    let containing = |idx: usize| {
        spans
            .iter()
            .position(|&(s, e)| idx >= s && idx < e)
            .unwrap_or(spans.len() - 1)
    };
    let first_span = containing(word_start.min(chars.len() - 1));
    let last_span = containing(last_word_char);
    let mut start = spans[first_span].0;
    let mut end = spans[last_span.max(first_span)].1;

    // Trim whitespace at the edges (never into the word itself).
    while start < word_start && chars[start].is_whitespace() {
        start += 1;
    }
    while end > word_end && chars[end - 1].is_whitespace() {
        end -= 1;
    }

    // Trim unbalanced brackets at the edges, e.g. the leading 「 of the first sentence of
    // a quote, or the trailing 」 of its last sentence.
    loop {
        let mut changed = false;
        if start < word_start {
            if let Some(close) = closer_for(chars[start]) {
                if !chars[start + 1..end].contains(&close) {
                    start += 1;
                    changed = true;
                }
            }
        }
        if end > word_end {
            let last = chars[end - 1];
            let unmatched = match opener_for(last) {
                Some(open) => !chars[start..end - 1].contains(&open),
                None => last == '"' && chars[start..end - 1].iter().filter(|&&c| c == '"').count() % 2 == 0,
            };
            if unmatched {
                end -= 1;
                changed = true;
            }
        }
        while start < word_start && chars[start].is_whitespace() {
            start += 1;
            changed = true;
        }
        while end > word_end && chars[end - 1].is_whitespace() {
            end -= 1;
            changed = true;
        }
        if !changed {
            break;
        }
    }

    let sentence: String = chars[start..end].iter().collect();
    (sentence, (word_start - start, word_end - start))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range_of(text: &str, word: &str) -> (usize, usize) {
        let byte = text.find(word).expect("word in text");
        let start = text[..byte].chars().count();
        (start, start + word.chars().count())
    }

    fn sentence_for(text: &str, word: &str) -> String {
        extract_sentence(text, range_of(text, word)).0
    }

    #[test]
    fn picks_only_the_sentence_containing_the_word() {
        let text = "今日は暑い。明日は雨が降るらしい。";
        assert_eq!(sentence_for(text, "暑い"), "今日は暑い。");
        assert_eq!(sentence_for(text, "雨"), "明日は雨が降るらしい。");
    }

    #[test]
    fn last_sentence_without_terminator() {
        let text = "準備はできた？じゃあ行こう";
        assert_eq!(sentence_for(text, "行こう"), "じゃあ行こう");
        assert_eq!(sentence_for(text, "準備"), "準備はできた？");
    }

    #[test]
    fn quoted_dialogue_brackets_are_balanced() {
        let text = "「おはよう。今日は暑いね。」";
        assert_eq!(sentence_for(text, "おはよう"), "おはよう。");
        assert_eq!(sentence_for(text, "暑い"), "今日は暑いね。");

        let single = "「本当に行くの？」";
        assert_eq!(sentence_for(single, "行く"), "「本当に行くの？」");
    }

    #[test]
    fn closing_bracket_after_terminator_belongs_to_previous_sentence() {
        let text = "「待って！」「何だよ。」";
        assert_eq!(sentence_for(text, "何"), "「何だよ。」");
        assert_eq!(sentence_for(text, "待って"), "「待って！」");
    }

    #[test]
    fn terminator_runs_stay_together() {
        let text = "嘘だろ！？信じられない。";
        assert_eq!(sentence_for(text, "嘘"), "嘘だろ！？");
        assert_eq!(sentence_for(text, "信じ"), "信じられない。");
    }

    #[test]
    fn ellipsis_does_not_split() {
        let text = "あの…私は学生です。";
        assert_eq!(sentence_for(text, "学生"), "あの…私は学生です。");
    }

    #[test]
    fn word_position_is_relative_to_sentence() {
        let text = "「おはよう。今日は暑いね。」";
        let (sentence, (s, e)) = extract_sentence(text, range_of(text, "暑い"));
        let chars: Vec<char> = sentence.chars().collect();
        assert_eq!(chars[s..e].iter().collect::<String>(), "暑い");
    }

    #[test]
    fn handles_out_of_range_and_empty_input() {
        assert_eq!(extract_sentence("", (0, 3)).0, "");
        let (s, _) = extract_sentence("短い文。", (2, 99));
        assert_eq!(s, "短い文。");
    }
}
