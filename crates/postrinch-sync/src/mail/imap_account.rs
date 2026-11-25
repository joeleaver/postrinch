//! One IMAP account's loops: syncing it, and sending what is made here
//! (docs/imap.md, "Sync" and "Writing and sending").
//!
//! - A server that turns the account away for now
//!   ([`SyncError::Busy`]) is waited out: the account's server's
//!   [`Backoff`](crate::Backoff) gives the wait (`BusyGate`),
//!   [`SyncEvent::Busy`] says so with the server's words, and nothing is
//!   asked of that server until the wait is over, a wake included. Its
//!   sign-in is fine, so it is never marked as needing to sign in. A pass
//!   or send that signs in resets the wait.
//! - A refused sign-in ([`SyncError::NeedsSignIn`], which the engine has
//!   recorded on the account) ends the loop: the account is left alone
//!   until it signs in again, when its keeper starts a loop anew.
//! - Anything else is tried again after [`KeepOptions::retry`], as for
//!   Google accounts ([`SyncEvent::Failed`], [`SyncEvent::SendFailed`]).
//!
//! Until IDLE tells of new mail at once (IMAP 11), the account is polled
//! every [`KeepOptions::interval`]; a wake starts a pass at once.

use std::sync::Arc;
use std::time::Duration;

use postrinch_model::EmailAddress;
use tokio::sync::Notify;

use super::imap_keep::ImapKeeper;
use crate::{KeepOptions, ProgressFn, SendReport, SyncError, SyncEvent};

/// The services an account's servers are named by, as
/// [`SyncError::Busy`] names them.
const IMAP: &str = "IMAP";
const SMTP: &str = "SMTP";

/// What a loop does before its next turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    /// Wait this long, or less if woken.
    Pause(Duration),
    /// Wait until woken.
    UntilWoken,
    /// Wait this long, wake or no wake: a busy server.
    WaitOut(Duration),
    /// The account must sign in again (or is gone): the loop ends.
    Stop,
}

impl ImapKeeper {
    /// Syncs the account, then downloads the bodies the sync left, again
    /// and again, until the account is removed or must sign in again.
    pub(super) async fn keep_account_in_sync(&self, account: &EmailAddress, wake: Arc<Notify>) {
        loop {
            if !self.still_kept(account).await {
                return;
            }
            let next = match self.busy.remaining(account, IMAP) {
                Some(wait) => Next::WaitOut(wait),
                None => self.sync_pass(account).await,
            };
            if !self.wait(next, &wake).await {
                return;
            }
        }
    }

    /// Sends the account's waiting changes, then its waiting mail, each
    /// time it is woken (a change made, mail written), until the account
    /// is removed or must sign in again. What failed is tried again after
    /// [`KeepOptions::retry`].
    pub(super) async fn keep_account_sending(&self, account: &EmailAddress, wake: Arc<Notify>) {
        loop {
            if !self.still_kept(account).await {
                return;
            }
            let next = self.send_turn(account).await;
            if !self.wait(next, &wake).await {
                return;
            }
        }
    }

    /// Waits as `next` says. Returns whether the loop goes on.
    async fn wait(&self, next: Next, wake: &Notify) -> bool {
        match next {
            Next::Pause(pause) => {
                tokio::select! {
                    () = tokio::time::sleep(pause) => {}
                    () = wake.notified() => {}
                }
            }
            Next::UntilWoken => wake.notified().await,
            Next::WaitOut(wait) => tokio::time::sleep(wait).await,
            Next::Stop => return false,
        }
        true
    }

    /// One pass: sync, then backfill.
    async fn sync_pass(&self, account: &EmailAddress) -> Next {
        self.emit(SyncEvent::Started {
            account: account.clone(),
        });
        match self.sync_and_download(account).await {
            Ok(downloaded) => {
                self.emit(SyncEvent::Done {
                    account: account.clone(),
                    downloaded,
                });
                Next::Pause(self.options.interval)
            }
            Err(e) => self.failed(account, e, |error, retry_in| SyncEvent::Failed {
                account: account.clone(),
                error,
                retry_in,
            }),
        }
    }

    async fn sync_and_download(&self, account: &EmailAddress) -> Result<usize, SyncError> {
        let progress: ProgressFn = {
            let (events, account) = (self.events.clone(), account.clone());
            Arc::new(move |progress| {
                events(SyncEvent::Progress {
                    account: account.clone(),
                    progress,
                })
            })
        };
        let report = self.sync.sync(account, progress.clone()).await?;
        // The pass signed in and read the folders: a busy wait is over.
        self.busy.reset(account, IMAP);
        self.emit(SyncEvent::Synced {
            account: account.clone(),
            report,
        });
        Ok(self.sync.backfill(account, progress).await?.fetched)
    }

    /// One turn of sending: the outbox over IMAP, then outgoing mail over
    /// SMTP (and IMAP, to file it), each only when something waits and
    /// its server is not being waited out.
    async fn send_turn(&self, account: &EmailAddress) -> Next {
        let (changes, mail) = match (
            self.sync.waiting(account).await,
            self.sync.outgoing_waiting(account).await,
        ) {
            (Ok(changes), Ok(mail)) => (changes, mail),
            (Err(e), _) | (_, Err(e)) => {
                tracing::error!("{account}: cannot read its outbox: {e}");
                return Next::Pause(self.options.retry);
            }
        };
        if changes + mail == 0 {
            return Next::UntilWoken;
        }
        // Both need the IMAP server: filing sent mail and keeping drafts
        // are IMAP's.
        if let Some(wait) = self.busy.remaining(account, IMAP) {
            return Next::WaitOut(wait);
        }
        let report = if changes > 0 {
            match self.sync.send_outbox(account).await {
                Ok(report) => {
                    self.busy.reset(account, IMAP);
                    report
                }
                Err(e) => return self.send_failed(account, e),
            }
        } else {
            SendReport::default()
        };
        let delivered = if mail == 0 {
            Ok(0)
        } else if let Some(wait) = self.busy.remaining(account, SMTP) {
            Err(Next::WaitOut(wait))
        } else {
            match self.sync.send_outgoing(account).await {
                Ok(delivered) => {
                    self.busy.reset(account, SMTP);
                    Ok(delivered)
                }
                Err(e) => Err(self.send_failed(account, e)),
            }
        };
        match delivered {
            Ok(mail) => {
                self.emit(SyncEvent::Sent {
                    account: account.clone(),
                    report,
                    mail,
                });
                Next::UntilWoken
            }
            Err(next) => {
                // The changes that went are news all the same: those the
                // server refused for good are shown as it has them.
                if report.sent > 0 || !report.refused.is_empty() {
                    self.emit(SyncEvent::Sent {
                        account: account.clone(),
                        report,
                        mail: 0,
                    });
                }
                next
            }
        }
    }

    fn send_failed(&self, account: &EmailAddress, e: SyncError) -> Next {
        self.failed(account, e, |error, retry_in| SyncEvent::SendFailed {
            account: account.clone(),
            error,
            retry_in,
        })
    }

    /// Says what failed and how long to wait: a busy server's wait from
    /// its backoff, nothing more for a refused sign-in, else the ordinary
    /// retry, reported by `other`.
    fn failed(
        &self,
        account: &EmailAddress,
        error: SyncError,
        other: impl FnOnce(String, Duration) -> SyncEvent,
    ) -> Next {
        match error {
            SyncError::NeedsSignIn(_) => {
                self.emit(SyncEvent::NeedsSignIn {
                    account: account.clone(),
                });
                Next::Stop
            }
            SyncError::Busy {
                service, message, ..
            } => {
                let retry_in = self.busy.busy(account, service);
                self.emit(SyncEvent::Busy {
                    account: account.clone(),
                    message,
                    retry_in,
                });
                Next::WaitOut(retry_in)
            }
            error => {
                let KeepOptions { retry, .. } = self.options;
                self.emit(other(error.to_string(), retry));
                Next::Pause(retry)
            }
        }
    }

    fn emit(&self, event: SyncEvent) {
        (self.events)(event);
    }
}
