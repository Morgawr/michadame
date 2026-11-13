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

/// Pre-normalized representation of a single whitespace-delimited search query token,
/// hoisted outside the per-game scoring loop so multi-script conversions run once per query.
#[derive(Debug, Clone)]
pub struct PreparedToken {
    compact: String,
    hira: String,
    roma: String,
    roma_short: String,
    roma_short_char_len: usize,
    roma_short_is_ascii: bool,
    skel: String,
}

impl PreparedToken {
    fn from_norm_token(raw_tok: &str) -> Self {
        let compact = strip_spaces_and_punct(raw_tok);
        if compact.is_empty() {
            return Self {
                compact,
                hira: String::new(),
                roma: String::new(),
                roma_short: String::new(),
                roma_short_char_len: 0,
                roma_short_is_ascii: true,
                skel: String::new(),
            };
        }

        let hira_raw = kanji_to_hiragana(raw_tok);
        let hira = strip_spaces_and_punct(&hira_raw);
        let roma_spaced = hiragana_to_romaji(&hira_raw, false);
        let en_expanded = expand_english_words_to_romaji(&roma_spaced);
        let roma = strip_spaces_and_punct(&en_expanded);
        let roma_short = collapse_long_vowels(&roma);
        let roma_short_char_len = roma_short.chars().count();
        let roma_short_is_ascii = roma_short.is_ascii();
        let skel = phonetic_skeleton(&en_expanded);

        Self {
            compact,
            hira,
            roma,
            roma_short,
            roma_short_char_len,
            roma_short_is_ascii,
            skel,
        }
    }
}

/// Precomputed query state (normalized tokens, Romaji/Hiragana/Katakana conversions,
/// vowel-collapsed forms, phonetic skeletons, and contiguous phrase forms) shared across
/// all candidate games during `filter_and_rank_games`.
#[derive(Debug, Clone)]
pub struct PreparedQuery {
    tokens: Vec<PreparedToken>,
    joined_compact: String,
    joined_short: String,
}

impl PreparedQuery {
    pub fn new(query: &str) -> Self {
        let norm_query = normalize_text(query);
        let raw_tokens: Vec<&str> = norm_query
            .split_whitespace()
            .filter(|t| !t.is_empty())
            .collect();
        if raw_tokens.is_empty() {
            return Self {
                tokens: Vec::new(),
                joined_compact: String::new(),
                joined_short: String::new(),
            };
        }

        let tokens = raw_tokens
            .iter()
            .map(|&t| PreparedToken::from_norm_token(t))
            .collect();

        let (joined_compact, joined_short) = if raw_tokens.len() > 1 {
            let joined_en_roma = expand_english_words_to_romaji(&norm_query);
            let jc = strip_spaces_and_punct(&joined_en_roma);
            let js = collapse_long_vowels(&jc);
            (jc, js)
        } else {
            (String::new(), String::new())
        };

        Self {
            tokens,
            joined_compact,
            joined_short,
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// Scores `idx` against this pre-normalized query, reusing `dp_buf` for any
    /// Damerau-Levenshtein edit-distance calculations.
    pub fn score_index_with_buf(
        &self,
        idx: &Pc98SearchIndex,
        dp_buf: &mut Vec<usize>,
    ) -> Option<i32> {
        if self.tokens.is_empty() {
            return Some(0);
        }

        let mut total_score: i32 = 0;

        for tok in &self.tokens {
            let tok_score = score_prepared_token(idx, tok, dp_buf)?;
            total_score += tok_score;
        }

        // Bonus when a multi-token query matches contiguously as a phrase in romaji or normalized title
        if self.tokens.len() > 1 && !self.joined_compact.is_empty() {
            if idx.norm_title_compact.contains(&self.joined_compact)
                || idx.romaji_hepburn.contains(&self.joined_compact)
                || idx.romaji_short.contains(&self.joined_short)
            {
                total_score += 450;
            }
        }

        // Slight tie-breaker preference for shorter titles when match quality is otherwise equal
        let len_penalty = (idx.norm_title_compact.len() as i32).min(60);
        Some((total_score - len_penalty).max(1))
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

    let prepared = PreparedQuery::new(trimmed);
    if prepared.is_empty() {
        return (0..games.len()).collect();
    }

    let mut dp_buf = Vec::new();
    let mut scored: Vec<(usize, i32)> = games
        .iter()
        .enumerate()
        .filter_map(|(idx, game)| {
            prepared
                .score_index_with_buf(&game.search_index, &mut dp_buf)
                .map(|s| (idx, s))
        })
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
    let prepared = PreparedQuery::new(query);
    let mut dp_buf = Vec::new();
    prepared.score_index_with_buf(idx, &mut dp_buf)
}

fn score_prepared_token(
    idx: &Pc98SearchIndex,
    tok: &PreparedToken,
    dp_buf: &mut Vec<usize>,
) -> Option<i32> {
    if tok.compact.is_empty() {
        return Some(1);
    }

    let mut best: i32 = 0;

    // 1. Direct normalized title / Hiragana title match
    if let Some(pos) = idx.norm_title_compact.find(&tok.compact) {
        let mut s = 1400 - (pos as i32) * 4;
        if pos == 0 {
            s += 300;
            if idx.norm_title_compact.len() == tok.compact.len() {
                s += 300;
            }
        }
        best = best.max(s.max(900));
    }
    if !tok.hira.is_empty() {
        if let Some(pos) = idx.hira_title_compact.find(&tok.hira) {
            let mut s = 1380 - (pos as i32) * 4;
            if pos == 0 {
                s += 280;
            }
            best = best.max(s.max(880));
        }
    }

    // 2. Romaji match (Hepburn, Kunrei, and Long-Vowel-Collapsed Short Romaji)
    if !tok.roma.is_empty() {
        for target in [&idx.romaji_hepburn, &idx.romaji_kunrei] {
            if let Some(pos) = target.find(&tok.roma) {
                let mut s = 1300 - (pos as i32) * 4;
                if pos == 0 {
                    s += 250;
                }
                best = best.max(s.max(850));
            }
        }
    }
    if !tok.roma_short.is_empty() {
        for target in [&idx.romaji_short, &idx.romaji_hepburn, &idx.romaji_kunrei] {
            if let Some(pos) = target.find(&tok.roma_short) {
                let mut s = 1220 - (pos as i32) * 4;
                if pos == 0 {
                    s += 220;
                }
                best = best.max(s.max(800));
            }
        }
    }

    // 3. Category & Disk Filename stem match (e.g. "touhou", "wizbcf", "dokyu", "san4")
    for candidate in [&tok.compact, &tok.hira, &tok.roma, &tok.roma_short] {
        if !candidate.is_empty() && idx.extra_blob.contains(candidate.as_str()) {
            best = best.max(750);
        }
    }

    // 4. English-to-Katakana phonetic consonant skeleton match (for tokens with >= 3 skeleton chars)
    if tok.skel.len() >= 3 && !idx.skeleton_title.is_empty() {
        if let Some(pos) = idx.skeleton_title.find(&tok.skel) {
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
    if tok.roma_short_char_len >= 4 && tok.roma_short_is_ascii {
        let max_dist = if tok.roma_short_char_len >= 7 { 2 } else { 1 };
        for word in &idx.words {
            if !word.is_ascii() {
                continue;
            }
            if word.len().abs_diff(tok.roma_short.len()) <= max_dist {
                let d = damerau_levenshtein_with_buf(&tok.roma_short, word, dp_buf);
                if d <= max_dist {
                    best = best.max(540 - (d as i32) * 80);
                }
            } else if word.len() > tok.roma_short.len() {
                // Check prefix of longer compound word
                let prefix_len = tok.roma_short.len();
                let d = damerau_levenshtein_with_buf(&tok.roma_short, &word[..prefix_len], dp_buf);
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
    if tok.roma_short.len() >= 3 && tok.roma_short_is_ascii {
        if let Some(sub_score) = subsequence_fuzzy_score(&tok.roma_short, &idx.romaji_short)
            .or_else(|| subsequence_fuzzy_score(&tok.compact, &idx.norm_title_compact))
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
/// Runs single-pass over a stack byte buffer into a single `String::with_capacity(...)`.
pub fn phonetic_skeleton(input: &str) -> String {
    let in_bytes = input.as_bytes();
    let mut stack_buf = [0u8; 256];
    let mut heap_buf: Vec<u8> = Vec::new();
    let mut len = 0usize;

    let push_byte = |b: u8, stack: &mut [u8; 256], heap: &mut Vec<u8>, len: &mut usize| {
        if heap.is_empty() {
            if *len < stack.len() {
                stack[*len] = b;
                *len += 1;
            } else {
                heap.reserve(in_bytes.len().saturating_add(16).max(320));
                heap.extend_from_slice(&stack[..*len]);
                heap.push(b);
                *len += 1;
            }
        } else {
            heap.push(b);
            *len += 1;
        }
    };

    let mut pos = 0;
    while pos < in_bytes.len() {
        let b = in_bytes[pos];
        if b.is_ascii_alphabetic() {
            let start = pos;
            while pos < in_bytes.len() && in_bytes[pos].is_ascii_alphabetic() {
                pos += 1;
            }
            let word = &input[start..pos];
            if let Some(mapped) = lookup_english_loanword(word) {
                for &mb in mapped.as_bytes() {
                    push_byte(mb, &mut stack_buf, &mut heap_buf, &mut len);
                }
            } else {
                for &wb in word.as_bytes() {
                    push_byte(
                        wb.to_ascii_lowercase(),
                        &mut stack_buf,
                        &mut heap_buf,
                        &mut len,
                    );
                }
            }
        } else if b.is_ascii_digit() {
            push_byte(b, &mut stack_buf, &mut heap_buf, &mut len);
            pos += 1;
        } else {
            pos += 1;
        }
    }

    if len == 0 {
        return String::new();
    }

    let buf: &[u8] = if heap_buf.is_empty() {
        &stack_buf[..len]
    } else {
        &heap_buf[..len]
    };

    let mut skel = String::with_capacity(len);
    let mut prev_dedup = 0u8;
    let mut last_skel = 0u8;

    let mut emit = |raw_b: u8| {
        let b = match raw_b {
            b'c' | b'q' => b'k',
            b'l' => b'r',
            b'v' => b'b',
            b'z' => b's',
            other => other,
        };
        if b == prev_dedup {
            return;
        }
        let is_first = prev_dedup == 0;
        prev_dedup = b;
        let is_vowel = matches!(b, b'a' | b'i' | b'u' | b'e' | b'o' | b'y');
        if (is_first || !is_vowel) && last_skel != b {
            skel.push(b as char);
            last_skel = b;
        }
    };

    let mut i = 0;
    while i < buf.len() {
        let rem = &buf[i..];
        if rem.starts_with(b"dge") {
            emit(b'j');
            i += 3;
        } else if rem.starts_with(b"tch") {
            i += 3;
            if buf.get(i) == Some(&b'h') {
                i += 1;
            }
            if matches!(buf.get(i), Some(b'e' | b'i' | b'y')) {
                emit(b's');
            } else {
                emit(b'k');
            }
        } else if rem.starts_with(b"ph") {
            emit(b'f');
            i += 2;
        } else if rem.starts_with(b"sh") {
            emit(b's');
            i += 2;
        } else if rem.starts_with(b"ch") {
            i += 2;
            if matches!(buf.get(i), Some(b'e' | b'i' | b'y')) {
                emit(b's');
            } else {
                emit(b'k');
            }
        } else if rem.starts_with(b"th") {
            emit(b's');
            i += 2;
        } else if rem.starts_with(b"tsh") || rem.starts_with(b"tth") {
            emit(b't');
            i += 3;
        } else if rem.starts_with(b"ts") {
            emit(b't');
            i += 2;
        } else if rem.starts_with(b"ck") || rem.starts_with(b"qu") {
            emit(b'k');
            i += 2;
        } else if rem[0] == b'x' {
            emit(b'k');
            emit(b's');
            i += 1;
        } else if rem[0] == b'c' {
            i += 1;
            if matches!(buf.get(i), Some(b'e' | b'i' | b'y')) {
                emit(b's');
            } else {
                emit(b'k');
            }
        } else if rem[0] == b'w' && matches!(rem.get(1), Some(b'i' | b'e' | b'o')) {
            emit(b'u');
            i += 1;
        } else {
            emit(rem[0]);
            i += 1;
        }
    }

    skel
}

/// Computes optimal Damerau-Levenshtein distance (insertions, deletions, substitutions,
/// and adjacent transpositions) between two ASCII strings.
pub fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let mut scratch = Vec::new();
    damerau_levenshtein_with_buf(a, b, &mut scratch)
}

/// Computes optimal string-alignment Damerau-Levenshtein distance reusing `scratch`
/// (3 rotating rows of length `b.len() + 1`) so repeated candidate comparisons do zero allocations.
pub fn damerau_levenshtein_with_buf(a: &str, b: &str, scratch: &mut Vec<usize>) -> usize {
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
    let needed = 3 * width;
    if scratch.len() < needed {
        scratch.resize(needed, 0);
    }

    let (mut prev2, rest) = scratch[..needed].split_at_mut(width);
    let (mut prev1, mut curr) = rest.split_at_mut(width);

    for (j, slot) in prev1.iter_mut().enumerate() {
        *slot = j;
    }

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = usize::from(a_bytes[i - 1] != b_bytes[j - 1]);
            let del = prev1[j] + 1;
            let ins = curr[j - 1] + 1;
            let sub = prev1[j - 1] + cost;
            let mut val = del.min(ins).min(sub);

            if i > 1
                && j > 1
                && a_bytes[i - 1] == b_bytes[j - 2]
                && a_bytes[i - 2] == b_bytes[j - 1]
            {
                val = val.min(prev2[j - 2] + 1);
            }
            curr[j] = val;
        }
        std::mem::swap(&mut prev2, &mut prev1);
        std::mem::swap(&mut prev1, &mut curr);
    }

    prev1[n]
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

    fn reference_multipass_phonetic_skeleton(input: &str) -> String {
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
        let mut dedup = Vec::with_capacity(mapped.len());
        for &b in &mapped {
            if dedup.last().copied() != Some(b) {
                dedup.push(b);
            }
        }
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

    fn reference_2d_damerau_levenshtein(a: &str, b: &str) -> usize {
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
                let mut val = (dp[(i - 1) * width + j] + 1)
                    .min(dp[i * width + (j - 1)] + 1)
                    .min(dp[(i - 1) * width + (j - 1)] + cost);
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

    #[test]
    fn test_single_pass_phonetic_skeleton_matches_multipass_reference() {
        let long_input = "wizardry6 phantasmagoria bridge witch xanadu queen ".repeat(8);
        let samples = [
            "",
            "   ---   ",
            "wizardry6",
            "Wizardry 6: Bane of the Cosmic Forge",
            "kitchen",
            "kicchin",
            "witch",
            "uicchi",
            "bridge",
            "phantasy star",
            "chronicle",
            "queen quest",
            "xanadu",
            "cyber",
            "centurion",
            "cinder",
            "window",
            "wedding",
            "world",
            "wisp",
            "tsukihime",
            "tsh",
            "tth",
            "uuto",
            "aiueo",
            "koko",
            "uccello",
            long_input.as_str(),
        ];

        for sample in samples {
            assert_eq!(
                phonetic_skeleton(sample),
                reference_multipass_phonetic_skeleton(sample),
                "phonetic_skeleton mismatch for input {sample:?}"
            );
        }

        for &(_, reading) in KANJI_WORDS.iter().take(200) {
            let roma = hiragana_to_romaji(reading, false);
            assert_eq!(
                phonetic_skeleton(&roma),
                reference_multipass_phonetic_skeleton(&roma),
                "phonetic_skeleton mismatch for romaji {roma:?}"
            );
        }
    }

    #[test]
    fn test_reused_dp_buffer_matches_2d_damerau_levenshtein_and_prepared_query() {
        let pairs = [
            ("", ""),
            ("", "wizardry"),
            ("wizardry", ""),
            ("a", "b"),
            ("ab", "ba"),
            ("wizadry", "wizardry"),
            ("brandish", "bradnish"),
            ("sangokushi", "sangokusi"),
            ("policenauts", "policenuats"),
            ("dokyusei", "doukyuusei"),
            ("rusty", "rasty"),
            ("sorcerian", "socerian"),
            ("xanadu", "xandau"),
        ];

        let mut shared_buf = Vec::new();
        for (a, b) in pairs {
            let expected = reference_2d_damerau_levenshtein(a, b);
            assert_eq!(
                damerau_levenshtein_with_buf(a, b, &mut shared_buf),
                expected,
                "forward mismatch for ({a:?}, {b:?})"
            );
            assert_eq!(
                damerau_levenshtein_with_buf(b, a, &mut shared_buf),
                reference_2d_damerau_levenshtein(b, a),
                "reverse mismatch for ({b:?}, {a:?})"
            );
        }

        let games = vec![
            dummy_game("うきうきアイランド", Some("う"), "Ukiukiia.FDI"),
            dummy_game(
                "Ｗｉｚａｒｄｒｙ６　Ｂａｎｅ　Ｏｆ　Ｔｈｅ　Ｃｏｓｍｉｃ　Ｆｏｒｇｅ",
                Some("う"),
                "WIZBCF_A.D88",
            ),
            dummy_game("ブランディッシュ２", Some("ふ"), "BR2_A.HDI"),
            dummy_game("同級生２", Some("と"), "DOKYU2.HDI"),
            dummy_game("ポリスノーツ", Some("ほ"), "PN_CD.CUE"),
        ];

        for query in [
            "wizadry 6",
            "bradnish 2",
            "policenuats",
            "ukiuki island",
            "doukyuusei 2",
            "   ",
        ] {
            let ranked = filter_and_rank_games(&games, query);
            let trimmed = query.trim();
            if trimmed.is_empty() {
                assert_eq!(ranked, (0..games.len()).collect::<Vec<_>>());
            } else {
                let prepared = PreparedQuery::new(trimmed);
                let mut buf = Vec::new();
                for (i, g) in games.iter().enumerate() {
                    assert_eq!(
                        prepared.score_index_with_buf(&g.search_index, &mut buf),
                        match_game_score(g, trimmed),
                        "PreparedQuery score mismatch on game {i} for query {query:?}"
                    );
                }
            }
        }
    }
}
