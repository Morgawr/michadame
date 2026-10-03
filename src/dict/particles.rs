//! Particle-swap matching for set expressions.
//!
//! Many Japanese expressions appear in text with a different particle than the
//! dictionary headword, e.g. `突拍子もない` written as `突拍子がない` / `突拍子はない`,
//! or `目の無い` written as `目が無い`.
//!
//! Instead of brute-forcing every particle combination against SQLite (3^k variants),
//! we build a small in-memory index of all dictionary *expressions* (`exp`) whose
//! headword or reading contains an interior swappable particle. Each one is stored under
//! a "masked" key where every interior particle is replaced by a placeholder:
//!
//! ```text
//! 突拍子もない      -> 突拍子●ない
//! とっぴょうしもない -> とっぴょうし●ない
//! ```
//!
//! At lookup time a candidate is masked the same way and a single hash lookup tells us
//! which real dictionary forms it could correspond to. Only kana particle positions are
//! wildcarded; every other character (kanji or kana) must match exactly, so kanji text
//! matches kanji headwords and kana text matches kana readings.

use super::database::DictDatabase;
use super::models::DeinflectionCandidate;
use std::collections::{HashMap, HashSet};

/// Particles that are considered interchangeable inside set expressions.
pub const SWAPPABLE_PARTICLES: [char; 4] = ['は', 'が', 'も', 'の'];

/// Placeholder used in masked keys. Never occurs in real dictionary text.
const PLACEHOLDER: char = '\u{FFFC}';

#[inline]
fn is_swappable(c: char) -> bool {
    SWAPPABLE_PARTICLES.contains(&c)
}

/// Masks every *interior* swappable particle (never the first or last character).
/// Returns `None` if the string has no interior swappable particle, so callers can
/// cheaply skip candidates that can't possibly benefit from a swap.
pub fn particle_mask(s: &str) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 3 {
        return None;
    }
    let last = chars.len() - 1;
    let mut found = false;
    let masked: String = chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            if i > 0 && i < last && is_swappable(c) {
                found = true;
                PLACEHOLDER
            } else {
                c
            }
        })
        .collect();
    found.then_some(masked)
}

/// Describes the particle substitutions between `from` (text) and `to` (dictionary form),
/// e.g. `"が→も"`. Both strings must have identical masks.
fn describe_swap(from: &str, to: &str) -> Option<String> {
    let parts: Vec<String> = from
        .chars()
        .zip(to.chars())
        .filter(|(a, b)| a != b)
        .map(|(a, b)| format!("{a}→{b}"))
        .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// In-memory index of expressions with swappable interior particles.
#[derive(Debug, Default)]
pub struct ParticleIndex {
    /// Masked key -> distinct dictionary surface forms (headwords or readings) with that mask.
    by_mask: HashMap<String, Vec<String>>,
    /// Allowed `(term, reading)` pairs; used to keep only `exp` entries in swapped results.
    allowed: HashSet<(String, String)>,
}

impl ParticleIndex {
    /// Builds an index from `(term, reading)` pairs of expression entries.
    pub fn from_pairs<I: IntoIterator<Item = (String, String)>>(pairs: I) -> Self {
        let mut index = Self::default();
        for (term, reading) in pairs {
            for surface in [&term, &reading] {
                if let Some(mask) = particle_mask(surface) {
                    let forms = index.by_mask.entry(mask).or_default();
                    if !forms.iter().any(|f| f == surface) {
                        forms.push(surface.clone());
                    }
                }
            }
            index.allowed.insert((term, reading));
        }
        index
    }

    /// Builds the index from the dictionary. Only entries tagged as expressions (`exp`)
    /// with an interior swappable particle in their headword or reading are included.
    pub fn build(db: &DictDatabase) -> anyhow::Result<Self> {
        let mut stmt = db.conn.prepare(
            "SELECT term, reading FROM terms
             WHERE (term GLOB '?*[はがもの]?*' OR reading GLOB '?*[はがもの]?*')
               AND instr(glossary, '\"code\":\"exp\"') > 0",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let pairs: Vec<(String, String)> = rows.filter_map(|r| r.ok()).collect();
        Ok(Self::from_pairs(pairs))
    }

    pub fn len(&self) -> usize {
        self.allowed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.allowed.is_empty()
    }

    /// Returns true if `(term, reading)` is an indexed expression entry.
    pub fn is_allowed(&self, term: &str, reading: &str) -> bool {
        self.allowed.contains(&(term.to_string(), reading.to_string()))
    }

    /// Generates particle-swapped candidates for the given deinflection candidates.
    /// Each swapped candidate keeps the deinflection rules/reasons and gains an extra
    /// reason such as `"が→も"`. Candidates identical to the input are skipped
    /// (those are already handled by the normal exact lookup).
    pub fn swapped_candidates(&self, candidates: &[DeinflectionCandidate]) -> Vec<DeinflectionCandidate> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for cand in candidates {
            let Some(mask) = particle_mask(&cand.term) else {
                continue;
            };
            let Some(forms) = self.by_mask.get(&mask) else {
                continue;
            };
            for form in forms {
                if form == &cand.term {
                    continue;
                }
                let Some(swap) = describe_swap(&cand.term, form) else {
                    continue;
                };
                if !seen.insert((form.clone(), cand.rules)) {
                    continue;
                }
                let mut reasons = cand.reasons.clone();
                reasons.push(swap);
                out.push(DeinflectionCandidate {
                    term: form.clone(),
                    rules: cand.rules,
                    reasons,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(term: &str) -> DeinflectionCandidate {
        DeinflectionCandidate {
            term: term.to_string(),
            rules: 0,
            reasons: Vec::new(),
        }
    }

    #[test]
    fn test_particle_mask_interior_only() {
        assert_eq!(particle_mask("突拍子もない"), Some("突拍子\u{FFFC}ない".to_string()));
        assert_eq!(particle_mask("突拍子がない"), particle_mask("突拍子はない"));
        // First/last characters are never masked
        assert_eq!(particle_mask("はなす"), None);
        assert_eq!(particle_mask("あるの"), None);
        // Too short
        assert_eq!(particle_mask("気が"), None);
        // No particle at all
        assert_eq!(particle_mask("食べる"), None);
    }

    #[test]
    fn test_kanji_never_wildcarded() {
        // Kanji text must not match a kana reading and vice versa
        assert_ne!(particle_mask("突拍子がない"), particle_mask("とっぴょうしもない"));
        assert_eq!(particle_mask("とっぴょうしがない"), particle_mask("とっぴょうしもない"));
    }

    #[test]
    fn test_swapped_candidates_kanji_and_kana() {
        let index = ParticleIndex::from_pairs(vec![
            ("突拍子もない".to_string(), "とっぴょうしもない".to_string()),
            ("目の無い".to_string(), "めのない".to_string()),
        ]);

        let out = index.swapped_candidates(&[cand("突拍子はない")]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].term, "突拍子もない");
        assert_eq!(out[0].reasons, vec!["は→も"]);

        let out = index.swapped_candidates(&[cand("とっぴょうしがない")]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].term, "とっぴょうしもない");
        assert_eq!(out[0].reasons, vec!["が→も"]);

        let out = index.swapped_candidates(&[cand("目が無い")]);
        assert_eq!(out[0].term, "目の無い");
        assert_eq!(out[0].reasons, vec!["が→の"]);

        // Exact form is not re-emitted
        assert!(index.swapped_candidates(&[cand("突拍子もない")]).is_empty());
        // Unrelated text
        assert!(index.swapped_candidates(&[cand("食べがない")]).is_empty());
    }

    #[test]
    fn test_swapped_candidates_preserve_deinflection() {
        let index = ParticleIndex::from_pairs(vec![(
            "突拍子もない".to_string(),
            "とっぴょうしもない".to_string(),
        )]);
        let c = DeinflectionCandidate {
            term: "突拍子がない".to_string(),
            rules: crate::dict::deinflect::RULE_ADJ_I,
            reasons: vec!["past".to_string()],
        };
        let out = index.swapped_candidates(&[c]);
        assert_eq!(out[0].rules, crate::dict::deinflect::RULE_ADJ_I);
        assert_eq!(out[0].reasons, vec!["past", "が→も"]);
    }
}
