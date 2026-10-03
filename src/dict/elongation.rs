//! Emphatic elongation normalization.
//!
//! Casual / stylized text stretches words for emphasis using `ー`, `〜`/`～`/`~` and
//! (runs of) `っ`/`ッ`, e.g. `ぜっっっったい` (絶対), `ぜーんぜん` (全然), `い〜っぱい` (いっぱい),
//! `すっごい` (すごい). The dictionary only stores the plain spelling.
//!
//! Each run of marker characters (not at the start of the string) gets a set of options:
//!
//! | run                  | options (priority order)        | example                    |
//! |----------------------|---------------------------------|----------------------------|
//! | single `っ`/`ッ`     | keep, remove                    | すっごい → すごい          |
//! | 2+ `っ`/`ッ`         | collapse to one, remove         | ぜっっったい → ぜったい    |
//! | `ー` run             | keep, remove                    | ぜーんぜん → ぜんぜん      |
//! | `〜`/`～`/`~` run    | remove, keep                    | い〜っぱい → いっぱい      |
//!
//! "keep" on a `ー`/`〜` run lets the result be further expanded by
//! [`long_vowel_variants`](super::long_vowel::long_vowel_variants), so mixed cases like
//! `ほんとーっに` → `ほんとうに` or `カンケ〜なーい` → `カンケイない` also resolve.
//!
//! Variants are the cartesian product over all runs (original string excluded), capped to
//! keep lookups cheap.

use super::long_vowel::long_vowel_variants;

/// Maximum number of marker runs expanded per string.
const MAX_EXPANDED_RUNS: usize = 4;
/// Hard cap on the number of variants returned.
const MAX_VARIANTS: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Sokuon,
    LongMark,
    Wave,
}

fn kind_of(c: char) -> Option<Kind> {
    match c {
        'っ' | 'ッ' | 'ｯ' => Some(Kind::Sokuon),
        'ー' | 'ｰ' => Some(Kind::LongMark),
        '〜' | '～' | '~' => Some(Kind::Wave),
        _ => None,
    }
}

/// Generates spelling variants of `s` with emphatic elongation markers collapsed/removed.
///
/// Returns `(variant, reason)` pairs in priority order, excluding the original string.
/// Returns an empty vec if `s` contains no expandable marker.
pub fn elongation_variants(s: &str) -> Vec<(String, String)> {
    if !s.chars().any(|c| kind_of(c).is_some()) {
        return Vec::new();
    }

    enum Seg {
        Lit(String),
        /// Options as (replacement text, reason or None if unchanged).
        Run(Vec<(String, Option<String>)>),
    }

    let chars: Vec<char> = s.chars().collect();
    let mut segs: Vec<Seg> = Vec::new();
    let mut runs = 0usize;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let kind = kind_of(c);
        // Markers at the very start have nothing to elongate; keep them literal.
        if let (Some(kind), true) = (kind, i > 0) {
            let mut j = i;
            while j < chars.len() && kind_of(chars[j]) == Some(kind) {
                j += 1;
            }
            let original: String = chars[i..j].iter().collect();
            if runs < MAX_EXPANDED_RUNS {
                let removed = (String::new(), Some(format!("{original}→∅")));
                let options = match kind {
                    Kind::Sokuon if j - i == 1 => vec![(original.clone(), None), removed],
                    Kind::Sokuon => {
                        let one = chars[i].to_string();
                        vec![(one.clone(), Some(format!("{original}→{one}"))), removed]
                    }
                    Kind::LongMark => vec![(original.clone(), None), removed],
                    Kind::Wave => vec![removed, (original.clone(), None)],
                };
                segs.push(Seg::Run(options));
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
            Seg::Run(options) => {
                let mut next = Vec::with_capacity(acc.len() * options.len());
                for (text, reasons) in &acc {
                    for (rep, reason) in options {
                        let mut t = text.clone();
                        t.push_str(rep);
                        let mut r = reasons.clone();
                        if let Some(reason) = reason {
                            r.push(reason.clone());
                        }
                        next.push((t, r));
                    }
                }
                acc = next;
            }
        }
    }

    let mut out: Vec<(String, String)> = Vec::new();
    let push = |out: &mut Vec<(String, String)>, t: String, r: String| {
        if !t.is_empty() && t != s && !out.iter().any(|(o, _)| *o == t) {
            out.push((t, r));
        }
    };
    for (text, reasons) in acc {
        if reasons.is_empty() {
            // Unchanged surface (only "keep" options chosen).
            continue;
        }
        let reason = reasons.join(", ");
        // Remaining ー runs can additionally be resolved as long vowels.
        let lv = long_vowel_variants(&text);
        push(&mut out, text, reason.clone());
        for (v, lv_reason) in lv {
            push(&mut out, v, format!("{reason}, {lv_reason}"));
        }
        if out.len() >= MAX_VARIANTS {
            out.truncate(MAX_VARIANTS);
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variants(s: &str) -> Vec<String> {
        elongation_variants(s).into_iter().map(|(v, _)| v).collect()
    }

    #[test]
    fn test_sokuon_runs() {
        let v = variants("ぜっっっっっったい");
        assert_eq!(v[0], "ぜったい");
        assert!(v.contains(&"ぜたい".to_string()));
        assert_eq!(variants("すっごい"), vec!["すごい"]);
        assert_eq!(variants("ズッッキュン")[0], "ズッキュン");
    }

    #[test]
    fn test_long_mark_removed() {
        assert!(variants("ぜーんぜん").contains(&"ぜんぜん".to_string()));
        assert!(variants("すごーーい").contains(&"すごい".to_string()));
    }

    #[test]
    fn test_wave_dash() {
        assert_eq!(variants("い〜っぱい")[0], "いっぱい");
        assert!(variants("すご～い").contains(&"すごい".to_string()));
        assert!(variants("ぜ~んぜん").contains(&"ぜんぜん".to_string()));
    }

    #[test]
    fn test_mixed_with_long_vowel() {
        assert!(variants("ほんとーっに").contains(&"ほんとうに".to_string()));
        assert!(variants("ぜーったい").contains(&"ぜったい".to_string()));
    }

    #[test]
    fn test_no_op_cases() {
        // A single っ is only offered for removal (no-op variant excluded)
        assert_eq!(variants("ぜったい"), vec!["ぜたい"]);
        assert!(elongation_variants("ぜんぜん").is_empty());
        // Leading markers are kept literal
        assert!(elongation_variants("〜ね").is_empty());
        assert!(elongation_variants("っ").is_empty());
    }

    #[test]
    fn test_reason() {
        let v = elongation_variants("ぜっっったい");
        assert_eq!(v[0], ("ぜったい".to_string(), "っっっ→っ".to_string()));
    }
}
