//! Dictionary entries and the merge rules shared by sync and import.
//!
//! Every entry is keyed by its kind and the folded form of what is heard, so
//! "Handy" and "handy" are the same word and a replacement has exactly one
//! target. Deletions are kept as tombstones so that removing a word on one Mac
//! also removes it on the other after a sync, instead of the word coming back.
//! Conflicts resolve last-writer-wins on `updated_at`, with a deterministic
//! tie-break so two machines always converge on the same result.

use serde::{Deserialize, Serialize};
use specta::Type;
use std::cmp::Ordering;
use std::collections::HashMap;

/// Longest term (heard or written) accepted, in characters.
pub const MAX_TERM_CHARS: usize = 50;
/// Upper bound on entries read from a file we did not write ourselves.
pub const MAX_ENTRIES: usize = 5000;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Type)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A term the model should know (sent as vocabulary when supported).
    Word,
    /// "Heard → written": rewritten after transcription.
    Replacement,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct DictionaryEntry {
    pub id: String,
    pub kind: EntryKind,
    /// Word: the term itself. Replacement: what the model writes by mistake.
    pub from: String,
    /// Replacement only: what should be written instead. Empty for words.
    #[serde(default)]
    pub to: String,
    /// Display name of whoever added (or last re-added) the entry.
    #[serde(default)]
    pub added_by: String,
    /// Milliseconds since the Unix epoch.
    #[serde(default)]
    pub created_at: i64,
    /// Milliseconds since the Unix epoch; drives conflict resolution.
    #[serde(default)]
    pub updated_at: i64,
    /// Tombstone: kept so deletions propagate through sync.
    #[serde(default)]
    pub deleted: bool,
}

/// How a term is compared: case-insensitive, whitespace collapsed and
/// typographic apostrophes treated like straight ones.
pub fn fold(term: &str) -> String {
    term.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(['’', 'ʼ'], "'")
        .to_lowercase()
}

/// User-typed term, tidied: trimmed, inner whitespace collapsed, and the
/// characters that never belong in a vocabulary term removed. Apostrophes are
/// kept on purpose, French needs them ("aujourd'hui", "j'ai").
pub fn clean_term(term: &str) -> String {
    term.chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>' | '"'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn entry_id(kind: EntryKind, from: &str) -> String {
    let prefix = match kind {
        EntryKind::Word => "word",
        EntryKind::Replacement => "replacement",
    };
    format!("{}:{}", prefix, fold(from))
}

impl DictionaryEntry {
    pub fn new(kind: EntryKind, from: &str, to: &str, author: &str, now: i64) -> Self {
        let to = match kind {
            EntryKind::Word => String::new(),
            EntryKind::Replacement => to.to_string(),
        };
        Self {
            id: entry_id(kind, from),
            kind,
            from: from.to_string(),
            to,
            added_by: author.to_string(),
            created_at: now,
            updated_at: now,
            deleted: false,
        }
    }

    /// The term this entry teaches the model: the word, or a replacement's target.
    pub fn written_form(&self) -> &str {
        match self.kind {
            EntryKind::Word => &self.from,
            EntryKind::Replacement => &self.to,
        }
    }

    /// Entry from a file we did not write: re-derives the id (never trusted),
    /// tidies the terms and rejects anything unusable.
    pub fn sanitized(&self) -> Option<Self> {
        let from = clean_term(&self.from);
        let to = match self.kind {
            EntryKind::Word => String::new(),
            EntryKind::Replacement => clean_term(&self.to),
        };
        let too_long = from.chars().count() > MAX_TERM_CHARS || to.chars().count() > MAX_TERM_CHARS;
        if from.is_empty() || too_long || (self.kind == EntryKind::Replacement && to.is_empty()) {
            return None;
        }
        Some(Self {
            id: entry_id(self.kind, &from),
            from,
            to,
            added_by: clean_term(&self.added_by).chars().take(40).collect(),
            ..self.clone()
        })
    }

    /// Fields compared when timestamps tie, so both sides pick the same winner.
    fn tie_break(&self) -> (bool, &str, &str, &str, i64) {
        (
            self.deleted,
            self.to.as_str(),
            self.from.as_str(),
            self.added_by.as_str(),
            self.created_at,
        )
    }

    /// Whether `self` should replace `other` (same id) in a merge.
    fn wins_over(&self, other: &Self) -> bool {
        match self.updated_at.cmp(&other.updated_at) {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => self.tie_break() > other.tie_break(),
        }
    }

    fn same_content(&self, other: &Self) -> bool {
        self.deleted == other.deleted && self.from == other.from && self.to == other.to
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeMode {
    /// Two-way sync: tombstones are honoured so deletions propagate.
    Sync,
    /// One-shot import of someone's file: it may add or update entries but
    /// never deletes anything here.
    Import,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MergeStats {
    pub added: u32,
    pub updated: u32,
    pub removed: u32,
}

impl MergeStats {
    pub fn changed(&self) -> bool {
        self.added + self.updated + self.removed > 0
    }
}

/// Merges `incoming` into `local` in place and reports what changed for the
/// user. Merging is commutative and idempotent, so syncing in any order and
/// any number of times converges.
pub fn merge_entries(
    local: &mut Vec<DictionaryEntry>,
    incoming: &[DictionaryEntry],
    mode: MergeMode,
) -> MergeStats {
    let mut stats = MergeStats::default();
    let mut index: HashMap<String, usize> = local
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.clone(), i))
        .collect();

    for raw in incoming.iter().take(MAX_ENTRIES) {
        let Some(entry) = raw.sanitized() else {
            continue;
        };
        if mode == MergeMode::Import && entry.deleted {
            continue;
        }
        match index.get(&entry.id) {
            None => {
                if !entry.deleted {
                    stats.added += 1;
                }
                index.insert(entry.id.clone(), local.len());
                local.push(entry);
            }
            Some(&i) => {
                let current = &local[i];
                if !entry.wins_over(current) {
                    continue;
                }
                match (current.deleted, entry.deleted) {
                    (true, false) => stats.added += 1,
                    (false, true) => stats.removed += 1,
                    (false, false) if !entry.same_content(current) => stats.updated += 1,
                    _ => {}
                }
                local[i] = entry;
            }
        }
    }
    stats
}

/// Live entries only, newest first.
pub fn live_entries(entries: &[DictionaryEntry]) -> Vec<DictionaryEntry> {
    let mut live: Vec<DictionaryEntry> = entries.iter().filter(|e| !e.deleted).cloned().collect();
    live.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    live
}

/// Vocabulary for the model: words, then replacement targets, oldest first,
/// without duplicates.
pub fn vocabulary(entries: &[DictionaryEntry]) -> Vec<String> {
    let mut live: Vec<&DictionaryEntry> = entries.iter().filter(|e| !e.deleted).collect();
    live.sort_by(|a, b| {
        (a.kind == EntryKind::Replacement)
            .cmp(&(b.kind == EntryKind::Replacement))
            .then(a.created_at.cmp(&b.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut seen = std::collections::HashSet::new();
    live.into_iter()
        .map(|e| e.written_form().to_string())
        .filter(|term| seen.insert(fold(term)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(from: &str, by: &str, at: i64) -> DictionaryEntry {
        DictionaryEntry::new(EntryKind::Word, from, "", by, at)
    }

    fn repl(from: &str, to: &str, by: &str, at: i64) -> DictionaryEntry {
        DictionaryEntry::new(EntryKind::Replacement, from, to, by, at)
    }

    fn tombstone(mut e: DictionaryEntry, at: i64) -> DictionaryEntry {
        e.deleted = true;
        e.updated_at = at;
        e
    }

    #[test]
    fn fold_ignores_case_spacing_and_apostrophe_style() {
        assert_eq!(fold("  J’ai   Pété "), "j'ai pété");
        assert_eq!(entry_id(EntryKind::Word, "Handy"), "word:handy");
    }

    #[test]
    fn clean_term_keeps_apostrophes_and_drops_markup() {
        assert_eq!(clean_term("  aujourd'hui <b> \"x\" "), "aujourd'hui b x");
    }

    #[test]
    fn merge_adds_unknown_entries_with_their_provenance() {
        let mut local = vec![word("Handy", "Marius", 1)];
        let stats = merge_entries(
            &mut local,
            &[word("Yasmine", "Yasmine", 2)],
            MergeMode::Sync,
        );
        assert_eq!(stats.added, 1);
        assert_eq!(local[1].added_by, "Yasmine");
    }

    #[test]
    fn newer_replacement_target_wins() {
        let mut local = vec![repl("Andy", "Handi", "Marius", 1)];
        let stats = merge_entries(
            &mut local,
            &[repl("andy", "Handy", "Yasmine", 5)],
            MergeMode::Sync,
        );
        assert_eq!(stats.updated, 1);
        assert_eq!(local[0].to, "Handy");
    }

    #[test]
    fn older_change_does_not_overwrite_newer_one() {
        let mut local = vec![repl("Andy", "Handy", "Marius", 9)];
        let stats = merge_entries(
            &mut local,
            &[repl("Andy", "Andie", "Yasmine", 3)],
            MergeMode::Sync,
        );
        assert!(!stats.changed());
        assert_eq!(local[0].to, "Handy");
    }

    #[test]
    fn deletion_propagates_through_sync_but_not_import() {
        let remote = vec![tombstone(word("Handy", "Marius", 1), 10)];

        let mut synced = vec![word("Handy", "Marius", 1)];
        let stats = merge_entries(&mut synced, &remote, MergeMode::Sync);
        assert_eq!(stats.removed, 1);
        assert!(synced[0].deleted);

        let mut imported = vec![word("Handy", "Marius", 1)];
        let stats = merge_entries(&mut imported, &remote, MergeMode::Import);
        assert!(!stats.changed());
        assert!(!imported[0].deleted);
    }

    #[test]
    fn re_adding_after_delete_revives_the_word() {
        let mut local = vec![tombstone(word("Handy", "Marius", 1), 5)];
        let stats = merge_entries(&mut local, &[word("Handy", "Yasmine", 8)], MergeMode::Sync);
        assert_eq!(stats.added, 1);
        assert!(!local[0].deleted);
    }

    #[test]
    fn merge_is_commutative_and_idempotent() {
        let a = vec![
            word("Handy", "Marius", 1),
            repl("Andy", "Handy", "Marius", 4),
            tombstone(word("Wispr", "Marius", 1), 6),
        ];
        let b = vec![
            word("Wispr", "Yasmine", 2),
            repl("Andy", "Handie", "Yasmine", 4),
            word("Yasmine", "Yasmine", 3),
        ];

        let mut ab = a.clone();
        merge_entries(&mut ab, &b, MergeMode::Sync);
        let mut ba = b.clone();
        merge_entries(&mut ba, &a, MergeMode::Sync);

        let sorted = |mut v: Vec<DictionaryEntry>| {
            v.sort_by(|x, y| x.id.cmp(&y.id));
            v
        };
        assert_eq!(sorted(ab.clone()), sorted(ba));

        let again = merge_entries(&mut ab, &b, MergeMode::Sync);
        assert!(!again.changed());
    }

    #[test]
    fn untrusted_entries_are_sanitized() {
        let mut bad = word("Handy", "Marius", 1);
        bad.id = "word:evil".into();
        let mut empty_target = repl("Andy", "", "Marius", 1);
        empty_target.to = " ".into();
        let mut local = Vec::new();
        merge_entries(&mut local, &[bad, empty_target], MergeMode::Sync);
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].id, "word:handy");
    }

    #[test]
    fn vocabulary_lists_words_then_targets_without_duplicates() {
        let entries = vec![
            repl("Andy", "Handy", "M", 1),
            word("Handy", "M", 2),
            word("Yasmine", "Y", 3),
            tombstone(word("Wispr", "M", 0), 4),
            repl("chat j'ai pété", "ChatGPT", "M", 5),
        ];
        assert_eq!(vocabulary(&entries), vec!["Handy", "Yasmine", "ChatGPT"]);
    }
}
