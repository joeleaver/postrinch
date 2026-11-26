//! Searching mail: a query in the search language (query.rs) run over the
//! store, as conversations, newest first. Words are searched in what has
//! been downloaded; mail known only by its headers matches on subject and
//! people, and [`Remote`] says when the accounts' servers should be asked
//! too: Gmail in its own language, an IMAP server by `SEARCH`
//! (imap_search.rs). What they find is listed by search_remote.rs.

use std::collections::{HashMap, HashSet};

use postrinch_model::{
    EmailAddress, ImapSearch, Message, Provider, ProviderMessageId, ProviderThreadId,
    ThreadSummary, Timestamp,
};
use postrinch_store::PersonEntry;
use serde::{Deserialize, Serialize};

use crate::messages::message_row;
use crate::query_or::Either;
use crate::{CommandResult, Commands, Grouping, ThreadRow};

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
    /// The servers' search to run too, when the words could be in mail
    /// not downloaded yet.
    #[serde(default)]
    pub remote: Option<Remote>,
}

/// Asking the providers about mail whose bodies are not here yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    /// The query in Gmail's language.
    pub query: String,
    /// The accounts to ask, with how many of their messages are not
    /// downloaded.
    pub accounts: Vec<(EmailAddress, usize)>,
    /// The IMAP accounts among them, each with its server's search; the
    /// others are Google's, asked in Gmail's language.
    #[serde(default)]
    pub imap: Vec<(EmailAddress, ImapSearch)>,
}

impl Remote {
    /// The IMAP server's search for an account, if it is an IMAP one.
    pub fn imap_search(&self, account: &EmailAddress) -> Option<&ImapSearch> {
        self.imap
            .iter()
            .find(|(a, _)| a == account)
            .map(|(_, search)| search)
    }
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
        self.search_grouped(query, &[], limit, Grouping::Conversations)
    }

    /// [`Commands::search`] in only these accounts (every account for
    /// none), listing what matched as conversations or as messages, each
    /// its own row.
    pub fn search_grouped(
        &self,
        query: &str,
        only: &[EmailAddress],
        limit: usize,
        grouping: Grouping,
    ) -> CommandResult<SearchResults> {
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
        let query = Either::parse(text, now)?;
        let (found, accounts) = self.found_in(&query, false, only)?;

        let mut threads: Vec<(EmailAddress, ProviderThreadId, ProviderMessageId)> = Vec::new();
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for found in found {
            let Some(thread) = self.store.found_thread(&found)? else {
                continue;
            };
            let one = match grouping {
                Grouping::Conversations => thread.0.clone(),
                Grouping::Messages => found.id.0.clone(),
            };
            if seen.insert((found.account.key(), one)) {
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
            let row = match grouping {
                Grouping::Conversations => {
                    self.search_row(&account, &thread, Some(&matched), &terms, &mut chips)?
                }
                Grouping::Messages => {
                    self.search_message_row(&account, &matched, &terms, &mut chips)?
                }
            };
            if let Some(row) = row {
                results.threads.push(row);
            }
        }
        if query.has_words() {
            results.remote = self.remote(text, &query, &accounts)?;
        }
        Ok(results)
    }

    /// The servers to ask about a query with words, of the accounts it
    /// looked in: those with mail whose bodies are not here, where the
    /// words could be. Gmail is asked in its language, an IMAP server by
    /// `SEARCH`.
    fn remote(
        &self,
        text: &str,
        query: &Either,
        accounts: &[EmailAddress],
    ) -> CommandResult<Option<Remote>> {
        let waiting = self.store.undownloaded_counts()?;
        let providers: HashMap<String, Provider> = self
            .store
            .accounts()?
            .into_iter()
            .map(|a| (a.address.key(), a.provider))
            .collect();
        let ask: Vec<(EmailAddress, usize)> = accounts
            .iter()
            .filter_map(|a| waiting.get(&a.key()).map(|&n| (a.clone(), n)))
            .filter(|(_, n)| *n > 0)
            .collect();
        if ask.is_empty() {
            return Ok(None);
        }
        let mut imap = Vec::new();
        for (account, _) in &ask {
            if providers.get(&account.key()) == Some(&Provider::Imap) {
                imap.push((account.clone(), self.imap_search(text, account)?));
            }
        }
        Ok(Some(Remote {
            query: query.gmail(),
            accounts: ask,
            imap,
        }))
    }

    /// A search result's row: the thread, with where `matched` matched.
    pub(crate) fn search_row(
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
            message: None,
        }))
    }

    /// A search result's row when messages are listed one at a time: the
    /// message, with where it matched.
    pub(crate) fn search_message_row(
        &self,
        account: &EmailAddress,
        id: &ProviderMessageId,
        terms: &[String],
        chips: &mut HashMap<String, HashMap<String, crate::Chip>>,
    ) -> CommandResult<Option<ThreadRow>> {
        let Some(message) = self.store.message(account, id)? else {
            return Ok(None);
        };
        let known = match chips.get(&account.key()) {
            Some(known) => known,
            None => {
                let made = self.account_chips(account)?;
                chips.entry(account.key()).or_insert(made)
            }
        };
        Ok(message_row(&message, known, None, Some(terms)))
    }

    /// People mail was from or to whose address or name holds `part`, for
    /// choosing senders and recipients to search by.
    pub fn people(&self, part: &str, limit: usize) -> CommandResult<Vec<PersonEntry>> {
        Ok(self.store.find_people(part, limit)?)
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
