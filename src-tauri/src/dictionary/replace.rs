//! "Heard → written" replacements applied to every transcription.
//!
//! Matching is case-insensitive, whole-word (Unicode aware, so accented
//! letters count as letters), tolerant to spacing and apostrophe style, and
//! prefers the longest phrase when several start at the same place.

use super::entries::fold;
use crate::settings::WordReplacement;
use log::warn;
use regex::{Captures, Regex};
use std::collections::HashMap;

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Regex for one heard phrase: whitespace matches any run of whitespace and
/// either apostrophe matches both styles. Word boundaries are only required
/// on sides that are letters, so terms like "R&D" or ".NET" still match.
fn phrase_pattern(from: &str) -> String {
    let words: Vec<String> = from
        .split_whitespace()
        .map(|w| regex::escape(w).replace(['\'', '’'], "['’]"))
        .collect();
    let body = words.join(r"\s+");
    let starts_word = from.chars().next().is_some_and(is_word_char);
    let ends_word = from.chars().last().is_some_and(is_word_char);
    format!(
        "{}{}{}",
        if starts_word { r"\b" } else { "" },
        body,
        if ends_word { r"\b" } else { "" }
    )
}

/// The model capitalised a lowercase term (sentence start): keep that.
fn adapt_case(matched: &str, rule: &WordReplacement) -> String {
    let matched_upper = matched.chars().next().is_some_and(char::is_uppercase);
    let rule_lower = rule.from.chars().next().is_some_and(char::is_lowercase);
    let mut to = rule.to.chars();
    match to.next() {
        Some(first) if matched_upper && rule_lower && first.is_lowercase() => {
            first.to_uppercase().chain(to).collect()
        }
        _ => rule.to.clone(),
    }
}

pub fn apply_replacements(text: &str, rules: &[WordReplacement]) -> String {
    let mut usable: Vec<&WordReplacement> = rules
        .iter()
        .filter(|r| !r.from.trim().is_empty() && !r.to.trim().is_empty())
        .collect();
    if usable.is_empty() || text.is_empty() {
        return text.to_string();
    }
    // Longest first: alternation is leftmost-first, so "chat j'ai pété" must
    // be tried before "chat".
    usable.sort_by_key(|r| std::cmp::Reverse(r.from.chars().count()));

    let by_fold: HashMap<String, &WordReplacement> =
        usable.iter().map(|r| (fold(&r.from), *r)).collect();
    let pattern = format!(
        "(?i)(?:{})",
        usable
            .iter()
            .map(|r| phrase_pattern(&r.from))
            .collect::<Vec<_>>()
            .join("|")
    );
    let regex = match Regex::new(&pattern) {
        Ok(regex) => regex,
        Err(e) => {
            warn!("Dictionary replacements skipped, pattern rejected: {}", e);
            return text.to_string();
        }
    };

    regex
        .replace_all(text, |caps: &Captures| {
            let matched = &caps[0];
            match by_fold.get(&fold(matched)) {
                Some(rule) => adapt_case(matched, rule),
                None => matched.to_string(),
            }
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(from: &str, to: &str) -> WordReplacement {
        WordReplacement {
            from: from.into(),
            to: to.into(),
        }
    }

    #[test]
    fn replaces_whole_words_case_insensitively() {
        let rules = [rule("Andy", "Handy")];
        assert_eq!(
            apply_replacements("J'utilise andy, et Andy marche.", &rules),
            "J'utilise Handy, et Handy marche."
        );
        // Not inside another word.
        assert_eq!(apply_replacements("Andyamo", &rules), "Andyamo");
    }

    #[test]
    fn multi_word_phrases_tolerate_spacing_and_apostrophes() {
        let rules = [rule("chat j'ai pété", "ChatGPT")];
        assert_eq!(
            apply_replacements("Demande à chat  j’ai pété.", &rules),
            "Demande à ChatGPT."
        );
    }

    #[test]
    fn longest_phrase_wins() {
        let rules = [rule("chat", "Chat"), rule("chat j'ai pété", "ChatGPT")];
        assert_eq!(apply_replacements("chat j'ai pété", &rules), "ChatGPT");
    }

    #[test]
    fn accented_letters_are_word_characters() {
        let rules = [rule("été", "ÉTÉ")];
        assert_eq!(apply_replacements("étéa été", &rules), "étéa ÉTÉ");
    }

    #[test]
    fn keeps_sentence_capitalisation_for_lowercase_targets() {
        let rules = [rule("rentrée", "rentrée scolaire")];
        assert_eq!(
            apply_replacements("Rentrée demain.", &rules),
            "Rentrée scolaire demain."
        );
    }

    #[test]
    fn symbols_at_term_edges_still_match() {
        let rules = [rule("R&D", "recherche")];
        assert_eq!(
            apply_replacements("la R&D avance", &rules),
            "la recherche avance"
        );
    }

    #[test]
    fn regex_metacharacters_are_literal() {
        let rules = [rule("a.b", "X")];
        assert_eq!(apply_replacements("acb a.b", &rules), "acb X");
    }

    #[test]
    fn no_rules_is_a_no_op() {
        assert_eq!(apply_replacements("bonjour", &[]), "bonjour");
    }
}
