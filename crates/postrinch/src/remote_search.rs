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
    Asking {
        waiting: usize,
    },
    /// They found `count` more; `incomplete` says what was not searched,
    /// when something was not.
    Found {
        count: usize,
        incomplete: Option<String>,
    },
    /// The servers could not be reached.
    Offline {
        waiting: usize,
    },
    /// A server is busy for the account, and asked again later.
    Busy {
        waiting: usize,
    },
    /// A server would not search as asked: the results are the mail
    /// here. The reason is the server's, or what IMAP cannot ask.
    Refused(String),
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
            let mut trouble: Option<RemoteStatus> = None;
            let mut incomplete: Option<String> = None;
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
                    Err(e) => {
                        trouble = Some(trouble_of(account, e, waiting));
                        continue;
                    }
                };
                incomplete = incomplete.or(found.incomplete);
                let here: HashSet<String> = listed
                    .iter()
                    .filter(|(a, _)| a == account)
                    .map(|(_, t)| t.clone())
                    .collect();
                let (store, q, a) = (sync.store.clone(), query.clone(), account.clone());
                let ids = found.ids;
                let found = tokio::task::spawn_blocking(move || {
                    Commands::new(store).remote_rows(&q, &a, &ids, &here, grouping)
                })
                .await;
                match found {
                    Ok(Ok(found)) => rows.extend(found),
                    Ok(Err(e)) => trouble = Some(RemoteStatus::Failed(e.to_string())),
                    Err(e) => trouble = Some(RemoteStatus::Failed(e.to_string())),
                }
            }
            rows.sort_by_key(|r| std::cmp::Reverse(r.summary.latest));
            let status = match trouble {
                Some(trouble) if rows.is_empty() => trouble,
                _ => RemoteStatus::Found {
                    count: rows.len(),
                    incomplete,
                },
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

/// What a server search's failure means for the line.
fn trouble_of(account: &EmailAddress, e: SyncError, waiting: usize) -> RemoteStatus {
    match e {
        SyncError::Http(_) | SyncError::Unreachable { .. } => {
            tracing::info!("server search for {account}: {e}");
            RemoteStatus::Offline { waiting }
        }
        SyncError::Busy { .. } => {
            tracing::info!("server search for {account}: {e}");
            RemoteStatus::Busy { waiting }
        }
        SyncError::SearchRefused { message, .. } => {
            tracing::info!("server search for {account} refused: {message}");
            RemoteStatus::Refused(message)
        }
        e => {
            tracing::warn!("server search for {account}: {e}");
            RemoteStatus::Failed(e.to_string())
        }
    }
}
