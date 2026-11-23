//! Mail that has just come in, for the window to tell the person about:
//! unread mail the chosen views show (each by its own search, so exactly
//! what it lists) that arrived since the window last looked.

use postrinch_model::{EmailAddress, ProviderMessageId, ProviderThreadId, Timestamp};

use crate::query_or::Either;
use crate::views::now;
use crate::{CommandResult, Commands};

/// A message to tell the person about. What it says is its sender's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arrived {
    /// The name of the view it came in, the first one when several show it.
    pub view: String,
    pub account: EmailAddress,
    pub thread: ProviderThreadId,
    pub message: ProviderMessageId,
    pub received: Timestamp,
    /// Its sender's name, or address.
    pub from: String,
    pub subject: String,
    pub snippet: String,
    /// It carries an invitation.
    pub invitation: bool,
}

/// A view to tell the person about: its name, search and accounts (none
/// for every account).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watched {
    pub name: String,
    pub query: String,
    pub accounts: Vec<EmailAddress>,
}

impl Commands {
    /// Unread mail the `views` show that arrived at or after `since`,
    /// newest first, each once, at most `limit`. `known`: only from people
    /// the person has written to.
    pub fn arrived(
        &self,
        views: &[Watched],
        since: Timestamp,
        known: bool,
        limit: usize,
    ) -> CommandResult<Vec<Arrived>> {
        let mut out: Vec<Arrived> = Vec::new();
        for (i, view) in views.iter().enumerate() {
            // An account's view is in one of every account with its search.
            let covered = !view.accounts.is_empty()
                && views[..i]
                    .iter()
                    .any(|v| v.accounts.is_empty() && v.query == view.query);
            if covered {
                continue;
            }
            let query = Either::parse(&format!("({}) is:unread", view.query), now())?;
            for f in self.found_since(&query, since, &view.accounts)? {
                if out
                    .iter()
                    .any(|a| a.account == f.account && a.message == f.id)
                {
                    continue;
                }
                let Some(m) = self.store.message(&f.account, &f.id)? else {
                    continue;
                };
                let sender = m.headers.from.first().cloned();
                if known && !sender.as_ref().is_some_and(|s| self.wrote_to(&s.address)) {
                    continue;
                }
                out.push(Arrived {
                    view: view.name.clone(),
                    from: sender.map(|m| m.display_name()).unwrap_or_default(),
                    subject: m.subject().to_owned(),
                    snippet: m.snippet.clone(),
                    invitation: crate::may_invite(&m),
                    account: f.account,
                    thread: m.thread,
                    message: f.id,
                    received: f.received,
                });
            }
        }
        out.sort_by_key(|a| std::cmp::Reverse(a.received));
        out.truncate(limit);
        Ok(out)
    }

    /// What `query` matches in `only` (every account for none) that
    /// arrived at or after `since`: the store looks only that far back.
    fn found_since(
        &self,
        query: &Either,
        since: Timestamp,
        only: &[EmailAddress],
    ) -> CommandResult<Vec<postrinch_store::Found>> {
        let mut found = Vec::new();
        for one in &query.queries {
            let mut these = self.search_accounts(one)?;
            if !only.is_empty() {
                these.retain(|a| only.contains(a));
                if these.is_empty() {
                    continue;
                }
            }
            let mut find = self.to_find(one, &these)?;
            find.after = Some(find.after.map_or(since, |a| a.max(since)));
            found.extend(self.store.find_messages(&find)?);
        }
        Ok(found)
    }

    /// Whether any account has sent mail to `address`.
    fn wrote_to(&self, address: &EmailAddress) -> bool {
        let Ok(query) = Either::parse(&format!("in:sent to:{address}"), now()) else {
            return false;
        };
        self.found(&query, false)
            .is_ok_and(|(found, _)| !found.is_empty())
    }
}
