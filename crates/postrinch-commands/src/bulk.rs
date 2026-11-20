//! Changing every thread a search matches, as selecting all of a search
//! and acting does in the window, however many there are.

use std::collections::HashSet;

use postrinch_model::{EmailAddress, Timestamp};

use crate::query_or::Either;
use crate::{CommandError, CommandResult, Commands, ThreadRef};

impl Commands {
    /// Every thread `query` (the search language) matches here: its
    /// conversations, newest first. Unlike [`Commands::search`] there is no
    /// limit, and every word match counts, not only the most relevant.
    pub fn search_threads(&self, query: &str) -> CommandResult<Vec<ThreadRef>> {
        let either = self.whole_search(query)?;
        self.threads_found(&either)
    }

    /// When `query` has words: the accounts it looks in whose bodies are
    /// not all here, with how many messages, as words are not looked for in
    /// those.
    pub fn words_not_searched(&self, query: &str) -> CommandResult<Vec<(EmailAddress, usize)>> {
        let either = self.whole_search(query)?;
        if !either.has_words() {
            return Ok(Vec::new());
        }
        let mut accounts: Vec<EmailAddress> = Vec::new();
        for one in &either.queries {
            accounts.extend(self.search_accounts(one)?);
        }
        accounts.dedup();
        let waiting = self.store.undownloaded_counts()?;
        Ok(accounts
            .iter()
            .filter_map(|a| waiting.get(&a.key()).map(|&n| (a.clone(), n)))
            .filter(|(_, n)| *n > 0)
            .collect())
    }

    fn whole_search(&self, query: &str) -> CommandResult<Either> {
        let text = query.trim();
        if text.is_empty() {
            return Err(CommandError::Invalid(
                "Give a search: an empty one would be every message.".into(),
            ));
        }
        let now = Timestamp::from_millis(jiff::Timestamp::now().as_millisecond());
        Either::parse(text, now)
    }

    fn threads_found(&self, either: &Either) -> CommandResult<Vec<ThreadRef>> {
        let (found, _) = self.found(either, true)?;
        let mut seen: HashSet<(String, String)> = HashSet::new();
        let mut threads = Vec::new();
        for f in found {
            let Some(thread) = self.store.found_thread(&f)? else {
                continue;
            };
            if seen.insert((f.account.key(), thread.0.clone())) {
                threads.push((f.account, thread));
            }
        }
        Ok(threads)
    }
}
