//! Helpers for mining-bank tags: normalization, canonical spelling and fuzzy suggestions.
//!
//! Each mined word has at most one tag (e.g. the game being played).

/// Trims `input`; returns `None` for an empty tag.
pub fn normalize_tag(input: &str) -> Option<String> {
    let trimmed = input.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Normalizes `input` and, if a known tag matches it case-insensitively, returns that
/// existing spelling instead (so "final fantasy 7" becomes "Final Fantasy 7").
pub fn canonicalize_tag(input: &str, known: &[String]) -> Option<String> {
    let tag = normalize_tag(input)?;
    let lower = tag.to_lowercase();
    Some(
        known
            .iter()
            .find(|k| k.to_lowercase() == lower)
            .cloned()
            .unwrap_or(tag),
    )
}

/// True if `tag` belongs to the filter `query`: a case-insensitive substring match, so
/// "Final Fantasy" matches "Final Fantasy 7" and "Final Fantasy 8". An empty query
/// matches everything (including untagged words).
pub fn tag_matches_filter(tag: Option<&str>, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }
    tag.map_or(false, |t| t.to_lowercase().contains(&query.to_lowercase()))
}

/// Scores how well `candidate` matches `query` (higher is better), or `None` if it does
/// not match. Exact > prefix > word-prefix > substring > in-order subsequence.
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    let q: Vec<char> = query.trim().to_lowercase().chars().collect();
    if q.is_empty() {
        return Some(0);
    }
    let c_str = candidate.to_lowercase();
    let c: Vec<char> = c_str.chars().collect();
    let q_str: String = q.iter().collect();
    let len_penalty = c.len().saturating_sub(q.len()).min(100) as i32;

    if c_str == q_str {
        return Some(10_000);
    }
    if c_str.starts_with(&q_str) {
        return Some(8_000 - len_penalty);
    }
    if let Some(byte_pos) = c_str.find(&q_str) {
        let pos = c_str[..byte_pos].chars().count();
        let at_word_start = c_str[..byte_pos]
            .chars()
            .last()
            .map_or(true, |ch| !ch.is_alphanumeric());
        let base = if at_word_start { 6_000 } else { 4_000 };
        return Some(base - (pos.min(100) as i32) * 10 - len_penalty);
    }

    // In-order subsequence, penalizing gaps between matched characters.
    let mut qi = 0;
    let mut gaps = 0i32;
    let mut last: Option<usize> = None;
    for (ci, ch) in c.iter().enumerate() {
        if qi < q.len() && *ch == q[qi] {
            if let Some(l) = last {
                gaps += (ci - l - 1) as i32;
            }
            last = Some(ci);
            qi += 1;
        }
    }
    (qi == q.len()).then(|| 2_000 - gaps.min(1_000) - len_penalty)
}

/// Returns up to `limit` known tags matching `query`, best first. `known` is expected to be
/// ordered by most recent use, which breaks ties. With an empty query, the most recently
/// used tags are returned. A tag identical to the query is not suggested.
pub fn suggest(query: &str, known: &[String], limit: usize) -> Vec<String> {
    let trimmed = query.trim();
    let mut scored: Vec<(i32, usize, &String)> = known
        .iter()
        .enumerate()
        .filter(|(_, k)| k.as_str() != trimmed)
        .filter_map(|(i, k)| fuzzy_score(trimmed, k).map(|s| (s, i, k)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, _, k)| k.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<String> {
        ["Final Fantasy 7", "Dragon Quest", "Final Fantasy 8", "Fire Emblem"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn normalize_trims_and_drops_empty() {
        assert_eq!(normalize_tag("  Grandia 2 "), Some("Grandia 2".into()));
        assert_eq!(normalize_tag("   "), None);
        assert_eq!(normalize_tag(""), None);
    }

    #[test]
    fn canonicalize_prefers_existing_spelling() {
        let k = known();
        assert_eq!(canonicalize_tag("final fantasy 7 ", &k), Some("Final Fantasy 7".into()));
        assert_eq!(canonicalize_tag("FINAL FANTASY 8", &k), Some("Final Fantasy 8".into()));
        assert_eq!(canonicalize_tag("final fantasy 9", &k), Some("final fantasy 9".into()));
        assert_eq!(canonicalize_tag(" ", &k), None);
    }

    #[test]
    fn filter_matches_subsets_case_insensitively() {
        assert!(tag_matches_filter(Some("Final Fantasy 7"), "Final Fantasy"));
        assert!(tag_matches_filter(Some("Final Fantasy 8"), "final fantasy"));
        assert!(tag_matches_filter(Some("Final Fantasy 8"), "fantasy"));
        assert!(!tag_matches_filter(Some("Dragon Quest"), "Final Fantasy"));
        assert!(!tag_matches_filter(None, "Final"));
        assert!(tag_matches_filter(None, "  "));
    }

    #[test]
    fn fuzzy_score_ranks_prefix_over_substring_over_subsequence() {
        let prefix = fuzzy_score("fin", "Final Fantasy 7").unwrap();
        let word = fuzzy_score("fan", "Final Fantasy 7").unwrap();
        let sub = fuzzy_score("ant", "Final Fantasy 7").unwrap();
        let seq = fuzzy_score("ff7", "Final Fantasy 7").unwrap();
        assert!(prefix > word && word > sub && sub > seq);
        assert_eq!(fuzzy_score("xyz", "Final Fantasy 7"), None);
        assert_eq!(fuzzy_score("7ff", "Final Fantasy 7"), None);
    }

    #[test]
    fn suggest_orders_by_score_then_recency() {
        let k = known();
        assert_eq!(suggest("final", &k, 8), vec!["Final Fantasy 7", "Final Fantasy 8"]);
        assert_eq!(suggest("ff8", &k, 8), vec!["Final Fantasy 8"]);
        assert_eq!(suggest("fin", &k, 1), vec!["Final Fantasy 7"]);
        assert_eq!(suggest("", &k, 2), vec!["Final Fantasy 7", "Dragon Quest"]);
        // Exact matches aren't suggested, but other casings are.
        assert_eq!(suggest("Dragon Quest", &k, 8), Vec::<String>::new());
        assert_eq!(suggest("dragon quest", &k, 8), vec!["Dragon Quest"]);
    }
}
