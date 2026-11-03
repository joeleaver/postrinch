//! Searching mail: the store's full-text matches, as threads.

use std::collections::{HashMap, HashSet};

use postrinch_model::{
    EmailAddress, LabelKind, Message, ProviderThreadId, SystemLabel, ThreadSummary,
};
use serde::{Deserialize, Serialize};

use crate::labels::chips;
use crate::{CommandResult, Commands, ThreadRow};

/// How many matching messages a search looks at.
const MESSAGE_LIMIT: usize = 500;
/// How much text an excerpt keeps before what matched, in chars: little,
/// so the match itself shows in a narrow row.
const BEFORE_CHARS: usize = 24;
/// How much it keeps after.
const AFTER_CHARS: usize = 80;

/// Threads that matched a search, best match first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResults {
    pub query: String,
    pub threads: Vec<ThreadRow>,
    /// Whether more threads matched than the limit let through.
    pub more: bool,
}

/// Where a search matched in a message, to show with the match marked.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Excerpt {
    pub before: String,
    pub hit: String,
    pub after: String,
}

impl Commands {
    /// Search every account's mail: subjects, people and bodies. The query
    /// takes plain words, `"exact phrases"`, `+required` words and
    /// `prefix*`. At most `limit` threads come back.
    pub fn search(&self, query: &str, limit: usize) -> CommandResult<SearchResults> {
        let query = query.trim();
        let mut results = SearchResults {
            query: query.to_owned(),
            threads: Vec::new(),
            more: false,
        };
        if query.is_empty() {
            return Ok(results);
        }
        // The best-scoring message of each thread, in the order found.
        let mut best: Vec<Message> = Vec::new();
        let mut seen: HashSet<(String, ProviderThreadId)> = HashSet::new();
        let mut set_aside: HashMap<String, HashSet<String>> = HashMap::new();
        for hit in self.store.search(query, MESSAGE_LIMIT)? {
            if !self.sees(&hit.message.account) {
                continue;
            }
            // The trash and spam are left out, as from every mailbox.
            let account = hit.message.account.key();
            if !set_aside.contains_key(&account) {
                let ids = self.set_aside_labels(&hit.message.account)?;
                set_aside.insert(account.clone(), ids);
            }
            if hit
                .message
                .labels
                .iter()
                .any(|l| set_aside[&account].contains(l))
            {
                continue;
            }
            let key = (hit.message.account.key(), hit.message.thread.clone());
            if seen.insert(key) {
                best.push(hit.message);
            }
            if best.len() > limit {
                break;
            }
        }
        results.more = best.len() > limit;
        best.truncate(limit);
        let terms = terms(query);
        let mut account_chips = HashMap::new();
        for matched in best {
            let account = matched.account.clone();
            let messages = self.store.thread_messages(&account, &matched.thread)?;
            let Some(summary) = ThreadSummary::from_messages(&messages) else {
                continue;
            };
            let known = match account_chips.get(&account.key()) {
                Some(known) => known,
                None => {
                    let chips = self.account_chips(&account)?;
                    account_chips.entry(account.key()).or_insert(chips)
                }
            };
            let chips = summary
                .labels
                .iter()
                .filter_map(|l| known.get(l).cloned())
                .collect();
            let text = format!("{}\n{}", matched.subject(), matched.body_text);
            let excerpt = excerpt(&text, &terms);
            results.threads.push(ThreadRow {
                summary,
                chips,
                excerpt,
            });
        }
        Ok(results)
    }

    /// The provider's ids for the account's trash and spam.
    fn set_aside_labels(&self, account: &EmailAddress) -> CommandResult<HashSet<String>> {
        Ok(self
            .store
            .labels(account)?
            .into_iter()
            .filter(|l| {
                matches!(
                    l.kind,
                    LabelKind::System(SystemLabel::Trash | SystemLabel::Spam)
                )
            })
            .map(|l| l.provider_id)
            .collect())
    }

    pub(crate) fn account_chips(
        &self,
        account: &EmailAddress,
    ) -> CommandResult<HashMap<String, crate::Chip>> {
        let labels = self.store.labels(account)?;
        let prefs = self.store.label_prefs(account)?;
        let stats = self.store.label_stats(account)?;
        Ok(chips(&labels, &prefs, &stats))
    }
}

/// The words a query looks for, lowercased, without its operators.
fn terms(query: &str) -> Vec<String> {
    query
        .split(|c: char| c.is_whitespace() || c == '"')
        .map(|w| w.trim_start_matches(['+', '-']).trim_end_matches('*'))
        .filter(|w| w.chars().count() > 1)
        .map(str::to_lowercase)
        .collect()
}

/// The first place in `text` one of `terms` starts a word, with some text
/// either side, all on one line.
pub(crate) fn excerpt(text: &str, terms: &[String]) -> Option<Excerpt> {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = flat.to_lowercase();
    // Lowercasing can change byte lengths; only use positions where it did
    // not, so slicing `flat` at them is safe.
    if lower.len() != flat.len() {
        return None;
    }
    let (start, len) = terms
        .iter()
        .filter_map(|t| word_start(&lower, t).map(|i| (i, t.len())))
        .min()?;
    let end = start + len;
    let before: String = flat[..start]
        .chars()
        .rev()
        .take(BEFORE_CHARS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let after: String = flat[end..].chars().take(AFTER_CHARS).collect();
    let ellipsis_before = if before.len() < start { "…" } else { "" };
    Some(Excerpt {
        before: format!("{ellipsis_before}{before}"),
        hit: flat[start..end].to_owned(),
        after,
    })
}

/// Where `term` first starts a word in `text`.
fn word_start(text: &str, term: &str) -> Option<usize> {
    text.match_indices(term).map(|(i, _)| i).find(|&i| {
        text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_leave_out_operators() {
        assert_eq!(
            terms(r#"+cover "my shift" swap*"#),
            ["cover", "my", "shift", "swap"]
        );
    }

    #[test]
    fn excerpts_mark_the_first_word_that_matched() {
        let text = "Hi Mary,\n\nI can take Thursday. Swap me onto it.";
        let e = excerpt(text, &["thursday".into(), "swap".into()]).unwrap();
        assert_eq!(e.before, "Hi Mary, I can take ");
        assert_eq!(e.hit, "Thursday");
        assert_eq!(e.after, ". Swap me onto it.");
        // Only at the start of a word.
        assert!(excerpt("unswappable", &["swap".into()]).is_none());
    }

    #[test]
    fn long_text_is_cut_either_side() {
        let text = format!("{} needle {}", "a ".repeat(100), "b ".repeat(100));
        let e = excerpt(&text, &["needle".into()]).unwrap();
        assert!(e.before.starts_with('…'));
        assert!(e.before.chars().count() <= BEFORE_CHARS + 1);
        assert_eq!(e.after.chars().count(), AFTER_CHARS);
    }
}
