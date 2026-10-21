//! Suggestions the user can confirm: never applied on their own.
//!
//! Two sources:
//! - a correction made in the history ("Andy" edited into "Handy"), turned
//!   into a replacement or a vocabulary word;
//! - proper nouns that keep coming back in recent dictations.
//!
//! Both are deliberately conservative. A global replacement fires on every
//! future dictation, so lowercase-to-lowercase fixes ("est" → "et") are
//! grammar, not mishearings, and are never suggested.

use super::entries::{fold, EntryKind, MAX_TERM_CHARS};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct DictionarySuggestion {
    /// Stable key used to dismiss the suggestion for good.
    pub key: String,
    pub kind: EntryKind,
    pub from: String,
    /// Replacement target; empty for words.
    pub to: String,
    /// How often a recurring name was seen (recurring suggestions only).
    pub occurrences: Option<u32>,
}

impl DictionarySuggestion {
    pub fn word(term: &str, occurrences: Option<u32>) -> Self {
        Self {
            key: format!("word:{}", fold(term)),
            kind: EntryKind::Word,
            from: term.to_string(),
            to: String::new(),
            occurrences,
        }
    }

    pub fn replacement(from: &str, to: &str) -> Self {
        Self {
            key: format!("replacement:{}>{}", fold(from), fold(to)),
            kind: EntryKind::Replacement,
            from: from.to_string(),
            to: to.to_string(),
            occurrences: None,
        }
    }
}

/// Most suggestions offered after one edit; more is noise.
const MAX_EDIT_SUGGESTIONS: usize = 3;
/// Longest changed span (in words) still read as a mishearing, not a rewrite.
const MAX_HEARD_WORDS: usize = 4;
const MAX_WRITTEN_WORDS: usize = 3;
/// Beyond this the edit is a rewrite; skip the quadratic diff.
const MAX_DIFF_CELLS: usize = 400_000;

fn is_edge_punctuation(c: char) -> bool {
    !c.is_alphanumeric() && !matches!(c, '&' | '+' | '#' | '@')
}

/// A word stripped of the punctuation around it ("Andy," → "Andy").
fn core(token: &str) -> &str {
    token.trim_matches(is_edge_punctuation)
}

fn ends_sentence(token: &str) -> bool {
    token
        .trim_end_matches(['"', '»', '”', ')', '\''])
        .ends_with(['.', '!', '?', '…', ':'])
}

struct Token<'a> {
    core: &'a str,
    sentence_start: bool,
}

fn tokens(text: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut sentence_start = true;
    for raw in text.split_whitespace() {
        let c = core(raw);
        if !c.is_empty() {
            out.push(Token {
                core: c,
                sentence_start,
            });
            sentence_start = false;
        }
        if ends_sentence(raw) {
            sentence_start = true;
        }
    }
    out
}

/// Looks like a name or a technical term rather than an ordinary word.
fn looks_like_term(term: &str) -> bool {
    term.chars()
        .any(|c| c.is_uppercase() || c.is_ascii_digit() || "-_.@&+#/".contains(c))
}

/// Capitalised only because it opens a sentence ("Et", "Alors").
fn only_initial_capital(term: &str) -> bool {
    let mut chars = term.chars();
    let first_upper = chars.next().is_some_and(char::is_uppercase);
    first_upper
        && !chars.any(|c| c.is_uppercase() || c.is_ascii_digit() || "-_.@&+#/".contains(c))
        && !term.contains(' ')
}

struct Hunk<'a> {
    heard: Vec<&'a str>,
    written: Vec<&'a str>,
    at_sentence_start: bool,
}

/// Word-level diff (LCS) reduced to the spans that changed.
fn changed_spans<'a>(before: &[Token<'a>], after: &[Token<'a>]) -> Vec<Hunk<'a>> {
    let (n, m) = (before.len(), after.len());
    if n == 0 || m == 0 || n * m > MAX_DIFF_CELLS {
        return Vec::new();
    }
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if before[i].core == after[j].core {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut hunks = Vec::new();
    let (mut i, mut j) = (0, 0);
    let mut current: Option<Hunk<'a>> = None;
    while i < n || j < m {
        if i < n && j < m && before[i].core == after[j].core {
            hunks.extend(current.take());
            i += 1;
            j += 1;
            continue;
        }
        let hunk = current.get_or_insert_with(|| Hunk {
            heard: Vec::new(),
            written: Vec::new(),
            at_sentence_start: after.get(j).is_some_and(|t| t.sentence_start),
        });
        if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            hunk.written.push(after[j].core);
            j += 1;
        } else {
            hunk.heard.push(before[i].core);
            i += 1;
        }
    }
    hunks.extend(current);
    hunks
}

/// What a correction in the history could teach the dictionary.
pub fn suggestions_from_edit(original: &str, edited: &str) -> Vec<DictionarySuggestion> {
    let before = tokens(original);
    let after = tokens(edited);
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for hunk in changed_spans(&before, &after) {
        if hunk.heard.is_empty()
            || hunk.written.is_empty()
            || hunk.heard.len() > MAX_HEARD_WORDS
            || hunk.written.len() > MAX_WRITTEN_WORDS
        {
            continue;
        }
        let heard = hunk.heard.join(" ");
        let written = hunk.written.join(" ");
        if heard.chars().count() > MAX_TERM_CHARS || written.chars().count() > MAX_TERM_CHARS {
            continue;
        }
        if hunk.at_sentence_start && only_initial_capital(&written) {
            continue;
        }

        let suggestion = if fold(&heard) == fold(&written) {
            // Only the casing changed: teach the word, not a rewrite rule.
            if !written.chars().any(char::is_uppercase) {
                continue;
            }
            DictionarySuggestion::word(&written, None)
        } else if looks_like_term(&written) {
            DictionarySuggestion::replacement(&heard, &written)
        } else {
            continue;
        };

        if seen.insert(suggestion.key.clone()) {
            out.push(suggestion);
        }
        if out.len() == MAX_EDIT_SUGGESTIONS {
            break;
        }
    }
    out
}

/// Words capitalised mid-sentence for reasons other than being a name.
const NOT_NAMES: &[&str] = &[
    "i",
    "i'm",
    "i've",
    "i'll",
    "i'd",
    "ok",
    "okay",
    "mr",
    "mrs",
    "ms",
    "dr",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

/// Longest run of capitalised words read as one name ("Claude Code").
const MAX_NAME_WORDS: usize = 3;

fn is_capitalised(core: &str) -> bool {
    core.chars().next().is_some_and(char::is_uppercase)
        && core.chars().count() >= 2
        && core.chars().any(char::is_alphabetic)
}

/// Proper nouns seen at least `min_occurrences` times across at least
/// `min_entries` different dictations, most frequent first. `known` holds
/// folded terms already in the dictionary or dismissed.
pub fn recurring_names(
    texts: &[&str],
    known: &HashSet<String>,
    min_occurrences: u32,
    min_entries: usize,
    limit: usize,
) -> Vec<DictionarySuggestion> {
    struct Seen {
        occurrences: u32,
        entries: HashSet<usize>,
        forms: HashMap<String, u32>,
    }
    let mut seen: HashMap<String, Seen> = HashMap::new();

    for (entry_index, text) in texts.iter().enumerate() {
        let raw_tokens: Vec<&str> = text.split_whitespace().collect();
        let mut sentence_start = true;
        let mut i = 0;
        while i < raw_tokens.len() {
            let token = raw_tokens[i];
            let c = core(token);
            if c.is_empty() {
                sentence_start = sentence_start || ends_sentence(token);
                i += 1;
                continue;
            }
            if !is_capitalised(c) {
                sentence_start = ends_sentence(token);
                i += 1;
                continue;
            }

            // Gather a run of capitalised words; punctuation ends the run.
            let mut run = vec![c];
            let mut last = token;
            let mut j = i + 1;
            while j < raw_tokens.len()
                && run.len() < MAX_NAME_WORDS
                && core(last) == last
                && is_capitalised(core(raw_tokens[j]))
            {
                last = raw_tokens[j];
                run.push(core(last));
                j += 1;
            }

            // A run opening a sentence is ambiguous ("Merci Yasmine"): skip it.
            if !sentence_start {
                let name = run.join(" ");
                let key = fold(&name);
                if !NOT_NAMES.contains(&key.as_str())
                    && !known.contains(&key)
                    && name.chars().count() <= MAX_TERM_CHARS
                {
                    let slot = seen.entry(key).or_insert_with(|| Seen {
                        occurrences: 0,
                        entries: HashSet::new(),
                        forms: HashMap::new(),
                    });
                    slot.occurrences += 1;
                    slot.entries.insert(entry_index);
                    *slot.forms.entry(name).or_insert(0) += 1;
                }
            }
            sentence_start = ends_sentence(last);
            i = j;
        }
    }

    let mut names: Vec<(String, u32)> = seen
        .into_values()
        .filter(|s| s.occurrences >= min_occurrences && s.entries.len() >= min_entries)
        .map(|s| {
            let form = s
                .forms
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
                .map(|(form, _)| form)
                .unwrap_or_default();
            (form, s.occurrences)
        })
        .collect();
    names.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    names
        .into_iter()
        .take(limit)
        .map(|(name, count)| DictionarySuggestion::word(&name, Some(count)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(s: &[DictionarySuggestion]) -> Vec<(EntryKind, String, String)> {
        s.iter()
            .map(|s| (s.kind, s.from.clone(), s.to.clone()))
            .collect()
    }

    #[test]
    fn misheard_name_becomes_a_replacement() {
        let s = suggestions_from_edit(
            "J'ai installé Andy sur mon Mac.",
            "J'ai installé Handy sur mon Mac.",
        );
        assert_eq!(
            pairs(&s),
            vec![(EntryKind::Replacement, "Andy".into(), "Handy".into())]
        );
    }

    #[test]
    fn several_words_heard_for_one_term() {
        let s = suggestions_from_edit(
            "demande à chat j'ai pété de résumer",
            "demande à ChatGPT de résumer",
        );
        assert_eq!(
            pairs(&s),
            vec![(
                EntryKind::Replacement,
                "chat j'ai pété".into(),
                "ChatGPT".into()
            )]
        );
    }

    #[test]
    fn grammar_fixes_are_not_suggested() {
        assert!(
            suggestions_from_edit("il est parti est revenu", "il est parti et revenu").is_empty()
        );
    }

    #[test]
    fn sentence_start_capital_is_not_a_name() {
        assert!(suggestions_from_edit("Bon. est-ce que", "Bon. Et que").is_empty());
    }

    #[test]
    fn casing_fix_teaches_the_word() {
        let s = suggestions_from_edit("on parle de yasmine demain", "on parle de Yasmine demain");
        assert_eq!(
            pairs(&s),
            vec![(EntryKind::Word, "Yasmine".into(), String::new())]
        );
    }

    #[test]
    fn punctuation_only_edits_are_ignored() {
        assert!(suggestions_from_edit("Salut Handy ça va", "Salut, Handy ! Ça va ?").is_empty());
    }

    #[test]
    fn long_rewrites_are_ignored() {
        assert!(suggestions_from_edit(
            "je pense que nous devrions partir tôt demain matin",
            "On part à 7 h avec Yasmine et Marius"
        )
        .is_empty());
    }

    #[test]
    fn identical_text_has_no_suggestions() {
        assert!(suggestions_from_edit("Bonjour Handy", "Bonjour Handy").is_empty());
    }

    #[test]
    fn recurring_names_need_repetition_across_dictations() {
        let texts = [
            "On voit Yasmine demain avec Claude Code.",
            "Dis à Yasmine que Claude Code est prêt.",
            "Je travaille avec Yasmine. Merci beaucoup.",
            "Rien à voir avec Paris.",
        ];
        let s = recurring_names(&texts, &HashSet::new(), 2, 2, 5);
        let names: Vec<_> = s.iter().map(|s| (s.from.clone(), s.occurrences)).collect();
        assert_eq!(
            names,
            vec![
                ("Yasmine".to_string(), Some(3)),
                ("Claude Code".to_string(), Some(2))
            ]
        );
    }

    #[test]
    fn known_and_sentence_start_words_are_skipped() {
        let texts = ["Handy marche. Handy est là", "Voilà Handy et Handy"];
        let known: HashSet<String> = HashSet::new();
        let s = recurring_names(&texts, &known, 2, 2, 5);
        assert_eq!(s.len(), 0, "only one mid-sentence use per text: {:?}", s);

        let texts = ["avec Handy", "avec Handy", "avec Handy"];
        let known: HashSet<String> = ["handy".to_string()].into_iter().collect();
        assert!(recurring_names(&texts, &known, 2, 2, 5).is_empty());
    }

    #[test]
    fn common_capitalised_words_are_not_names() {
        let texts = ["so I think OK", "and I said OK", "then I left OK"];
        assert!(recurring_names(&texts, &HashSet::new(), 2, 2, 5).is_empty());
    }
}
