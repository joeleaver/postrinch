//! Asking the servers too: when a search's words could be in mail whose
//! bodies are not downloaded yet, each account's server searches once for
//! that query (Gmail's own search, or an IMAP server's `SEARCH`; docs/
//! imap.md, "Searching the server"), and what they find that the local
//! search did not is listed after the local results, marked as found on
//! the server. The line about it is [`remote_line`].

use std::collections::HashSet;

use postrinch_commands::{Commands, Remote, SearchResults, ThreadRow};
use postrinch_model::EmailAddress;
use postrinch_sync::SyncError;
use rinch::{Effect, untracked};

use crate::actions::sync;
use crate::state::State;

pub use crate::remote_line::remote_line;

/// How many messages each account's server is asked for.
const REMOTE_MAX: usize = 50;

/// The servers' part of the search being shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSearch {
    /// The search it belongs to, as typed.
    pub query: String,
    /// Who is asked, as the line names them.
    pub asked: Asked,
    pub status: RemoteStatus,
    /// Threads the servers found that the local search did not, newest
    /// first.
    pub rows: Vec<ThreadRow>,
}

/// Who a search asks: Gmail only, one IMAP server, or several servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    Gmail,
    Server,
    Servers,
}

impl Asked {
    /// Who the remote part asks.
    pub fn of(remote: &Remote) -> Self {
        let imap = remote.imap.len();
        match (remote.accounts.len(), imap) {
            (_, 0) => Self::Gmail,
            (1, 1) => Self::Server,
            _ => Self::Servers,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteStatus {
    /// Waiting for the servers; `waiting` messages are not downloaded.
    Asking { waiting: usize },
    /// The servers that answered found `count` more. `notes` says, for
    /// each account, what was not searched, or why its server was not
    /// asked, when something was not.
    Found { count: usize, notes: Vec<Note> },
    /// The server could not be reached, or none of the servers could.
    Offline { waiting: usize },
    /// The one server asked is busy for the account, and asked again later.
    Busy { waiting: usize },
    /// The one server asked would not search as asked: the results are the
    /// mail here. The reason is the server's, or what IMAP cannot ask.
    Refused(String),
    /// The one server asked could not be asked.
    Failed(String),
    /// None of the several servers asked could search, each for the
    /// reason its note gives.
    Unanswered { waiting: usize, notes: Vec<Note> },
}

/// What one account's server search left out, or why it did not search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub account: EmailAddress,
    pub what: Trouble,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trouble {
    /// It searched, but not everywhere: what was not searched, and why.
    Incomplete(String),
    /// Its server could not be reached.
    Offline,
    /// Its server is busy for the account.
    Busy,
    /// Its server would not search as asked, and why.
    Refused(String),
    /// It could not be asked, and why.
    Failed(String),
}

impl State {
    /// Ask the servers whenever a new search's results say they should be.
    pub fn watch_remote(self) {
        Effect::new(move || {
            let Some(results) = self.results.get() else {
                untracked(|| self.remote.set(None));
                return;
            };
            let asked = untracked(|| {
                self.remote
                    .with(|r| r.as_ref().is_some_and(|r| r.query == results.query))
            });
            match results.remote.clone() {
                Some(remote) if !asked => untracked(|| self.ask_servers(&results, remote)),
                Some(_) => {}
                None => untracked(|| self.remote.set(None)),
            }
        });
    }

    fn ask_servers(self, results: &SearchResults, remote: Remote) {
        let waiting = remote.accounts.iter().map(|(_, n)| n).sum();
        let query = results.query.clone();
        let asked = Asked::of(&remote);
        let Some(sync) = sync() else {
            self.remote.set(Some(RemoteSearch {
                query,
                asked,
                status: RemoteStatus::Offline { waiting },
                rows: Vec::new(),
            }));
            return;
        };
        self.remote.set(Some(RemoteSearch {
            query: query.clone(),
            asked,
            status: RemoteStatus::Asking { waiting },
            rows: Vec::new(),
        }));
        // What the local search listed, by thread or by message as grouped.
        let grouping = self.grouping();
        let listed: Vec<(EmailAddress, String)> = results
            .threads
            .iter()
            .map(|r| {
                let one = r.message.as_ref().map_or(&r.summary.id.0, |m| &m.0).clone();
                (r.summary.account.clone(), one)
            })
            .collect();
        let signal = self.remote;
        let mail = sync.mail().clone();
        sync.runtime.clone().spawn(async move {
            let mut rows: Vec<ThreadRow> = Vec::new();
            let mut notes: Vec<Note> = Vec::new();
            let mut answered = 0;
            for (account, _) in &remote.accounts {
                let found = mail
                    .search_server(
                        account,
                        &remote.query,
                        remote.imap_search(account),
                        REMOTE_MAX,
                    )
                    .await;
                let found = match found {
                    Ok(found) => found,
                    // A newer search took this one's place; it says what
                    // the servers found.
                    Err(SyncError::SearchSuperseded) => return,
                    Err(e) => {
                        notes.push(Note {
                            account: account.clone(),
                            what: trouble_of(account, e),
                        });
                        continue;
                    }
                };
                let here: HashSet<String> = listed
                    .iter()
                    .filter(|(a, _)| a == account)
                    .map(|(_, t)| t.clone())
                    .collect();
                let (store, q, a) = (sync.store.clone(), query.clone(), account.clone());
                let ids = found.ids;
                let checked = tokio::task::spawn_blocking(move || {
                    Commands::new(store).remote_rows(&q, &a, &ids, &here, grouping)
                })
                .await;
                let failed = match checked {
                    Ok(Ok(found)) => {
                        rows.extend(found);
                        None
                    }
                    Ok(Err(e)) => Some(e.to_string()),
                    Err(e) => Some(e.to_string()),
                };
                if let Some(why) = failed {
                    tracing::warn!("server search for {account}: {why}");
                    notes.push(Note {
                        account: account.clone(),
                        what: Trouble::Failed(why),
                    });
                    continue;
                }
                answered += 1;
                if let Some(why) = found.incomplete {
                    notes.push(Note {
                        account: account.clone(),
                        what: Trouble::Incomplete(why),
                    });
                }
            }
            rows.sort_by_key(|r| std::cmp::Reverse(r.summary.latest));
            let status = if answered > 0 {
                RemoteStatus::Found {
                    count: rows.len(),
                    notes,
                }
            } else {
                unanswered(notes, waiting)
            };
            signal.update_send(move |r| {
                if let Some(r) = r
                    && r.query == query
                {
                    r.status = status;
                    r.rows = rows;
                }
            });
        });
    }
}

/// The status when no server searched: one server's trouble as it is,
/// several offline as offline, otherwise each one's.
fn unanswered(mut notes: Vec<Note>, waiting: usize) -> RemoteStatus {
    if notes.iter().all(|n| n.what == Trouble::Offline) {
        return RemoteStatus::Offline { waiting };
    }
    if notes.len() > 1 {
        return RemoteStatus::Unanswered { waiting, notes };
    }
    match notes.pop().map(|n| n.what) {
        Some(Trouble::Busy) => RemoteStatus::Busy { waiting },
        Some(Trouble::Refused(why)) => RemoteStatus::Refused(why),
        Some(Trouble::Failed(why) | Trouble::Incomplete(why)) => RemoteStatus::Failed(why),
        Some(Trouble::Offline) | None => RemoteStatus::Offline { waiting },
    }
}

/// What a server search's failure means for the line.
fn trouble_of(account: &EmailAddress, e: SyncError) -> Trouble {
    match e {
        SyncError::Http(_) | SyncError::Unreachable { .. } => {
            tracing::info!("server search for {account}: {e}");
            Trouble::Offline
        }
        SyncError::Busy { .. } => {
            tracing::info!("server search for {account}: {e}");
            Trouble::Busy
        }
        SyncError::SearchRefused { message, .. } => {
            tracing::info!("server search for {account} refused: {message}");
            Trouble::Refused(message)
        }
        e => {
            tracing::warn!("server search for {account}: {e}");
            Trouble::Failed(e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(account: &str, what: Trouble) -> Note {
        Note {
            account: EmailAddress::parse(account).unwrap(),
            what,
        }
    }

    #[test]
    fn one_servers_trouble_is_the_status() {
        let busy = vec![note("a@x.org", Trouble::Busy)];
        assert_eq!(unanswered(busy, 3), RemoteStatus::Busy { waiting: 3 });
        let refused = vec![note("a@x.org", Trouble::Refused("why".into()))];
        assert_eq!(unanswered(refused, 3), RemoteStatus::Refused("why".into()));
    }

    #[test]
    fn several_servers_troubles_are_each_kept() {
        let offline = vec![
            note("a@x.org", Trouble::Offline),
            note("b@x.org", Trouble::Offline),
        ];
        assert_eq!(unanswered(offline, 2), RemoteStatus::Offline { waiting: 2 });
        let mixed = vec![
            note("a@x.org", Trouble::Busy),
            note("b@x.org", Trouble::Offline),
        ];
        assert_eq!(
            unanswered(mixed.clone(), 2),
            RemoteStatus::Unanswered {
                waiting: 2,
                notes: mixed
            }
        );
    }
}
