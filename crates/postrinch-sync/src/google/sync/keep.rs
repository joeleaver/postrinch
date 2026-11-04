//! Keeping every Google account in sync for as long as the app runs.
//!
//! One pass syncs each signed-in account, then downloads the bodies its
//! sync left as headers. Passes repeat every [`KeepOptions::interval`], or
//! after [`KeepOptions::retry`] when an account failed: a network that drops
//! for minutes at a time costs a wait, not the sync.

use std::sync::Arc;
use std::time::Duration;

use postrinch_model::{EmailAddress, Provider};
use tokio::sync::Notify;

use super::{GmailSync, Progress, ProgressFn, SendReport, SyncReport};
use crate::SyncError;

/// What a running sync reports, for a window or a log to show.
#[derive(Debug, Clone)]
pub enum SyncEvent {
    /// An account's pass began.
    Started { account: EmailAddress },
    Progress {
        account: EmailAddress,
        progress: Progress,
    },
    /// An account's mail is up to date with Gmail; its bodies download next.
    Synced {
        account: EmailAddress,
        report: SyncReport,
    },
    /// An account's pass finished: synced, and every body downloaded.
    Done {
        account: EmailAddress,
        downloaded: usize,
    },
    /// An account's pass failed; it is tried again after `retry_in`.
    Failed {
        account: EmailAddress,
        error: String,
        retry_in: Duration,
    },
    /// Gmail no longer accepts the account's sign-in. It is marked so in
    /// the store and left alone until it signs in again.
    NeedsSignIn { account: EmailAddress },
    /// Changes made here reached Gmail; any it refused for good were
    /// dropped, and their messages read again.
    Sent {
        account: EmailAddress,
        report: SendReport,
        /// How many messages and drafts written here went.
        mail: usize,
    },
    /// Sending an account's changes failed; they are tried again after
    /// `retry_in`, and stay applied here meanwhile.
    SendFailed {
        account: EmailAddress,
        error: String,
        retry_in: Duration,
    },
}

pub type SyncEventFn = Arc<dyn Fn(SyncEvent) + Send + Sync>;

#[derive(Debug, Clone, Copy)]
pub struct KeepOptions {
    /// The pause between passes when every account succeeded.
    pub interval: Duration,
    /// The pause before the next pass when an account failed.
    pub retry: Duration,
}

impl Default for KeepOptions {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(60),
            retry: Duration::from_secs(20),
        }
    }
}

impl GmailSync {
    /// Sync every signed-in Google account, again and again, until the
    /// future is dropped. Notifying `wake` starts the next pass at once:
    /// after an account signs in, or when the person asks.
    pub async fn keep_in_sync(&self, options: KeepOptions, events: SyncEventFn, wake: Arc<Notify>) {
        loop {
            let pause = if self.pass(&options, &events).await {
                options.interval
            } else {
                options.retry
            };
            tokio::select! {
                () = tokio::time::sleep(pause) => {}
                () = wake.notified() => {}
            }
        }
    }

    /// Send every account's changes to Gmail, again and again, until the
    /// future is dropped. It runs beside [`Self::keep_in_sync`], so a long
    /// first sync never holds a change back. Notifying `wake` sends at once:
    /// after a change is made. What failed is tried again after
    /// [`KeepOptions::retry`].
    pub async fn keep_sending(&self, options: KeepOptions, events: SyncEventFn, wake: Arc<Notify>) {
        loop {
            let pause = if self.send_pass(&options, &events).await {
                // Nothing waits: only a new change brings the next pass.
                Duration::from_secs(3600)
            } else {
                options.retry
            };
            tokio::select! {
                () = tokio::time::sleep(pause) => {}
                () = wake.notified() => {}
            }
        }
    }

    /// Send each account's waiting changes. Returns whether all were sent.
    async fn send_pass(&self, options: &KeepOptions, events: &SyncEventFn) -> bool {
        let accounts = match self.with_store(|s| s.accounts()).await {
            Ok(accounts) => accounts,
            Err(e) => {
                tracing::error!("cannot read the accounts to send changes for: {e}");
                return false;
            }
        };
        let mut all_sent = true;
        for account in accounts {
            if account.provider != Provider::Google || !account.is_active() {
                continue;
            }
            let address = account.address;
            let waiting = match (
                self.waiting(&address).await,
                self.outgoing_waiting(&address).await,
            ) {
                (Ok(changes), Ok(mail)) => Ok(changes + mail),
                (Err(e), _) | (_, Err(e)) => Err(e),
            };
            match waiting {
                Ok(0) => continue,
                Ok(_) => {}
                Err(e) => {
                    tracing::error!("{address}: cannot read its outbox: {e}");
                    all_sent = false;
                    continue;
                }
            }
            // Changes first, then mail, each in the order made.
            let sent = match self.send_outbox(&address).await {
                Ok(report) => self
                    .send_outgoing(&address)
                    .await
                    .map(|mail| (report, mail)),
                Err(e) => Err(e),
            };
            match sent {
                Ok((report, mail)) => events(SyncEvent::Sent {
                    account: address,
                    report,
                    mail,
                }),
                Err(SyncError::NeedsSignIn(_)) => {
                    events(SyncEvent::NeedsSignIn { account: address });
                }
                Err(e) => {
                    all_sent = false;
                    events(SyncEvent::SendFailed {
                        account: address,
                        error: e.to_string(),
                        retry_in: options.retry,
                    });
                }
            }
        }
        all_sent
    }

    /// One pass over the accounts. Returns whether every account succeeded.
    async fn pass(&self, options: &KeepOptions, events: &SyncEventFn) -> bool {
        let accounts = match self.with_store(|s| s.accounts()).await {
            Ok(accounts) => accounts,
            Err(e) => {
                tracing::error!("cannot read the accounts to sync: {e}");
                return false;
            }
        };
        let mut all_ok = true;
        for account in accounts {
            if account.provider != Provider::Google || !account.is_active() {
                continue;
            }
            let address = account.address;
            events(SyncEvent::Started {
                account: address.clone(),
            });
            match self.sync_and_download(&address, events).await {
                Ok(downloaded) => events(SyncEvent::Done {
                    account: address,
                    downloaded,
                }),
                Err(SyncError::NeedsSignIn(_)) => {
                    events(SyncEvent::NeedsSignIn { account: address })
                }
                Err(e) => {
                    all_ok = false;
                    events(SyncEvent::Failed {
                        account: address,
                        error: e.to_string(),
                        retry_in: options.retry,
                    });
                }
            }
        }
        all_ok
    }

    async fn sync_and_download(
        &self,
        account: &EmailAddress,
        events: &SyncEventFn,
    ) -> Result<usize, SyncError> {
        let progress: ProgressFn = {
            let (events, account) = (events.clone(), account.clone());
            Arc::new(move |progress| {
                events(SyncEvent::Progress {
                    account: account.clone(),
                    progress,
                })
            })
        };
        let report = self.sync(account, progress.clone()).await?;
        events(SyncEvent::Synced {
            account: account.clone(),
            report,
        });
        Ok(self.backfill(account, progress).await?.fetched)
    }
}
