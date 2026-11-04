//! Asking Gmail too: when a search's words could be in mail whose bodies
//! are not downloaded yet, Gmail's own search runs once for that query, and
//! what it finds that the local search did not is listed after the local
//! results, marked as found by Gmail.

use std::collections::HashSet;

use postrinch_commands::{Commands, Remote, SearchResults, ThreadRow};
use postrinch_model::{EmailAddress, ProviderThreadId};
use postrinch_sync::SyncError;
use rinch::{Effect, untracked};

use crate::actions::sync;
use crate::state::State;

/// How many messages Gmail is asked for, per account.
const REMOTE_MAX: usize = 50;

/// Gmail's part of the search being shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSearch {
    /// The search it belongs to, as typed.
    pub query: String,
    pub status: RemoteStatus,
    /// Threads Gmail found that the local search did not, newest first.
    pub rows: Vec<ThreadRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteStatus {
    /// Waiting for Gmail; `waiting` messages are not downloaded.
    Asking {
        waiting: usize,
    },
    Found(usize),
    /// Gmail could not be reached.
    Offline {
        waiting: usize,
    },
    Failed(String),
}

impl State {
    /// Ask Gmail whenever a new search's results say it should be.
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
                Some(remote) if !asked => untracked(|| self.ask_gmail(&results, remote)),
                Some(_) => {}
                None => untracked(|| self.remote.set(None)),
            }
        });
    }

    fn ask_gmail(self, results: &SearchResults, remote: Remote) {
        let waiting = remote.accounts.iter().map(|(_, n)| n).sum();
        let query = results.query.clone();
        let Some(sync) = sync() else {
            self.remote.set(Some(RemoteSearch {
                query,
                status: RemoteStatus::Offline { waiting },
                rows: Vec::new(),
            }));
            return;
        };
        self.remote.set(Some(RemoteSearch {
            query: query.clone(),
            status: RemoteStatus::Asking { waiting },
            rows: Vec::new(),
        }));
        let listed: Vec<(EmailAddress, ProviderThreadId)> = results
            .threads
            .iter()
            .map(|r| (r.summary.account.clone(), r.summary.id.clone()))
            .collect();
        let signal = self.remote;
        sync.runtime.clone().spawn(async move {
            let mut rows: Vec<ThreadRow> = Vec::new();
            let mut trouble: Option<RemoteStatus> = None;
            for (account, _) in &remote.accounts {
                let ids = match sync
                    .sync_engine()
                    .search_remote(account, &remote.query, REMOTE_MAX)
                    .await
                {
                    Ok(ids) => ids,
                    Err(SyncError::Http(e)) => {
                        tracing::info!("Gmail search for {account}: {e}");
                        trouble = Some(RemoteStatus::Offline { waiting });
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!("Gmail search for {account}: {e}");
                        trouble = Some(RemoteStatus::Failed(e.to_string()));
                        continue;
                    }
                };
                let here: HashSet<ProviderThreadId> = listed
                    .iter()
                    .filter(|(a, _)| a == account)
                    .map(|(_, t)| t.clone())
                    .collect();
                let (store, q, a) = (sync.store.clone(), query.clone(), account.clone());
                let found = tokio::task::spawn_blocking(move || {
                    Commands::new(store).remote_rows(&q, &a, &ids, &here)
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
                _ => RemoteStatus::Found(rows.len()),
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

/// The line under a search's heading about Gmail's part, if it has one.
pub fn remote_line(remote: Option<&RemoteSearch>) -> String {
    let Some(remote) = remote else {
        return String::new();
    };
    let messages = |n: usize| {
        if n == 1 {
            "1 message".to_owned()
        } else {
            format!("{} messages", crate::sync::thousands(n))
        }
    };
    match &remote.status {
        RemoteStatus::Asking { waiting } => format!(
            "Asking Gmail too: {} here are not downloaded yet\u{2026}",
            messages(*waiting)
        ),
        RemoteStatus::Found(0) => "Gmail found nothing more in mail not downloaded yet.".to_owned(),
        RemoteStatus::Found(1) => "Gmail found 1 more in mail not downloaded yet.".to_owned(),
        RemoteStatus::Found(n) => {
            format!("Gmail found {n} more in mail not downloaded yet.")
        }
        RemoteStatus::Offline { waiting } => format!(
            "Offline: mail not downloaded yet ({}) is searched by subject and sender only.",
            messages(*waiting)
        ),
        RemoteStatus::Failed(why) => format!("Gmail could not be asked: {why}"),
    }
}
