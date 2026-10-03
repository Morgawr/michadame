//! Long-vowel mark (ー) normalization.
//!
//! Casual / stylized text often writes long vowels with the chōonpu `ー` even in hiragana,
//! e.g. `どーせ` (どうせ), `せんせー` (せんせい), `ねー` (ねえ), `ほんとー` (ほんとう),
//! while the dictionary stores the standard spelling.
//!
//! Rule: a run of `ー` is replaced by the vowel kana matching the vowel of the preceding kana.
//!
//! | preceding vowel | replacements (in priority order) | example                  |
//! |-----------------|----------------------------------|--------------------------|
//! | a  (か, ゃ, …)  | あ                               | おかーさん → おかあさん  |
//! | i  (き, …)      | い                               | ちーさい → ちいさい      |
//! | u  (く, ゅ, …)  | う                               | すーじ → すうじ          |
//! | e  (け, …)      | い, え                           | せんせー → せんせい, ねー → ねえ |
//! | o  (こ, ょ, …)  | う, お                           | どーせ → どうせ, とーり → とおり |
//!
//! The replacement is emitted in the same script as the preceding kana (hiragana/katakana).
//! Variants are the cartesian product over each `ー` run, capped to keep lookups cheap.

/// Maximum number of `ー` runs expanded per string (2^3 = at most 8 variants).
const MAX_EXPANDED_RUNS: usize = 3;

fn is_long_mark(c: char) -> bool {
    matches!(c, 'ー' | 'ｰ')
}

fn to_hiragana(c: char) -> char {
    let u = c as u32;
    if (0x30A1..=0x30F6).contains(&u) {
        char::from_u32(u - 0x60).unwrap_or(c)
    } else {
        c
    }
}

fn is_katakana(c: char) -> bool {
    (0x30A1..=0x30FA).contains(&(c as u32))
}

fn to_katakana(c: char) -> char {
    let u = c as u32;
    if (0x3041..=0x3096).contains(&u) {
        char::from_u32(u + 0x60).unwrap_or(c)
    } else {
        c
    }
}

/// Returns the vowel-kana replacements for a `ー` following `prev`, in priority order.
fn replacements_for(prev: char) -> &'static [char] {
    match to_hiragana(prev) {
        'あ' | 'か' | 'さ' | 'た' | 'な' | 'は' | 'ま' | 'や' | 'ら' | 'わ' | 'が' | 'ざ' | 'だ'
        | 'ば' | 'ぱ' | 'ぁ' | 'ゃ' | 'ゎ' => &['あ'],
        'い' | 'き' | 'し' | 'ち' | 'に' | 'ひ' | 'み' | 'り' | 'ぎ' | 'じ' | 'ぢ' | 'び' | 'ぴ'
        | 'ぃ' => &['い'],
        'う' | 'く' | 'す' | 'つ' | 'ぬ' | 'ふ' | 'む' | 'ゆ' | 'る' | 'ぐ' | 'ず' | 'づ' | 'ぶ'
        | 'ぷ' | 'ぅ' | 'ゅ' | 'ゔ' => &['う'],
        'え' | 'け' | 'せ' | 'て' | 'ね' | 'へ' | 'め' | 'れ' | 'げ' | 'ぜ' | 'で' | 'べ' | 'ぺ'
        | 'ぇ' => &['い', 'え'],
        'お' | 'こ' | 'そ' | 'と' | 'の' | 'ほ' | 'も' | 'よ' | 'ろ' | 'を' | 'ご' | 'ぞ' | 'ど'
        | 'ぼ' | 'ぽ' | 'ぉ' | 'ょ' => &['う', 'お'],
        _ => &[],
    }
}

/// Generates spelling variants of `s` with `ー` runs replaced by vowel kana.
///
/// Returns `(variant, reason)` pairs, excluding the original string. `reason` describes the
/// substitutions, e.g. `"ー→う"`. Returns an empty vec if `s` has no expandable `ー`.
pub fn long_vowel_variants(s: &str) -> Vec<(String, String)> {
    if !s.chars().any(is_long_mark) {
        return Vec::new();
    }

    // Split into segments: literal text and expandable ー runs with their options.
    enum Seg {
        Lit(String),
        Run { original: String, options: Vec<char> },
    }

    let chars: Vec<char> = s.chars().collect();
    let mut segs: Vec<Seg> = Vec::new();
    let mut runs = 0usize;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if is_long_mark(c) && i > 0 {
            let mut j = i;
            while j < chars.len() && is_long_mark(chars[j]) {
                j += 1;
            }
            let original: String = chars[i..j].iter().collect();
            let prev = chars[i - 1];
            let opts = replacements_for(prev);
            if runs < MAX_EXPANDED_RUNS && !opts.is_empty() {
                let kata = is_katakana(prev);
                let options = opts
                    .iter()
                    .map(|&o| if kata { to_katakana(o) } else { o })
                    .collect();
                segs.push(Seg::Run { original, options });
                runs += 1;
            } else {
                segs.push(Seg::Lit(original));
            }
            i = j;
        } else {
            match segs.last_mut() {
                Some(Seg::Lit(lit)) => lit.push(c),
                _ => segs.push(Seg::Lit(c.to_string())),
            }
            i += 1;
        }
    }

    if runs == 0 {
        return Vec::new();
    }

    // Cartesian product, preserving priority order (first option of each run first).
    let mut acc: Vec<(String, Vec<String>)> = vec![(String::new(), Vec::new())];
    for seg in &segs {
        match seg {
            Seg::Lit(lit) => {
                for (text, _) in acc.iter_mut() {
                    text.push_str(lit);
                }
            }
            Seg::Run { original, options } => {
                let mut next = Vec::with_capacity(acc.len() * options.len());
                for (text, reasons) in &acc {
                    for &o in options {
                        let mut t = text.clone();
                        t.push(o);
                        let mut r = reasons.clone();
                        r.push(format!("{}→{}", original, o));
                        next.push((t, r));
                    }
                }
                acc = next;
            }
        }
    }

    acc.into_iter()
        .filter(|(t, _)| t != s)
        .map(|(t, r)| (t, r.join(", ")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variants(s: &str) -> Vec<String> {
        long_vowel_variants(s).into_iter().map(|(v, _)| v).collect()
    }

    #[test]
    fn test_basic_vowel_rows() {
        assert_eq!(variants("どーせ"), vec!["どうせ", "どおせ"]);
        assert_eq!(variants("せんせー"), vec!["せんせい", "せんせえ"]);
        assert_eq!(variants("ねー"), vec!["ねい", "ねえ"]);
        assert_eq!(variants("おかーさん"), vec!["おかあさん"]);
        assert_eq!(variants("ちーさい"), vec!["ちいさい"]);
        assert_eq!(variants("すーじ"), vec!["すうじ"]);
        assert_eq!(variants("とーり"), vec!["とうり", "とおり"]);
    }

    #[test]
    fn test_youon_and_runs() {
        // Small ゃゅょ carry the vowel
        assert_eq!(variants("きょー"), vec!["きょう", "きょお"]);
        assert_eq!(variants("ちゃー"), vec!["ちゃあ"]);
        // A run of several ー collapses into one vowel
        assert_eq!(variants("すごーーい"), vec!["すごうい", "すごおい"]);
    }

    #[test]
    fn test_reason_and_no_op_cases() {
        let v = long_vowel_variants("どーせ");
        assert_eq!(v[0], ("どうせ".to_string(), "ー→う".to_string()));
        // Nothing to expand
        assert!(long_vowel_variants("どうせ").is_empty());
        // Leading ー or ー after ん has no vowel to copy
        assert!(long_vowel_variants("ーあ").is_empty());
        assert!(long_vowel_variants("んー").is_empty());
    }

    #[test]
    fn test_katakana_keeps_script() {
        assert_eq!(variants("センセー"), vec!["センセイ", "センセエ"]);
    }

    #[test]
    fn test_variant_cap() {
        // 4 expandable o-runs -> only first 3 expanded -> 2^3 = 8 variants
        assert_eq!(variants("どーどーどーどー").len(), 8);
    }
}
