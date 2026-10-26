use super::kanji_table::{KANJI_CHARS, KANJI_WORDS};
use super::library::Pc98GameEntry;

/// Precomputed multi-representation search index for a single PC-98 game entry.
/// Enables fast 60 FPS fuzzy matching across Japanese (Kanji, Hiragana, Katakana,
/// Halfwidth Katakana), Romaji (Hepburn, Kunrei/Wapuro, and vowel-collapsed),
/// and English loanwords.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pc98SearchIndex {
    /// Normalized lowercase title (fullwidth ASCII -> halfwidth, Katakana -> Hiragana, Kyujitai -> Shinjitai).
    pub norm_title: String,
    /// Spaceless version of `norm_title`.
    pub norm_title_compact: String,
    /// Title with all Kanji and Katakana converted into Hiragana.
    pub hira_title: String,
    /// Spaceless version of `hira_title`.
    pub hira_title_compact: String,
    /// Hepburn romaji of `hira_title` (spaceless).
    pub romaji_hepburn: String,
    /// Kunrei / Wapuro romaji of `hira_title` (spaceless).
    pub romaji_kunrei: String,
    /// Long-vowel-collapsed Hepburn romaji (e.g. `doukyuusei` -> `dokyuse`, `touhou` -> `toho`, `imaaju` -> `imaju`).
    pub romaji_short: String,
    /// Phonetic consonant+digit skeleton for matching English spellings against Katakana loanwords.
    pub skeleton_title: String,
    /// Space-separated words from normalized title and romaji title for word-level fuzzy / typo matching.
    pub words: Vec<String>,
    /// Additional searchable text (category + disk image file stems in normalized, hiragana, and romaji forms).
    pub extra_blob: String,
}

impl Pc98SearchIndex {
    pub fn build(
        title: &str,
        category: Option<&str>,
        floppies: &[std::path::PathBuf],
        hdds: &[std::path::PathBuf],
        cdroms: &[std::path::PathBuf],
    ) -> Self {
        let norm_title = normalize_text(title);
        let norm_title_compact = strip_spaces_and_punct(&norm_title);
        let hira_title = kanji_to_hiragana(&norm_title);
        let hira_title_compact = strip_spaces_and_punct(&hira_title);

        let romaji_hepburn_spaced = hiragana_to_romaji(&hira_title, false);
        let romaji_kunrei_spaced = hiragana_to_romaji(&hira_title, true);

        let romaji_hepburn = strip_spaces_and_punct(&romaji_hepburn_spaced);
        let romaji_kunrei = strip_spaces_and_punct(&romaji_kunrei_spaced);
        let romaji_short = collapse_long_vowels(&romaji_hepburn);
        let skeleton_title = phonetic_skeleton(&romaji_hepburn_spaced);

        let mut words = Vec::new();
        for w in norm_title
            .split_whitespace()
            .chain(romaji_hepburn_spaced.split_whitespace())
        {
            let clean = strip_spaces_and_punct(w);
            if clean.len() >= 2 && !words.contains(&clean) {
                words.push(clean.clone());
                let short = collapse_long_vowels(&clean);
                if short.len() >= 2 && !words.contains(&short) {
                    words.push(short);
                }
            }
        }

        let mut extra_raw = String::new();
        if let Some(cat) = category {
            extra_raw.push_str(cat);
            extra_raw.push(' ');
        }
        for path in floppies.iter().chain(hdds.iter()).chain(cdroms.iter()) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                extra_raw.push_str(stem);
                extra_raw.push(' ');
            }
        }
        let norm_extra = normalize_text(&extra_raw);
        let hira_extra = kanji_to_hiragana(&norm_extra);
        let roma_extra = hiragana_to_romaji(&hira_extra, false);
        let roma_extra_short = collapse_long_vowels(&strip_spaces_and_punct(&roma_extra));
        let extra_blob = format!(
            "{} {} {} {}",
            strip_spaces_and_punct(&norm_extra),
            strip_spaces_and_punct(&hira_extra),
            strip_spaces_and_punct(&roma_extra),
            roma_extra_short
        );

        Self {
            norm_title,
            norm_title_compact,
            hira_title,
            hira_title_compact,
            romaji_hepburn,
            romaji_kunrei,
            romaji_short,
            skeleton_title,
            words,
            extra_blob,
        }
    }
}

/// Filters and ranks `games` against `query`, returning indices into `games`.
/// When `query` is empty or whitespace, returns all indices `0..games.len()` in library order.
/// When `query` is non-empty, returns matching indices sorted by descending relevance score.
pub fn filter_and_rank_games(games: &[Pc98GameEntry], query: &str) -> Vec<usize> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return (0..games.len()).collect();
    }

    let mut scored: Vec<(usize, i32)> = games
        .iter()
        .enumerate()
        .filter_map(|(idx, game)| match_game_score(game, trimmed).map(|s| (idx, s)))
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored.into_iter().map(|(idx, _)| idx).collect()
}

/// Computes a fuzzy match score (`Some(score > 0)`) if `game` matches `query`, or `None` if it does not.
pub fn match_game_score(game: &Pc98GameEntry, query: &str) -> Option<i32> {
    match_search_index_score(&game.search_index, query)
}

/// Computes a fuzzy match score (`Some(score > 0)`) if `idx` matches `query`, or `None` if it does not.
pub fn match_search_index_score(idx: &Pc98SearchIndex, query: &str) -> Option<i32> {
    let norm_query = normalize_text(query);
    let tokens: Vec<&str> = norm_query
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return Some(0);
    }

    let mut total_score: i32 = 0;

    for &raw_tok in &tokens {
        let tok_score = score_single_token(idx, raw_tok)?;
        total_score += tok_score;
    }

    // Bonus when a multi-token query matches contiguously as a phrase in romaji or normalized title
    if tokens.len() > 1 {
        let joined_en_roma = expand_english_words_to_romaji(&norm_query);
        let joined_compact = strip_spaces_and_punct(&joined_en_roma);
        let joined_short = collapse_long_vowels(&joined_compact);
        if !joined_compact.is_empty() {
            if idx.norm_title_compact.contains(&joined_compact)
                || idx.romaji_hepburn.contains(&joined_compact)
                || idx.romaji_short.contains(&joined_short)
            {
                total_score += 450;
            }
        }
    }

    // Slight tie-breaker preference for shorter titles when match quality is otherwise equal
    let len_penalty = (idx.norm_title_compact.len() as i32).min(60);
    Some((total_score - len_penalty).max(1))
}

fn score_single_token(idx: &Pc98SearchIndex, raw_tok: &str) -> Option<i32> {
    let tok_compact = strip_spaces_and_punct(raw_tok);
    if tok_compact.is_empty() {
        return Some(1);
    }

    let tok_hira = strip_spaces_and_punct(&kanji_to_hiragana(raw_tok));
    let tok_roma_spaced = hiragana_to_romaji(&kanji_to_hiragana(raw_tok), false);
    let tok_en_expanded = expand_english_words_to_romaji(&tok_roma_spaced);
    let tok_roma = strip_spaces_and_punct(&tok_en_expanded);
    let tok_roma_short = collapse_long_vowels(&tok_roma);
    let tok_skel = phonetic_skeleton(&tok_en_expanded);

    let mut best: i32 = 0;

    // 1. Direct normalized title / Hiragana title match
    if let Some(pos) = idx.norm_title_compact.find(&tok_compact) {
        let mut s = 1400 - (pos as i32) * 4;
        if pos == 0 {
            s += 300;
            if idx.norm_title_compact.len() == tok_compact.len() {
                s += 300;
            }
        }
        best = best.max(s.max(900));
    }
    if !tok_hira.is_empty() {
        if let Some(pos) = idx.hira_title_compact.find(&tok_hira) {
            let mut s = 1380 - (pos as i32) * 4;
            if pos == 0 {
                s += 280;
            }
            best = best.max(s.max(880));
        }
    }

    // 2. Romaji match (Hepburn, Kunrei, and Long-Vowel-Collapsed Short Romaji)
    if !tok_roma.is_empty() {
        for target in [&idx.romaji_hepburn, &idx.romaji_kunrei] {
            if let Some(pos) = target.find(&tok_roma) {
                let mut s = 1300 - (pos as i32) * 4;
                if pos == 0 {
                    s += 250;
                }
                best = best.max(s.max(850));
            }
        }
    }
    if !tok_roma_short.is_empty() {
        for target in [&idx.romaji_short, &idx.romaji_hepburn, &idx.romaji_kunrei] {
            if let Some(pos) = target.find(&tok_roma_short) {
                let mut s = 1220 - (pos as i32) * 4;
                if pos == 0 {
                    s += 220;
                }
                best = best.max(s.max(800));
            }
        }
    }

    // 3. Category & Disk Filename stem match (e.g. "touhou", "wizbcf", "dokyu", "san4")
    for candidate in [&tok_compact, &tok_hira, &tok_roma, &tok_roma_short] {
        if !candidate.is_empty() && idx.extra_blob.contains(candidate.as_str()) {
            best = best.max(750);
        }
    }

    // 4. English-to-Katakana phonetic consonant skeleton match (for tokens with >= 3 skeleton chars)
    if tok_skel.len() >= 3 && !idx.skeleton_title.is_empty() {
        if let Some(pos) = idx.skeleton_title.find(&tok_skel) {
            let mut s = 680 - (pos as i32) * 5;
            if pos == 0 {
                s += 120;
            }
            best = best.max(s.max(520));
        }
    }

    if best > 0 {
        return Some(best);
    }

    // 5. Typo-tolerant edit distance match (Damerau-Levenshtein <= 1 for len >= 4, <= 2 for len >= 7)
    let char_len = tok_roma_short.chars().count();
    if char_len >= 4 && tok_roma_short.is_ascii() {
        let max_dist = if char_len >= 7 { 2 } else { 1 };
        for word in &idx.words {
            if !word.is_ascii() {
                continue;
            }
            if word.len().abs_diff(tok_roma_short.len()) <= max_dist {
                let d = damerau_levenshtein(&tok_roma_short, word);
                if d <= max_dist {
                    best = best.max(540 - (d as i32) * 80);
                }
            } else if word.len() > tok_roma_short.len() {
                // Check prefix of longer compound word
                let prefix_len = tok_roma_short.len();
                let d = damerau_levenshtein(&tok_roma_short, &word[..prefix_len]);
                if d <= 1 {
                    best = best.max(500);
                }
            }
        }
    }

    if best > 0 {
        return Some(best);
    }

    // 6. Subsequence fuzzy match (fzf-style) for abbreviations (e.g. "wiz6", "ultuw", "eam2")
    if tok_roma_short.len() >= 3 && tok_roma_short.is_ascii() {
        if let Some(sub_score) = subsequence_fuzzy_score(&tok_roma_short, &idx.romaji_short)
            .or_else(|| subsequence_fuzzy_score(&tok_compact, &idx.norm_title_compact))
        {
            return Some(sub_score);
        }
    }

    None
}

/// Converts fullwidth ASCII/digits, Roman numerals, circled digits, halfwidth Katakana
/// (with dakuten/handakuten composition), and fullwidth Katakana into normalized lowercase
/// text with Hiragana and Shinjitai Kanji.
pub fn normalize_text(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len() + 8);
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        // 1. Halfwidth Katakana (U+FF66..=U+FF9F) with optional dakuten (ﾞ U+FF9E) / handakuten (ﾟ U+FF9F)
        if ('\u{FF66}'..='\u{FF9D}').contains(&c) {
            let next = chars.get(i + 1).copied();
            let (hira, consumed_next) = halfwidth_katakana_to_hiragana(c, next);
            out.push(hira);
            i += if consumed_next { 2 } else { 1 };
            continue;
        }
        if c == '\u{FF70}' {
            out.push('ー');
            i += 1;
            continue;
        }
        if matches!(c, '\u{FF61}'..='\u{FF65}' | '\u{FF9E}' | '\u{FF9F}') {
            out.push(' ');
            i += 1;
            continue;
        }

        // 2. Fullwidth ASCII (U+FF01..=U+FF5E) -> Halfwidth lowercase ASCII
        if ('\u{FF01}'..='\u{FF5E}').contains(&c) {
            let ascii = char::from_u32((c as u32) - 0xFEE0).unwrap_or(c);
            out.push(ascii.to_ascii_lowercase());
            i += 1;
            continue;
        }

        // 3. Fullwidth Katakana (U+30A1..=U+30F6) -> Hiragana
        if ('\u{30A1}'..='\u{30F6}').contains(&c) {
            let hira = char::from_u32((c as u32) - 0x60).unwrap_or(c);
            out.push(hira);
            i += 1;
            continue;
        }

        // 4. Roman numerals (U+2160..=U+217B) -> Arabic digits
        if let Some(num_str) = roman_numeral_to_digits(c) {
            out.push_str(num_str);
            i += 1;
            continue;
        }

        // 5. Circled digits ①..⑳ (U+2460..=U+2473)
        if ('\u{2460}'..='\u{2473}').contains(&c) {
            let n = (c as u32) - 0x2460 + 1;
            out.push_str(&n.to_string());
            i += 1;
            continue;
        }

        // 6. Kyujitai / variant Kanji -> standard Shinjitai Kanji
        let norm_kanji = normalize_kanji_variant(c);
        if norm_kanji != c {
            out.push(norm_kanji);
            i += 1;
            continue;
        }

        // 7. Separators / punctuation -> space
        if c.is_whitespace()
            || matches!(
                c,
                '・' | '～'
                    | '〜'
                    | '－'
                    | '―'
                    | '‐'
                    | '：'
                    | '／'
                    | '（'
                    | '）'
                    | '【'
                    | '】'
                    | '「'
                    | '」'
                    | '『'
                    | '』'
                    | '_'
                    | '-'
                    | '/'
                    | ':'
                    | '.'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '~'
            )
        {
            out.push(' ');
            i += 1;
            continue;
        }

        out.push(c.to_ascii_lowercase());
        i += 1;
    }

    out
}

fn normalize_kanji_variant(c: char) -> char {
    match c {
        '國' => '国',
        '靈' => '霊',
        '傳' => '伝',
        '戰' => '戦',
        '學' => '学',
        '劍' | '剱' => '剣',
        '戀' => '恋',
        '櫻' => '桜',
        '眞' => '真',
        '神' => '神',
        '獸' => '獣',
        '鬪' => '闘',
        '雙' => '双',
        '錄' => '録',
        '龍' => '竜',
        '嶋' | '嶌' => '島',
        '齋' | '齊' => '斉',
        '邊' | '邉' => '辺',
        '澤' => '沢',
        '舊' => '旧',
        '畫' => '画',
        '號' => '号',
        '變' => '変',
        '體' => '体',
        '藝' => '芸',
        '團' => '団',
        '圓' => '円',
        '壓' => '圧',
        '廣' => '広',
        '應' => '応',
        '戲' => '戯',
        '擊' => '撃',
        '斷' => '断',
        '會' => '会',
        '榮' => '栄',
        '樂' => '楽',
        '氣' => '気',
        '淨' => '浄',
        '滿' => '満',
        '潛' => '潜',
        '燈' => '灯',
        '爭' => '争',
        '獨' => '独',
        '獵' => '猟',
        '獻' => '献',
        '發' => '発',
        '盜' => '盗',
        '盡' => '尽',
        '禮' => '礼',
        '祕' => '秘',
        '絲' => '糸',
        '繪' => '絵',
        '續' => '続',
        '聲' => '声',
        '腦' => '脳',
        '萬' => '万',
        '處' => '処',
        '蟲' => '虫',
        '覺' => '覚',
        '觀' => '観',
        '譯' => '訳',
        '讀' => '読',
        '豐' => '豊',
        '賣' => '売',
        '轉' => '転',
        '輕' => '軽',
        '遙' => '遥',
        '醉' => '酔',
        '釋' => '釈',
        '鐵' => '鉄',
        '關' => '関',
        '險' => '険',
        '隱' => '隠',
        '雜' => '雑',
        '靜' => '静',
        '顯' => '顕',
        '飜' => '翻',
        '餘' => '余',
        '驅' => '駆',
        '髓' => '髄',
        '默' => '黙',
        _ => c,
    }
}

fn roman_numeral_to_digits(c: char) -> Option<&'static str> {
    match c {
        'Ⅰ' | 'ⅰ' => Some("1"),
        'Ⅱ' | 'ⅱ' => Some("2"),
        'Ⅲ' | 'ⅲ' => Some("3"),
        'Ⅳ' | 'ⅳ' => Some("4"),
        'Ⅴ' | 'ⅴ' => Some("5"),
        'Ⅵ' | 'ⅵ' => Some("6"),
        'Ⅶ' | 'ⅶ' => Some("7"),
        'Ⅷ' | 'ⅷ' => Some("8"),
        'Ⅸ' | 'ⅸ' => Some("9"),
        'Ⅹ' | 'ⅹ' => Some("10"),
        'Ⅺ' | 'ⅺ' => Some("11"),
        'Ⅻ' | 'ⅻ' => Some("12"),
        _ => None,
    }
}

fn halfwidth_katakana_to_hiragana(c: char, next: Option<char>) -> (char, bool) {
    let dakuten = next == Some('\u{FF9E}');
    let handakuten = next == Some('\u{FF9F}');
    match c {
        '\u{FF66}' => ('を', false),
        '\u{FF67}' => ('ぁ', false),
        '\u{FF68}' => ('ぃ', false),
        '\u{FF69}' => ('ぅ', false),
        '\u{FF6A}' => ('ぇ', false),
        '\u{FF6B}' => ('ぉ', false),
        '\u{FF6C}' => ('ゃ', false),
        '\u{FF6D}' => ('ゅ', false),
        '\u{FF6E}' => ('ょ', false),
        '\u{FF6F}' => ('っ', false),
        '\u{FF71}' => ('あ', false),
        '\u{FF72}' => ('い', false),
        '\u{FF73}' => {
            if dakuten {
                ('ゔ', true)
            } else {
                ('う', false)
            }
        }
        '\u{FF74}' => ('え', false),
        '\u{FF75}' => ('お', false),
        '\u{FF76}' => (if dakuten { 'が' } else { 'か' }, dakuten),
        '\u{FF77}' => (if dakuten { 'ぎ' } else { 'き' }, dakuten),
        '\u{FF78}' => (if dakuten { 'ぐ' } else { 'く' }, dakuten),
        '\u{FF79}' => (if dakuten { 'げ' } else { 'け' }, dakuten),
        '\u{FF7A}' => (if dakuten { 'ご' } else { 'こ' }, dakuten),
        '\u{FF7B}' => (if dakuten { 'ざ' } else { 'さ' }, dakuten),
        '\u{FF7C}' => (if dakuten { 'じ' } else { 'し' }, dakuten),
        '\u{FF7D}' => (if dakuten { 'ず' } else { 'す' }, dakuten),
        '\u{FF7E}' => (if dakuten { 'ぜ' } else { 'せ' }, dakuten),
        '\u{FF7F}' => (if dakuten { 'ぞ' } else { 'そ' }, dakuten),
        '\u{FF80}' => (if dakuten { 'だ' } else { 'た' }, dakuten),
        '\u{FF81}' => (if dakuten { 'ぢ' } else { 'ち' }, dakuten),
        '\u{FF82}' => (if dakuten { 'づ' } else { 'つ' }, dakuten),
        '\u{FF83}' => (if dakuten { 'で' } else { 'て' }, dakuten),
        '\u{FF84}' => (if dakuten { 'ど' } else { 'と' }, dakuten),
        '\u{FF85}' => ('な', false),
        '\u{FF86}' => ('に', false),
        '\u{FF87}' => ('ぬ', false),
        '\u{FF88}' => ('ね', false),
        '\u{FF89}' => ('の', false),
        '\u{FF8A}' => {
            if dakuten {
                ('ば', true)
            } else if handakuten {
                ('ぱ', true)
            } else {
                ('は', false)
            }
        }
        '\u{FF8B}' => {
            if dakuten {
                ('び', true)
            } else if handakuten {
                ('ぴ', true)
            } else {
                ('ひ', false)
            }
        }
        '\u{FF8C}' => {
            if dakuten {
                ('ぶ', true)
            } else if handakuten {
                ('ぷ', true)
            } else {
                ('ふ', false)
            }
        }
        '\u{FF8D}' => {
            if dakuten {
                ('べ', true)
            } else if handakuten {
                ('ぺ', true)
            } else {
                ('へ', false)
            }
        }
        '\u{FF8E}' => {
            if dakuten {
                ('ぼ', true)
            } else if handakuten {
                ('ぽ', true)
            } else {
                ('ほ', false)
            }
        }
        '\u{FF8F}' => ('ま', false),
        '\u{FF90}' => ('み', false),
        '\u{FF91}' => ('む', false),
        '\u{FF92}' => ('め', false),
        '\u{FF93}' => ('も', false),
        '\u{FF94}' => ('ゃ', false),
        '\u{FF95}' => ('ゅ', false),
        '\u{FF96}' => ('ょ', false),
        '\u{FF97}' => ('ら', false),
        '\u{FF98}' => ('り', false),
        '\u{FF99}' => ('る', false),
        '\u{FF9A}' => ('れ', false),
        '\u{FF9B}' => ('ろ', false),
        '\u{FF9C}' => ('わ', false),
        '\u{FF9D}' => ('ん', false),
        _ => (c, false),
    }
}

fn is_kanji_char(c: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&c) || c == '々'
}

fn lookup_kanji_word(word: &str) -> Option<&'static str> {
    KANJI_WORDS
        .binary_search_by_key(&word, |&(k, _)| k)
        .ok()
        .map(|i| KANJI_WORDS[i].1)
}

fn lookup_kanji_char(c: char) -> Option<&'static str> {
    KANJI_CHARS
        .binary_search_by_key(&c, |&(k, _)| k)
        .ok()
        .map(|i| KANJI_CHARS[i].1)
}

/// Converts Kanji sequences inside `text` into Hiragana using `KANJI_WORDS` (longest-prefix match)
/// and `KANJI_CHARS` fallback. Also converts Katakana to Hiragana.
pub fn kanji_to_hiragana(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() * 2);
    let mut i = 0;
    let mut last_kanji_reading: &str = "";

    while i < chars.len() {
        let c = chars[i];
        if ('\u{30A1}'..='\u{30F6}').contains(&c) {
            let hira = char::from_u32((c as u32) - 0x60).unwrap_or(c);
            out.push(hira);
            i += 1;
            continue;
        }
        if !is_kanji_char(c) {
            out.push(c);
            i += 1;
            continue;
        }

        // Try longest compound match (up to 8 chars) in KANJI_WORDS, checking both normalized
        // and original Kanji forms.
        let max_len = (chars.len() - i).min(8);
        let mut matched_len = 0;
        for len in (2..=max_len).rev() {
            let cand: String = chars[i..i + len].iter().collect();
            if let Some(reading) = lookup_kanji_word(&cand) {
                out.push_str(reading);
                last_kanji_reading = reading;
                matched_len = len;
                break;
            }
        }
        if matched_len > 0 {
            i += matched_len;
            continue;
        }

        if c == '々' && !last_kanji_reading.is_empty() {
            out.push_str(last_kanji_reading);
            i += 1;
            continue;
        }

        if let Some(reading) = lookup_kanji_char(c) {
            out.push_str(reading);
            last_kanji_reading = reading;
        } else {
            out.push(c);
        }
        i += 1;
    }

    out
}

/// Converts Hiragana text to Romaji (either Hepburn or Kunrei/Wapuro).
pub fn hiragana_to_romaji(text: &str, kunrei: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() * 2);
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();

        // Sokuon (small tsu `っ`): doubles the first consonant of the following kana
        if c == 'っ' {
            if let Some(nc) = next {
                let (next_roma, _) = kana_pair_to_romaji(nc, chars.get(i + 2).copied(), kunrei);
                if let Some(first_ch) = next_roma.chars().next() {
                    if first_ch.is_ascii_alphabetic()
                        && !matches!(first_ch, 'a' | 'i' | 'u' | 'e' | 'o')
                    {
                        out.push(first_ch);
                        i += 1;
                        continue;
                    }
                }
            }
            i += 1;
            continue;
        }

        // Prolonged sound mark (`ー`): repeats the last vowel in `out`
        if c == 'ー' {
            if let Some(last_v) = out
                .chars()
                .rev()
                .take(2)
                .find(|&ch| matches!(ch, 'a' | 'i' | 'u' | 'e' | 'o'))
            {
                out.push(last_v);
            }
            i += 1;
            continue;
        }

        let (roma, consumed) = kana_pair_to_romaji(c, next, kunrei);
        if consumed > 0 {
            out.push_str(roma);
            i += consumed;
        } else {
            out.push(c);
            i += 1;
        }
    }

    out
}

fn kana_pair_to_romaji(c: char, next: Option<char>, kunrei: bool) -> (&'static str, usize) {
    // 1. Digraphs (Youon / foreign combinations)
    if let Some(n) = next {
        let digraph: Option<&'static str> = match (c, n) {
            ('き', 'ゃ') => Some("kya"),
            ('き', 'ゅ') => Some("kyu"),
            ('き', 'ょ') => Some("kyo"),
            ('ぎ', 'ゃ') => Some("gya"),
            ('ぎ', 'ゅ') => Some("gyu"),
            ('ぎ', 'ょ') => Some("gyo"),
            ('し', 'ゃ') => Some(if kunrei { "sya" } else { "sha" }),
            ('し', 'ゅ') => Some(if kunrei { "syu" } else { "shu" }),
            ('し', 'ょ') => Some(if kunrei { "syo" } else { "sho" }),
            ('し', 'ぇ') => Some("she"),
            ('じ', 'ゃ') | ('ぢ', 'ゃ') => Some(if kunrei { "zya" } else { "ja" }),
            ('じ', 'ゅ') | ('ぢ', 'ゅ') => Some(if kunrei { "zyu" } else { "ju" }),
            ('じ', 'ょ') | ('ぢ', 'ょ') => Some(if kunrei { "zyo" } else { "jo" }),
            ('じ', 'ぇ') => Some("je"),
            ('ち', 'ゃ') => Some(if kunrei { "tya" } else { "cha" }),
            ('ち', 'ゅ') => Some(if kunrei { "tyu" } else { "chu" }),
            ('ち', 'ょ') => Some(if kunrei { "tyo" } else { "cho" }),
            ('ち', 'ぇ') => Some("che"),
            ('て', 'ぃ') => Some("ti"),
            ('で', 'ぃ') => Some("di"),
            ('て', 'ゅ') => Some("tyu"),
            ('で', 'ゅ') => Some("dyu"),
            ('と', 'ぅ') => Some("tu"),
            ('ど', 'ぅ') => Some("du"),
            ('に', 'ゃ') => Some("nya"),
            ('に', 'ゅ') => Some("nyu"),
            ('に', 'ょ') => Some("nyo"),
            ('ひ', 'ゃ') => Some("hya"),
            ('ひ', 'ゅ') => Some("hyu"),
            ('ひ', 'ょ') => Some("hyo"),
            ('び', 'ゃ') => Some("bya"),
            ('び', 'ゅ') => Some("byu"),
            ('び', 'ょ') => Some("byo"),
            ('ぴ', 'ゃ') => Some("pya"),
            ('ぴ', 'ゅ') => Some("pyu"),
            ('ぴ', 'ょ') => Some("pyo"),
            ('ふ', 'ぁ') => Some("fa"),
            ('ふ', 'ぃ') => Some("fi"),
            ('ふ', 'ぇ') => Some("fe"),
            ('ふ', 'ぉ') => Some("fo"),
            ('ふ', 'ゅ') => Some("fyu"),
            ('み', 'ゃ') => Some("mya"),
            ('み', 'ゅ') => Some("myu"),
            ('み', 'ょ') => Some("myo"),
            ('り', 'ゃ') => Some("rya"),
            ('り', 'ゅ') => Some("ryu"),
            ('り', 'ょ') => Some("ryo"),
            ('う', 'ぃ') => Some("wi"),
            ('う', 'ぇ') => Some("we"),
            ('う', 'ぉ') => Some("wo"),
            ('ゔ', 'ぁ') => Some("va"),
            ('ゔ', 'ぃ') => Some("vi"),
            ('ゔ', 'ぇ') => Some("ve"),
            ('ゔ', 'ぉ') => Some("vo"),
            ('ゔ', 'ゅ') => Some("vyu"),
            ('く', 'ぁ') | ('く', 'ゎ') => Some("kwa"),
            ('く', 'ぃ') => Some("kwi"),
            ('く', 'ぇ') => Some("kwe"),
            ('く', 'ぉ') => Some("kwo"),
            ('ぐ', 'ぁ') | ('ぐ', 'ゎ') => Some("gwa"),
            ('つ', 'ぁ') => Some("tsa"),
            ('つ', 'ぃ') => Some("tsi"),
            ('つ', 'ぇ') => Some("tse"),
            ('つ', 'ぉ') => Some("tso"),
            _ => None,
        };
        if let Some(d) = digraph {
            return (d, 2);
        }
    }

    // 2. Single Hiragana characters
    let single: Option<&'static str> = match c {
        'あ' | 'ぁ' => Some("a"),
        'い' | 'ぃ' => Some("i"),
        'う' | 'ぅ' => Some("u"),
        'え' | 'ぇ' => Some("e"),
        'お' | 'ぉ' => Some("o"),
        'か' | 'ゕ' => Some("ka"),
        'き' => Some("ki"),
        'く' => Some("ku"),
        'け' | 'ゖ' => Some("ke"),
        'こ' => Some("ko"),
        'が' => Some("ga"),
        'ぎ' => Some("gi"),
        'ぐ' => Some("gu"),
        'げ' => Some("ge"),
        'ご' => Some("go"),
        'さ' => Some("sa"),
        'し' => Some(if kunrei { "si" } else { "shi" }),
        'す' => Some("su"),
        'せ' => Some("se"),
        'そ' => Some("so"),
        'ざ' => Some("za"),
        'じ' => Some(if kunrei { "zi" } else { "ji" }),
        'ず' => Some("zu"),
        'ぜ' => Some("ze"),
        'ぞ' => Some("zo"),
        'た' => Some("ta"),
        'ち' => Some(if kunrei { "ti" } else { "chi" }),
        'つ' => Some(if kunrei { "tu" } else { "tsu" }),
        'て' => Some("te"),
        'と' => Some("to"),
        'だ' => Some("da"),
        'ぢ' => Some(if kunrei { "di" } else { "ji" }),
        'づ' => Some(if kunrei { "du" } else { "zu" }),
        'で' => Some("de"),
        'ど' => Some("do"),
        'な' => Some("na"),
        'に' => Some("ni"),
        'ぬ' => Some("nu"),
        'ね' => Some("ne"),
        'の' => Some("no"),
        'は' => Some("ha"),
        'ひ' => Some("hi"),
        'ふ' => Some(if kunrei { "hu" } else { "fu" }),
        'へ' => Some("he"),
        'ほ' => Some("ho"),
        'ば' => Some("ba"),
        'び' => Some("bi"),
        'ぶ' => Some("bu"),
        'べ' => Some("be"),
        'ぼ' => Some("bo"),
        'ぱ' => Some("pa"),
        'ぴ' => Some("pi"),
        'ぷ' => Some("pu"),
        'ぺ' => Some("pe"),
        'ぽ' => Some("po"),
        'ま' => Some("ma"),
        'み' => Some("mi"),
        'む' => Some("mu"),
        'め' => Some("me"),
        'も' => Some("mo"),
        'や' | 'ゃ' => Some("ya"),
        'ゆ' | 'ゅ' => Some("yu"),
        'よ' | 'ょ' => Some("yo"),
        'ら' => Some("ra"),
        'り' => Some("ri"),
        'る' => Some("ru"),
        'れ' => Some("re"),
        'ろ' => Some("ro"),
        'わ' | 'ゎ' => Some("wa"),
        'ゐ' => Some("wi"),
        'ゑ' => Some("we"),
        'を' => Some("o"),
        'ん' => Some("n"),
        'ゔ' => Some("vu"),
        _ => None,
    };

    if let Some(s) = single {
        (s, 1)
    } else {
        ("", 0)
    }
}

/// Collapses Japanese long vowels in Romaji (`ou` -> `o`, `uu` -> `u`, `aa` -> `a`,
/// `ii` -> `i`, `ee` -> `e`, `oo` -> `o`) so e.g. `doukyuusei` and `dokyusei`,
/// `touhou` and `toho`, `imaaju` and `imaju` normalize to the same string.
pub fn collapse_long_vowels(roma: &str) -> String {
    let bytes = roma.as_bytes();
    let mut out = String::with_capacity(roma.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if i + 1 < bytes.len() {
            let n = bytes[i + 1];
            if (b == b'o' && (n == b'u' || n == b'o'))
                || (b == b'u' && n == b'u')
                || (b == b'a' && n == b'a')
                || (b == b'i' && n == b'i')
                || (b == b'e' && n == b'e')
            {
                out.push(b as char);
                i += 2;
                continue;
            }
        }
        out.push(b as char);
        i += 1;
    }
    out
}

fn strip_spaces_and_punct(s: &str) -> String {
    s.chars()
        .filter(|&c| c.is_alphanumeric() || ('\u{3040}'..='\u{9FFF}').contains(&c))
        .collect()
}

/// Maps common English words used in PC-98 titles to their Japanese Katakana Romaji equivalents.
/// Splits adjacent ASCII alphabetic and numeric runs (e.g. `wizardry6` -> `wizardry` + `6`) so
/// attached version numbers never prevent loanword lookup.
pub fn expand_english_words_to_romaji(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for (idx, token) in s.split_whitespace().enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        let mut pos = 0;
        let bytes = token.as_bytes();
        while pos < bytes.len() {
            if bytes[pos].is_ascii_alphabetic() {
                let start = pos;
                while pos < bytes.len() && bytes[pos].is_ascii_alphabetic() {
                    pos += 1;
                }
                let word = &token[start..pos];
                if let Some(mapped) = lookup_english_loanword(word) {
                    out.push_str(mapped);
                } else {
                    out.push_str(word);
                }
            } else {
                let ch = token[pos..].chars().next().unwrap();
                out.push(ch);
                pos += ch.len_utf8();
            }
        }
    }
    out
}

fn lookup_english_loanword(word: &str) -> Option<&'static str> {
    match word {
        "island" | "islands" => Some("airando"),
        "air" => Some("ea"),
        "management" => Some("manejimento"),
        "image" | "images" => Some("imaju"),
        "wizardry" => Some("uizadori"),
        "wizard" | "wizards" => Some("uizado"),
        "winning" => Some("uiningu"),
        "wedding" => Some("uedingu"),
        "warrior" => Some("uoria"),
        "warriors" => Some("uoriazu"),
        "world" | "worlds" => Some("warudo"),
        "wars" => Some("uozu"),
        "war" => Some("uo"),
        "water" => Some("uota"),
        "white" => Some("howaito"),
        "wheel" => Some("hoiru"),
        "whisper" => Some("uisupa"),
        "witch" => Some("uicchi"),
        "wing" => Some("uingu"),
        "wings" => Some("uingusu"),
        "wind" => Some("uindo"),
        "winter" => Some("uinta"),
        "wild" => Some("wairudo"),
        "woman" => Some("uman"),
        "wolf" => Some("urufu"),
        "wood" => Some("uddo"),
        "wonder" => Some("wanda"),
        "walk" => Some("uoku"),
        "wall" => Some("uoru"),
        "watch" => Some("uocchi"),
        "knight" => Some("naito"),
        "knights" => Some("naitsu"),
        "night" => Some("naito"),
        "nights" => Some("naitsu"),
        "light" | "right" => Some("raito"),
        "fight" => Some("faito"),
        "fighter" => Some("faita"),
        "fighters" => Some("faitazu"),
        "sight" => Some("saito"),
        "might" => Some("maito"),
        "mighty" => Some("maiti"),
        "flight" => Some("furaito"),
        "bright" => Some("buraito"),
        "high" => Some("hai"),
        "heart" => Some("hato"),
        "hearts" => Some("hatsu"),
        "earth" => Some("asu"),
        "guardian" => Some("gadian"),
        "guard" => Some("gado"),
        "sword" => Some("sodo"),
        "chaos" => Some("kaosu"),
        "angel" => Some("enjeru"),
        "angels" => Some("enjeruzu"),
        "energy" => Some("enerugi"),
        "legend" => Some("rejendo"),
        "magic" => Some("majikku"),
        "magical" => Some("majikaru"),
        "dungeon" => Some("danjon"),
        "soldier" => Some("soruja"),
        "major" => Some("meja"),
        "virgin" => Some("bajin"),
        "engine" => Some("enjin"),
        "page" => Some("peji"),
        "stage" => Some("suteji"),
        "cage" => Some("keji"),
        "rage" => Some("reji"),
        "age" => Some("eji"),
        "message" => Some("messeji"),
        "village" => Some("birejji"),
        "mirage" => Some("miraju"),
        "college" | "courage" => Some("karejji"),
        "marriage" => Some("marijji"),
        "voyage" => Some("boiji"),
        "quest" => Some("kuesuto"),
        "queen" => Some("kuin"),
        "quiz" => Some("kuizu"),
        "quick" => Some("kuikku"),
        "square" => Some("sukuea"),
        "quarter" => Some("kuota"),
        "quartz" => Some("kuotsu"),
        "quality" => Some("kuoriti"),
        "princess" => Some("purinsesu"),
        "maker" => Some("meka"),
        "custom" => Some("kasutamu"),
        "mate" => Some("meito"),
        "brandish" => Some("burandisshu"),
        "rance" | "lance" => Some("ransu"),
        "alice" => Some("arisu"),
        "policenauts" => Some("porisunotsu"),
        "recall" => Some("rikoru"),
        "post" => Some("posuto"),
        "horse" => Some("hosu"),
        "card" => Some("kado"),
        "buster" => Some("basuta"),
        "busters" => Some("basutazu"),
        "emerald" => Some("emerarudo"),
        "dragon" => Some("doragon"),
        "story" => Some("sutori"),
        "stories" => Some("sutorizu"),
        "fantasy" => Some("fantaji"),
        "adventure" => Some("adobencha"),
        "monster" => Some("monsuta"),
        "monsters" => Some("monsutazu"),
        "master" => Some("masuta"),
        "masters" => Some("masutazu"),
        "crystal" => Some("kurisutaru"),
        "hero" => Some("hiro"),
        "heroes" => Some("hirozu"),
        "devil" => Some("debiru"),
        "star" => Some("suta"),
        "space" => Some("supesu"),
        "battle" => Some("batoru"),
        "castle" => Some("kyassuru"),
        "castles" => Some("kyassuruzu"),
        "little" => Some("ritoru"),
        "puzzle" => Some("pazuru"),
        "double" => Some("daburu"),
        "trouble" => Some("toraburu"),
        "chronicle" => Some("kuronikuru"),
        "miracle" => Some("mirakuru"),
        "bible" => Some("baiburu"),
        "power" => Some("pawa"),
        "super" => Some("supa"),
        "hyper" => Some("haipa"),
        "ultra" => Some("urutora"),
        "cyber" => Some("saiba"),
        "viper" => Some("baipa"),
        "sister" => Some("shisuta"),
        "sisters" => Some("shisutazu"),
        "hunter" => Some("hanta"),
        "shooter" => Some("shuta"),
        "player" => Some("pureiya"),
        "royal" => Some("roiyaru"),
        "final" => Some("fainaru"),
        "metal" => Some("metaru"),
        "mahjong" => Some("majan"),
        "rusty" => Some("rasuti"),
        "xanadu" => Some("zanadu"),
        "sorcerian" => Some("sosarian"),
        "ys" => Some("isu"),
        "farland" => Some("farando"),
        "lodoss" => Some("rodosu"),
        "illusion" => Some("irujon"),
        "graduation" => Some("sotsugyo"),
        "eve" | "ivu" | "ibu" => Some("ibu"),
        _ => None,
    }
}

/// Builds a phonetic consonant+digit skeleton so English loanwords and Japanese Romaji
/// match even when epenthetic vowels (`a, i, u, e, o`) or `l/r`, `b/v`, `c/k/s` differ.
pub fn phonetic_skeleton(input: &str) -> String {
    let expanded = expand_english_words_to_romaji(input);
    let mut s = String::with_capacity(expanded.len());
    for ch in expanded.chars() {
        if ch.is_ascii_alphanumeric() {
            s.push(ch.to_ascii_lowercase());
        }
    }
    if s.is_empty() {
        return s;
    }

    for (pat, rep) in [
        ("ph", "f"),
        ("dge", "j"),
        ("tch", "c"),
        ("sh", "s"),
        ("ch", "c"),
        ("th", "s"),
        ("ts", "t"),
        ("ck", "k"),
        ("qu", "k"),
        ("x", "ks"),
        ("ce", "se"),
        ("ci", "si"),
        ("cy", "sy"),
        ("wi", "ui"),
        ("we", "ue"),
        ("wo", "uo"),
    ] {
        s = s.replace(pat, rep);
    }

    let mapped: Vec<u8> = s
        .bytes()
        .map(|b| match b {
            b'c' | b'q' => b'k',
            b'l' => b'r',
            b'v' => b'b',
            b'z' => b's',
            other => other,
        })
        .collect();

    // Collapse adjacent identical consonants before vowel stripping
    let mut dedup = Vec::with_capacity(mapped.len());
    for &b in &mapped {
        if dedup.last().copied() != Some(b) {
            dedup.push(b);
        }
    }

    // Keep the very first character (even if a vowel), strip subsequent vowels, and collapse repeats
    let mut skel = String::with_capacity(dedup.len());
    for (idx, &b) in dedup.iter().enumerate() {
        let is_vowel = matches!(b, b'a' | b'i' | b'u' | b'e' | b'o' | b'y');
        if idx == 0 || !is_vowel {
            let ch = b as char;
            if skel.chars().last() != Some(ch) {
                skel.push(ch);
            }
        }
    }
    skel
}

/// Computes optimal Damerau-Levenshtein distance (insertions, deletions, substitutions,
/// and adjacent transpositions) between two ASCII strings.
pub fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    let m = a_bytes.len();
    let n = b_bytes.len();
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    let width = n + 1;
    let mut dp = vec![0usize; (m + 1) * width];
    for i in 0..=m {
        dp[i * width] = i;
    }
    for j in 0..=n {
        dp[j] = j;
    }

    for i in 1..=m {
        for j in 1..=n {
            let cost = usize::from(a_bytes[i - 1] != b_bytes[j - 1]);
            let del = dp[(i - 1) * width + j] + 1;
            let ins = dp[i * width + (j - 1)] + 1;
            let sub = dp[(i - 1) * width + (j - 1)] + cost;
            let mut val = del.min(ins).min(sub);

            if i > 1
                && j > 1
                && a_bytes[i - 1] == b_bytes[j - 2]
                && a_bytes[i - 2] == b_bytes[j - 1]
            {
                val = val.min(dp[(i - 2) * width + (j - 2)] + 1);
            }
            dp[i * width + j] = val;
        }
    }

    dp[m * width + n]
}

/// Computes an `fzf`-style subsequence match score if all characters of `needle` appear in order
/// inside `haystack` within a compact span (`span <= needle.len() * 2 + 3`).
pub fn subsequence_fuzzy_score(needle: &str, haystack: &str) -> Option<i32> {
    let n_bytes = needle.as_bytes();
    let h_bytes = haystack.as_bytes();
    if n_bytes.len() < 3 || n_bytes.len() > h_bytes.len() {
        return None;
    }

    // Find the tightest matching window starting at each occurrence of n_bytes[0]
    let max_span = n_bytes.len() * 2 + 3;
    let mut best_score: Option<i32> = None;

    for start in 0..h_bytes.len() {
        if h_bytes[start] != n_bytes[0] {
            continue;
        }
        let mut h_idx = start;
        let mut prev_match = start;
        let mut consecutive = 0i32;
        let mut matched_all = true;

        for &nb in &n_bytes[1..] {
            h_idx += 1;
            while h_idx < h_bytes.len() && h_bytes[h_idx] != nb {
                h_idx += 1;
            }
            if h_idx >= h_bytes.len() {
                matched_all = false;
                break;
            }
            if h_idx == prev_match + 1 {
                consecutive += 1;
            }
            prev_match = h_idx;
        }

        if !matched_all {
            break;
        }

        let span = (prev_match - start) + 1;
        if span <= max_span {
            let gaps = (span - n_bytes.len()) as i32;
            let start_bonus = if start == 0 { 60 } else { 0 };
            let score = 360 + consecutive * 25 + start_bonus - gaps * 12 - (start as i32) * 2;
            if score >= 280 {
                best_score = Some(best_score.map_or(score, |b| b.max(score)));
            }
        }
    }

    best_score
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pc98::library::Pc98BootMode;
    use std::path::PathBuf;

    fn dummy_game(title: &str, category: Option<&str>, disk: &str) -> Pc98GameEntry {
        let floppies = vec![PathBuf::from(disk)];
        let search_index = Pc98SearchIndex::build(title, category, &floppies, &[], &[]);
        Pc98GameEntry {
            id: title.to_string(),
            title: title.to_string(),
            category: category.map(|s| s.to_string()),
            root_path: PathBuf::from(title),
            boot_mode: Pc98BootMode::SelfBootFloppy,
            floppies,
            hdds: Vec::new(),
            cdroms: Vec::new(),
            search_index,
        }
    }

    #[test]
    fn test_romaji_kanji_katakana_and_english_loanword_matching() {
        let games = vec![
            dummy_game("うきうきアイランド", Some("う"), "Ukiukiia.FDI"),
            dummy_game(
                "Ｗｉｚａｒｄｒｙ６　Ｂａｎｅ　Ｏｆ　Ｔｈｅ　Ｃｏｓｍｉｃ　Ｆｏｒｇｅ",
                Some("う"),
                "WIZBCF_A.D88",
            ),
            dummy_game(
                "エアーマネジメントⅡ",
                Some("え"),
                "エアーマネジメントⅡ_A.fdi",
            ),
            dummy_game("イマージュ", Some("い"), "イマージュ_1.fdi"),
            dummy_game("同級生２", Some("と"), "DOKYU2.HDI"),
            dummy_game("三國志4PK", Some("さ"), "三國志4PK.hdi"),
            dummy_game("1_靈異伝", Some("同人/東方Project"), "1_靈異伝.hdi"),
            dummy_game("Ｕｌｔｉｍａ　ＵｎｄｅｒＷｏｒｌｄ", Some("う"), "UW_A.FDI"),
        ];

        // 1. Romaji + English loanword -> うきうきアイランド
        let r = filter_and_rank_games(&games, "ukiuki");
        assert_eq!(games[r[0]].title, "うきうきアイランド");
        let r = filter_and_rank_games(&games, "ukiuki island");
        assert_eq!(games[r[0]].title, "うきうきアイランド");
        let r = filter_and_rank_games(&games, "ウキウキ");
        assert_eq!(games[r[0]].title, "うきうきアイランド");

        // 2. Fullwidth Latin + fuzzy abbreviation -> Wizardry 6
        let r = filter_and_rank_games(&games, "wizardry 6");
        assert_eq!(
            games[r[0]].title,
            "Ｗｉｚａｒｄｒｙ６　Ｂａｎｅ　Ｏｆ　Ｔｈｅ　Ｃｏｓｍｉｃ　Ｆｏｒｇｅ"
        );
        let r = filter_and_rank_games(&games, "wiz6");
        assert_eq!(
            games[r[0]].title,
            "Ｗｉｚａｒｄｒｙ６　Ｂａｎｅ　Ｏｆ　Ｔｈｅ　Ｃｏｓｍｉｃ　Ｆｏｒｇｅ"
        );
        // Typo tolerance ("wizadry")
        let r = filter_and_rank_games(&games, "wizadry");
        assert_eq!(
            games[r[0]].title,
            "Ｗｉｚａｒｄｒｙ６　Ｂａｎｅ　Ｏｆ　Ｔｈｅ　Ｃｏｓｍｉｃ　Ｆｏｒｇｅ"
        );

        // 3. English loanwords + Roman numeral Ⅱ -> エアーマネジメントⅡ
        let r = filter_and_rank_games(&games, "air management 2");
        assert_eq!(games[r[0]].title, "エアーマネジメントⅡ");
        let r = filter_and_rank_games(&games, "eamanejimento");
        assert_eq!(games[r[0]].title, "エアーマネジメントⅡ");

        // 4. Collapsed long vowel romaji + English "image" -> イマージュ
        let r = filter_and_rank_games(&games, "imaju");
        assert_eq!(games[r[0]].title, "イマージュ");
        let r = filter_and_rank_games(&games, "image");
        assert_eq!(games[r[0]].title, "イマージュ");

        // 5. Kanji title matched by both "dokyusei" and "doukyuusei" and "どうきゅうせい" -> 同級生２
        let r = filter_and_rank_games(&games, "dokyusei");
        assert_eq!(games[r[0]].title, "同級生２");
        let r = filter_and_rank_games(&games, "doukyuusei 2");
        assert_eq!(games[r[0]].title, "同級生２");
        let r = filter_and_rank_games(&games, "どうきゅうせい");
        assert_eq!(games[r[0]].title, "同級生２");

        // 6. Kyujitai Kanji 三國志 matched by Shinjitai 三国志 and romaji "sangokushi" / "sangokusi"
        let r = filter_and_rank_games(&games, "三国志");
        assert_eq!(games[r[0]].title, "三國志4PK");
        let r = filter_and_rank_games(&games, "sangokushi");
        assert_eq!(games[r[0]].title, "三國志4PK");
        let r = filter_and_rank_games(&games, "sangokusi 4");
        assert_eq!(games[r[0]].title, "三國志4PK");

        // 7. Category & Kanji reading ("touhou" / "toho" / "reiiden") -> 1_靈異伝
        let r = filter_and_rank_games(&games, "touhou");
        assert_eq!(games[r[0]].title, "1_靈異伝");
        let r = filter_and_rank_games(&games, "reiiden");
        assert_eq!(games[r[0]].title, "1_靈異伝");

        // 8. Searching English-titled games using Japanese Katakana / Hiragana
        let r = filter_and_rank_games(&games, "ウルティマ");
        assert_eq!(games[r[0]].title, "Ｕｌｔｉｍａ　ＵｎｄｅｒＷｏｒｌｄ");
        let r = filter_and_rank_games(&games, "うるてぃま");
        assert_eq!(games[r[0]].title, "Ｕｌｔｉｍａ　ＵｎｄｅｒＷｏｒｌｄ");
        let r = filter_and_rank_games(&games, "ウィザードリィ");
        assert_eq!(
            games[r[0]].title,
            "Ｗｉｚａｒｄｒｙ６　Ｂａｎｅ　Ｏｆ　Ｔｈｅ　Ｃｏｓｍｉｃ　Ｆｏｒｇｅ"
        );
    }
}
