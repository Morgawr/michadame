use super::models::DeinflectionCandidate;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

pub const RULE_V1: u32 = 1 << 0;
pub const RULE_V5: u32 = 1 << 1;
pub const RULE_VS: u32 = 1 << 2;
pub const RULE_VK: u32 = 1 << 3;
pub const RULE_VZ: u32 = 1 << 4;
pub const RULE_ADJ_I: u32 = 1 << 5;
pub const RULE_IRU: u32 = 1 << 6;

/// Parses a space-separated rule string (from Yomitan term_bank) into a bitmask.
pub fn parse_rule_flags(rules_str: &str) -> u32 {
    let mut flags = 0;
    for rule in rules_str.split_whitespace() {
        match rule {
            "v1" => flags |= RULE_V1,
            "v5" => flags |= RULE_V5,
            "vs" => flags |= RULE_VS,
            "vk" => flags |= RULE_VK,
            "vz" => flags |= RULE_VZ,
            "adj-i" => flags |= RULE_ADJ_I,
            "iru" => flags |= RULE_IRU,
            _ => {
                // If it starts with v5 (e.g. v5k, v5s, v5r), treat as v5
                if rule.starts_with("v5") {
                    flags |= RULE_V5;
                } else if rule.starts_with("v1") {
                    flags |= RULE_V1;
                }
            }
        }
    }
    flags
}

#[derive(Debug, Deserialize)]
struct RawRule {
    #[serde(rename = "kanaIn")]
    kana_in: String,
    #[serde(rename = "kanaOut")]
    kana_out: String,
    #[serde(rename = "rulesIn")]
    rules_in: Vec<String>,
    #[serde(rename = "rulesOut")]
    rules_out: Vec<String>,
}

#[derive(Clone, Debug)]
struct VariantRule {
    kana_in: String,
    kana_out: String,
    rules_in: u32,
    rules_out: u32,
}

#[derive(Clone, Debug)]
struct ReasonGroup {
    reason: String,
    variants: Vec<VariantRule>,
}

pub struct Deinflector {
    reasons: Vec<ReasonGroup>,
}

impl Deinflector {
    pub fn new() -> Self {
        let raw_json = include_str!("deinflect_data.json");
        Self::from_json(raw_json).unwrap_or_else(|e| {
            tracing::error!("Failed to parse embedded deinflect data: {e}");
            Self { reasons: Vec::new() }
        })
    }

    pub fn from_json(json_str: &str) -> Result<Self, serde_json::Error> {
        let parsed: HashMap<String, Vec<RawRule>> = serde_json::from_str(json_str)?;
        let mut reasons = Vec::with_capacity(parsed.len());

        for (reason_name, raw_variants) in parsed {
            let mut variants = Vec::with_capacity(raw_variants.len());
            for r in raw_variants {
                let mut r_in = 0;
                for tag in &r.rules_in {
                    r_in |= parse_rule_flags(tag);
                }
                let mut r_out = 0;
                for tag in &r.rules_out {
                    r_out |= parse_rule_flags(tag);
                }
                variants.push(VariantRule {
                    kana_in: r.kana_in,
                    kana_out: r.kana_out,
                    rules_in: r_in,
                    rules_out: r_out,
                });
            }
            reasons.push(ReasonGroup {
                reason: reason_name,
                variants,
            });
        }

        Ok(Self { reasons })
    }

    /// Deconjugates a Japanese word or expression into candidate base forms
    /// along with their required rule flags and inflection reason chains.
    pub fn deinflect(&self, source: &str) -> Vec<DeinflectionCandidate> {
        let mut results = vec![DeinflectionCandidate {
            term: source.to_string(),
            rules: 0,
            reasons: Vec::new(),
        }];

        let mut visited = HashSet::new();
        visited.insert((source.to_string(), 0));

        let mut i = 0;
        while i < results.len() {
            let current = results[i].clone();
            i += 1;

            for group in &self.reasons {
                for variant in &group.variants {
                    if current.rules != 0 && (current.rules & variant.rules_in) == 0 {
                        continue;
                    }
                    if !current.term.ends_with(&variant.kana_in) {
                        continue;
                    }

                    // Check character count rather than byte count
                    let term_char_count = current.term.chars().count();
                    let in_char_count = variant.kana_in.chars().count();
                    let out_char_count = variant.kana_out.chars().count();

                    if term_char_count < in_char_count {
                        continue;
                    }

                    let new_char_count = term_char_count - in_char_count + out_char_count;
                    if new_char_count == 0 {
                        continue;
                    }

                    // Build new candidate term
                    let prefix_byte_len = current.term.len() - variant.kana_in.len();
                    let mut new_term = current.term[..prefix_byte_len].to_string();
                    new_term.push_str(&variant.kana_out);

                    let key = (new_term.clone(), variant.rules_out);
                    if visited.insert(key) {
                        let mut new_reasons = current.reasons.clone();
                        // Format clean reason (e.g. remove leading dash)
                        let clean_reason = group.reason.trim_start_matches('-').to_string();
                        new_reasons.push(clean_reason);

                        results.push(DeinflectionCandidate {
                            term: new_term,
                            rules: variant.rules_out,
                            reasons: new_reasons,
                        });
                    }
                }
            }
        }

        results
    }
}

pub fn global_deinflector() -> &'static Deinflector {
    static DEINFLECTOR: OnceLock<Deinflector> = OnceLock::new();
    DEINFLECTOR.get_or_init(Deinflector::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deinflect_verbs() {
        let deinflector = global_deinflector();

        // 食べた -> 食べる (past)
        let candidates = deinflector.deinflect("食べた");
        let taberu = candidates
            .iter()
            .find(|c| c.term == "食べる" && (c.rules & RULE_V1) != 0);
        assert!(taberu.is_some(), "Expected 食べる in {:?}", candidates);
        assert!(taberu.unwrap().reasons.contains(&"ta".to_string()) || taberu.unwrap().reasons.contains(&"past".to_string()));

        // 書かない -> 書く (negative)
        let candidates = deinflector.deinflect("書かない");
        let kaku = candidates
            .iter()
            .find(|c| c.term == "書く" && (c.rules & RULE_V5) != 0);
        assert!(kaku.is_some(), "Expected 書く in {:?}", candidates);

        // 話しました -> 話す (polite past)
        let candidates = deinflector.deinflect("話しました");
        let hanasu = candidates
            .iter()
            .find(|c| c.term == "話す" && (c.rules & RULE_V5) != 0);
        assert!(hanasu.is_some(), "Expected 話す in {:?}", candidates);

        // 行かなかった -> 行く (negative past)
        let candidates = deinflector.deinflect("行かなかった");
        let iku = candidates
            .iter()
            .find(|c| c.term == "行く" && (c.rules & RULE_V5) != 0);
        assert!(iku.is_some(), "Expected 行く in {:?}", candidates);

        // 泳げない -> 泳ぐ (potential negative)
        let candidates = deinflector.deinflect("泳げない");
        let oyogu = candidates
            .iter()
            .find(|c| c.term == "泳ぐ" && (c.rules & RULE_V5) != 0);
        assert!(oyogu.is_some(), "Expected 泳ぐ in {:?}", candidates);
    }

    #[test]
    fn test_deinflect_adjectives() {
        let deinflector = global_deinflector();

        // 高ければ -> 高い
        let candidates = deinflector.deinflect("高ければ");
        let takai = candidates
            .iter()
            .find(|c| c.term == "高い" && (c.rules & RULE_ADJ_I) != 0);
        assert!(takai.is_some(), "Expected 高い in {:?}", candidates);

        // 高くなかった -> 高い
        let candidates = deinflector.deinflect("高くなかった");
        let takai = candidates
            .iter()
            .find(|c| c.term == "高い" && (c.rules & RULE_ADJ_I) != 0);
        assert!(takai.is_some(), "Expected 高い in {:?}", candidates);
    }
}
