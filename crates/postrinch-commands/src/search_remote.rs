//! What the servers found for a search, as rows after the local results
//! (search.rs). Gmail's search is Gmail's language, so what it finds
//! holds. An IMAP server's `SEARCH` asks for at least what the query asks
//! (imap_search.rs), so what it finds is checked here against the rest of
//! the query, by the headers stored for it: everything but the words,
//! which only the server could match.

use std::collections::{HashMap, HashSet};

use postrinch_model::{EmailAddress, LabelKind, Provider, ProviderMessageId, Timestamp};
use postrinch_store::Found;

use crate::imap_search::names_only_labels_in;
use crate::query::Last;
use crate::query_or::Either;
use crate::{CommandResult, Commands, Grouping, ThreadRow};

impl Commands {
    /// Rows for messages the provider found that the local search did
    /// not: their threads (or, by [`Grouping::Messages`], the messages
    /// themselves), leaving out those in `listed` (thread or message ids,
    /// as grouped) and, unless the query looks there, the trash and spam.
    /// For an IMAP account, only those the rest of the query holds for.
    /// Marked as found remotely.
    pub fn remote_rows(
        &self,
        query: &str,
        account: &EmailAddress,
        found: &[ProviderMessageId],
        listed: &HashSet<String>,
        grouping: Grouping,
    ) -> CommandResult<Vec<ThreadRow>> {
        self.check(account)?;
        let now = Timestamp::from_millis(jiff::Timestamp::now().as_millisecond());
        let query = Either::parse(query, now)?;
        let set_aside = if query.set_aside_too() {
            HashSet::new()
        } else {
            self.set_aside_ids(account)?
        };
        let imap = self
            .store
            .account(account)?
            .is_some_and(|a| a.provider == Provider::Imap);
        let holds = if imap {
            Some(self.query_holds(&query, account, found)?)
        } else {
            None
        };
        let terms = query.terms();
        let mut chips = HashMap::new();
        let mut seen: HashSet<String> = listed.clone();
        let mut rows = Vec::new();
        for id in found {
            if holds.as_ref().is_some_and(|h| !h.contains(id)) {
                continue;
            }
            let Some(message) = self.store.message(account, id)? else {
                continue;
            };
            let one = match grouping {
                Grouping::Conversations => message.thread.0.clone(),
                Grouping::Messages => message.id.0.clone(),
            };
            if !seen.insert(one) {
                continue;
            }
            if message.labels.iter().any(|l| set_aside.contains(l)) {
                continue;
            }
            let row = match grouping {
                Grouping::Conversations => {
                    self.search_row(account, &message.thread, Some(id), &terms, &mut chips)?
                }
                Grouping::Messages => self.search_message_row(account, id, &terms, &mut chips)?,
            };
            if let Some(mut row) = row {
                row.remote = true;
                rows.push(row);
            }
        }
        Ok(rows)
    }

    /// Of the messages a server found, those every term of an alternative
    /// of the query holds for by what is stored, but the words (matched by
    /// the server, as the bodies are not here) and `has:attachment` (which
    /// only Gmail's own language asks, and a server without it refused).
    fn query_holds(
        &self,
        query: &Either,
        account: &EmailAddress,
        found: &[ProviderMessageId],
    ) -> CommandResult<HashSet<ProviderMessageId>> {
        let within: Vec<(EmailAddress, ProviderMessageId)> = found
            .iter()
            .map(|id| (account.clone(), id.clone()))
            .collect();
        let labels = self.store.labels(account)?;
        let mut holds: Vec<Found> = Vec::new();
        let mut seen = HashSet::new();
        for one in &query.queries {
            if !self.search_accounts(one)?.contains(account) || !names_only_labels_in(one, &labels)
            {
                continue;
            }
            let mut find = self.to_find(one, std::slice::from_ref(account))?;
            find.text = None;
            find.attachments = None;
            find.within = Some(within.clone());
            for f in self.store.find_messages(&find)? {
                if seen.insert(f.id.clone()) {
                    holds.push(f);
                }
            }
        }
        if let Some(last) = query.last()? {
            holds = self.store.whose_last(holds, last == Last::Me)?;
        }
        Ok(holds.into_iter().map(|f| f.id).collect())
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
}
