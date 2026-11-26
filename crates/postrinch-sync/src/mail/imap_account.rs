//! One IMAP account's loops: syncing it, and sending what is made here
//! (docs/imap.md, "Sync" and "Writing and sending").
//!
//! - A server that turns the account away for now
//!   ([`SyncError::Busy`]) is waited out: the account's server's
//!   [`Backoff`](crate::Backoff) gives the wait (`BusyGate`),
//!   [`SyncEvent::Busy`] says so with the server's words, and nothing is
//!   asked of that server until the wait is over, a wake included: a
//!   wake only makes the loop look at the wait again. Its sign-in is
//!   fine, so it is never marked as needing to sign in. A pass or send
//!   that signs in resets the wait; so does the account's removal, or its
//!   servers' check when it is added again, and the next wake then finds
//!   the account gone, or no wait left.
//! - A refused sign-in ([`SyncError::NeedsSignIn`], which the engine has
//!   recorded on the account) ends the loop: the account is left alone
//!   until it signs in again, when its keeper starts a loop anew.
//! - Anything else is tried again after [`KeepOptions::retry`], as for
//!   Google accounts ([`SyncEvent::Failed`], [`SyncEvent::SendFailed`]).
//!
//! The account is polled every [`KeepOptions::interval`], or every
//! [`crate::IdleOptions::interval`] while an IDLE on its inbox tells of
//! news at once (`imap_idle`); a wake starts a pass at once. A pass's
//! backfill gives way between batches once the next pass is due, woken,
//! or with the interval gone by since backfill began (`giving_way`):
//! that pass runs at once, with no [`SyncEvent::Done`] for the one that
//! gave way, and backfill carries on after its sync, its count with it.
//! A pass whose backfill took a wake is followed at once by the next,
//! however it ended, as the wake is used up.

use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use postrinch_model::EmailAddress;
use tokio::sync::Notify;
use tokio::time::Instant;

use super::imap_idle::Watching;
use super::imap_keep::ImapKeeper;
use crate::giving_way::{Share, Tally};
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
    /// Begin the next turn now: a pass is due that the last one's
    /// backfill gave way to, or whose wake it took.
    Again,
    /// Wait this long, or until woken, and then look at the busy
    /// server's wait again: a wake never cuts the wait short, but one that
    /// was forgotten meanwhile (the account removed, or added again) ends.
    WaitOut(Duration),
    /// The account must sign in again (or is gone): the loop ends.
    Stop,
}

impl ImapKeeper {
    /// Syncs the account, then downloads the bodies the sync left, again
    /// and again, until the account is removed or must sign in again;
    /// beside it, an IDLE on the inbox wakes it for news
    /// ([`Self::keep_idling`]), and ends with it.
    pub(super) async fn keep_account_in_sync(&self, account: &EmailAddress, wake: Arc<Notify>) {
        let watching = Watching::new();
        tokio::select! {
            () = self.sync_loop(account, &wake, &watching) => {}
            () = self.keep_idling(account, &wake, &watching) => {}
        }
    }

    async fn sync_loop(&self, account: &EmailAddress, wake: &Notify, watching: &Watching) {
        let (mut tally, mut share) = (Tally::default(), Share::default());
        loop {
            if !self.still_kept(account).await {
                return;
            }
            let next = match self.busy.remaining(account, IMAP) {
                Some(wait) => Next::WaitOut(wait),
                None => {
                    self.sync_pass(account, wake, watching, &mut tally, &mut share)
                        .await
                }
            };
            if !self.wait(next, wake).await {
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
            // A busy server's wait is looked at again at the start of the
            // next turn, before the server is asked anything.
            Next::Pause(pause) | Next::WaitOut(pause) => {
                tokio::select! {
                    () = tokio::time::sleep(pause) => {}
                    () = wake.notified() => {}
                }
            }
            Next::UntilWoken => wake.notified().await,
            // A wake that came since the backfill gave way asked for the
            // pass about to begin: it is taken, so it brings no other.
            Next::Again => {
                let _ = wake.notified().now_or_never();
            }
            Next::Stop => return false,
        }
        true
    }

    /// One pass: sync, then backfill until it is done or the next pass is
    /// due (woken, or the interval gone by since backfill began).
    async fn sync_pass(
        &self,
        account: &EmailAddress,
        wake: &Notify,
        watching: &Watching,
        tally: &mut Tally,
        share: &mut Share,
    ) -> Next {
        self.emit(SyncEvent::Started {
            account: account.clone(),
        });
        watching.pass_began();
        let progress = self.progress_of(account);
        let began = Instant::now();
        let (result, woken) = match self.sync_account(account, &progress, tally).await {
            Ok(()) => {
                let mut give_way = share.give_way(wake, began.elapsed());
                let due = || give_way.due(watching.interval(&self.options));
                let backfilled = self
                    .sync
                    .backfill_until(account, tally.progress(progress), due)
                    .await;
                (tally.count(backfilled), give_way.woken())
            }
            Err(e) => (Err(e), false),
        };
        share.passed(woken);
        watching.pass_ended(result.is_ok());
        let next = match result {
            Ok(backfilled) if backfilled.gave_way => {
                let why = if woken { "woken" } else { "polled" };
                tracing::debug!("{account}: backfill gives way to a pass ({why})");
                Next::Again
            }
            Ok(backfilled) => {
                self.emit(SyncEvent::Done {
                    account: account.clone(),
                    downloaded: backfilled.report.fetched,
                });
                Next::Pause(watching.interval(&self.options))
            }
            Err(e) => self.failed(account, e, |error, retry_in| SyncEvent::Failed {
                account: account.clone(),
                error,
                retry_in,
            }),
        };
        match next {
            // The wake that would have ended the pause is used up.
            Next::Pause(_) if woken => Next::Again,
            next => next,
        }
    }

    /// The account's progress, as its events.
    fn progress_of(&self, account: &EmailAddress) -> ProgressFn {
        let (events, account) = (self.events.clone(), account.clone());
        Arc::new(move |progress| {
            events(SyncEvent::Progress {
                account: account.clone(),
                progress,
            })
        })
    }

    async fn sync_account(
        &self,
        account: &EmailAddress,
        progress: &ProgressFn,
        tally: &mut Tally,
    ) -> Result<(), SyncError> {
        let report = self.sync.sync(account, progress.clone()).await?;
        // The pass signed in and read the folders: a busy wait is over.
        self.busy.reset(account, IMAP);
        tally.synced(&report);
        self.emit(SyncEvent::Synced {
            account: account.clone(),
            report,
        });
        Ok(())
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
