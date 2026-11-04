//! Searching mail: a query in the search language (query.rs) run over the
//! store, as conversations, newest first. Words are searched in what has
//! been downloaded; mail known only by its headers matches on subject and
//! people, and [`Remote`] says when Gmail should be asked too.

use std::collections::{HashMap, HashSet};

use postrinch_model::{
    EmailAddress, LabelKind, Message, ProviderMessageId, ProviderThreadId, ThreadSummary, Timestamp,
};
use postrinch_store::Find;
use serde::{Deserialize, Serialize};

use crate::query::{LabelName, Query, label_matches};
use crate::{CommandError, CommandResult, Commands, ThreadRow};

/// How much text an excerpt keeps before what matched, in chars: little,
/// so the match itself shows in a narrow row.
const BEFORE_CHARS: usize = 24;
/// How much it keeps after.
const AFTER_CHARS: usize = 80;

/// Threads that matched a search, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResults {
    pub query: String,
    pub threads: Vec<ThreadRow>,
    /// Whether more threads matched than the limit let through.
    pub more: bool,
    /// Gmail's search to run too, when the words could be in mail not
    /// downloaded yet.
    #[serde(default)]
    pub remote: Option<Remote>,
}

/// Asking the provider about mail whose bodies are not here yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    /// The query in Gmail's language.
    pub query: String,
    /// The accounts to ask, with how many of their messages are not
    /// downloaded.
    pub accounts: Vec<(EmailAddress, usize)>,
}

/// Where a search matched in a message, to show with the match marked.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Excerpt {
    pub before: String,
    pub hit: String,
    pub after: String,
}

impl Commands {
    /// Search the mail this caller sees. `query` is in the search language:
    /// words (all required), `"exact phrases"`, `prefix*`, and Gmail's
    /// operators (`from:`, `to:`, `subject:`, `label:`, `in:`, `is:`,
    /// `has:attachment`, `after:`, `before:`, `newer_than:`,
    /// `older_than:`, `category:`) plus `account:`. At most `limit`
    /// threads come back, newest first.
    pub fn search(&self, query: &str, limit: usize) -> CommandResult<SearchResults> {
        let text = query.trim();
        let mut results = SearchResults {
            query: text.to_owned(),
            threads: Vec::new(),
            more: false,
            remote: None,
        };
        if text.is_empty() {
            return Ok(results);
        }
        let now = Timestamp::from_millis(jiff::Timestamp::now().as_millisecond());
        let query = Query::parse(text, now)?;
        let accounts = self.search_accounts(&query)?;
        let find = self.to_find(&query, &accounts)?;

        let mut threads: Vec<(EmailAddress, ProviderThreadId, ProviderMessageId)> = Vec::new();
        let mut seen: HashSet<(String, ProviderThreadId)> = HashSet::new();
        for found in self.store.find_messages(&find)? {
            let Some(thread) = self.store.found_thread(&found)? else {
                continue;
            };
            if seen.insert((found.account.key(), thread.clone())) {
                threads.push((found.account, thread, found.id));
                if threads.len() > limit {
                    break;
                }
            }
        }
        results.more = threads.len() > limit;
        threads.truncate(limit);
        let terms = query.terms();
        let mut chips = HashMap::new();
        for (account, thread, matched) in threads {
            if let Some(row) =
                self.search_row(&account, &thread, Some(&matched), &terms, &mut chips)?
            {
                results.threads.push(row);
            }
        }
        if query.has_words() {
            let waiting = self.store.undownloaded_counts()?;
            let ask: Vec<(EmailAddress, usize)> = accounts
                .iter()
                .filter_map(|a| waiting.get(&a.key()).map(|&n| (a.clone(), n)))
                .filter(|(_, n)| *n > 0)
                .collect();
            if !ask.is_empty() {
                results.remote = Some(Remote {
                    query: query.gmail.clone(),
                    accounts: ask,
                });
            }
        }
        Ok(results)
    }

    /// Rows for messages the provider found that the local search did
    /// not: their threads, leaving out those in `listed` and, unless the
    /// query looks there, the trash and spam. Marked as found remotely.
    pub fn remote_rows(
        &self,
        query: &str,
        account: &EmailAddress,
        found: &[ProviderMessageId],
        listed: &HashSet<ProviderThreadId>,
    ) -> CommandResult<Vec<ThreadRow>> {
        self.check(account)?;
        let now = Timestamp::from_millis(jiff::Timestamp::now().as_millisecond());
        let query = Query::parse(query, now)?;
        let set_aside = if query.set_aside_too {
            HashSet::new()
        } else {
            self.set_aside_ids(account)?
        };
        let terms = query.terms();
        let mut chips = HashMap::new();
        let mut seen: HashSet<ProviderThreadId> = listed.clone();
        let mut rows = Vec::new();
        for id in found {
            let Some(message) = self.store.message(account, id)? else {
                continue;
            };
            if !seen.insert(message.thread.clone()) {
                continue;
            }
            if message.labels.iter().any(|l| set_aside.contains(l)) {
                continue;
            }
            if let Some(mut row) =
                self.search_row(account, &message.thread, Some(id), &terms, &mut chips)?
            {
                row.remote = true;
                rows.push(row);
            }
        }
        Ok(rows)
    }

    /// A search result's row: the thread, with where `matched` matched.
    fn search_row(
        &self,
        account: &EmailAddress,
        thread: &ProviderThreadId,
        matched: Option<&ProviderMessageId>,
        terms: &[String],
        chips: &mut HashMap<String, HashMap<String, crate::Chip>>,
    ) -> CommandResult<Option<ThreadRow>> {
        let messages = self.store.thread_messages(account, thread)?;
        let Some(summary) = ThreadSummary::from_messages(&messages) else {
            return Ok(None);
        };
        let known = match chips.get(&account.key()) {
            Some(known) => known,
            None => {
                let made = self.account_chips(account)?;
                chips.entry(account.key()).or_insert(made)
            }
        };
        let chips = summary
            .labels
            .iter()
            .filter_map(|l| known.get(l).cloned())
            .collect();
        let matched: Option<&Message> = matched
            .and_then(|id| messages.iter().find(|m| &m.id == id))
            .or(messages.last());
        let excerpt = matched.and_then(|m| {
            let text = format!("{}\n{}", m.subject(), m.body_text);
            excerpt(&text, terms)
        });
        Ok(Some(ThreadRow {
            summary,
            chips,
            excerpt,
            remote: false,
        }))
    }

    /// The accounts a query looks in: those this caller sees, narrowed by
    /// `account:`.
    fn search_accounts(&self, query: &Query) -> CommandResult<Vec<EmailAddress>> {
        let mut accounts: Vec<EmailAddress> = self
            .visible_accounts()?
            .into_iter()
            .map(|a| a.address)
            .collect();
        for part in &query.accounts {
            let part = part.to_lowercase();
            accounts.retain(|a| a.key().contains(&part));
            if accounts.is_empty() {
                return Err(CommandError::Invalid(format!(
                    "No account matches account:{part}."
                )));
            }
        }
        Ok(accounts)
    }

    /// The store's form of a query over `accounts`, labels found by name.
    fn to_find(&self, query: &Query, accounts: &[EmailAddress]) -> CommandResult<Find> {
        let resolve = |names: &[LabelName]| -> CommandResult<Vec<Vec<(EmailAddress, String)>>> {
            let mut all = Vec::new();
            for name in names {
                let mut any = Vec::new();
                for account in accounts {
                    for label in self.store.labels(account)? {
                        let hit = match (name, &label.kind) {
                            (LabelName::System(s), LabelKind::System(k)) => s == k,
                            (LabelName::Named(n), LabelKind::User) => label_matches(&label.name, n),
                            _ => false,
                        };
                        if hit {
                            any.push((account.clone(), label.provider_id));
                        }
                    }
                }
                if any.is_empty()
                    && let LabelName::Named(n) = name
                {
                    return Err(CommandError::Invalid(format!("No label is called {n}.")));
                }
                all.push(any);
            }
            Ok(all)
        };
        Ok(Find {
            text: query.text(),
            accounts: accounts.to_vec(),
            from: query.from.clone(),
            to: query.to.clone(),
            subject: query.subject.clone(),
            labels: resolve(&query.labels)?,
            without: resolve(&query.without)?,
            unread: query.unread,
            starred: query.starred,
            attachments: query.attachments,
            after: query.after,
            before: query.before,
            set_aside_too: query.set_aside_too,
        })
    }

    /// The provider's ids for the account's trash and spam.
    fn set_aside_ids(&self, account: &EmailAddress) -> CommandResult<HashSet<String>> {
        Ok(self
            .store
            .labels(account)?
            .into_iter()
            .filter(|l| {
                matches!(
                    l.kind,
                    LabelKind::System(
                        postrinch_model::SystemLabel::Trash | postrinch_model::SystemLabel::Spam
                    )
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
        Ok(crate::labels::chips(&labels, &prefs, &stats))
    }
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
