//! Keeping every Google account in sync for as long as the app runs.
//!
//! One pass syncs each signed-in account, then downloads the bodies its
//! sync left as headers. Passes repeat every [`KeepOptions::interval`], or
//! after [`KeepOptions::retry`] when an account failed: a network that drops
//! for minutes at a time costs a wait, not the sync.
//!
//! A pass syncs every account first, then backfills them, so the pass a
//! wake asks for reaches each account's new mail without waiting behind
//! another account's backfill. Backfill gives way once the next pass is
//! due, woken or with the interval gone by since backfill began
//! (`giving_way`): it starts no more downloads, and the accounts after it
//! do one round of downloads at most. The next pass then begins at once,
//! with no [`SyncEvent::Done`] for an account whose backfill gave way,
//! and each backfill carries on after the syncs, its count with it. A
//! pass whose backfill took a wake begins the next at once whatever
//! failed after, as the wake is used up.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use postrinch_model::{EmailAddress, Provider};
use tokio::sync::Notify;
use tokio::time::Instant;

use super::GmailSync;
use crate::giving_way::{Share, Tally};
use crate::{KeepOptions, ProgressFn, SyncError, SyncEvent, SyncEventFn};

/// How a pass over the accounts ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Passed {
    /// Every account synced, and its bodies are downloaded.
    Done,
    /// An account failed: the next pass comes after the retry.
    Failed,
    /// The next pass is due now: a backfill gave way to it, or took the
    /// wake asking for it.
    Again,
}

/// What the loop keeps from pass to pass.
#[derive(Default)]
struct Kept {
    /// Each account's backfill count, by its address's key.
    tallies: HashMap<String, Tally>,
    /// What the next pass owes the backfills.
    share: Share,
}

impl GmailSync {
    /// Sync every signed-in Google account, again and again, until the
    /// future is dropped. Notifying `wake` starts the next pass at once:
    /// after an account signs in, or when the person asks.
    pub async fn keep_in_sync(&self, options: KeepOptions, events: SyncEventFn, wake: Arc<Notify>) {
        let mut kept = Kept::default();
        loop {
            let pause = match self.pass(&options, &events, &wake, &mut kept).await {
                Passed::Done => options.interval,
                Passed::Failed => options.retry,
                Passed::Again => {
                    // A wake that came since a backfill gave way asked for
                    // the pass about to begin: it is taken, so it brings
                    // no other.
                    let _ = wake.notified().now_or_never();
                    continue;
                }
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

    /// One pass over the accounts: each synced, then each backfilled
    /// until the next pass is due.
    async fn pass(
        &self,
        options: &KeepOptions,
        events: &SyncEventFn,
        wake: &Notify,
        kept: &mut Kept,
    ) -> Passed {
        let accounts = match self.with_store(|s| s.accounts()).await {
            Ok(accounts) => accounts,
            Err(e) => {
                tracing::error!("cannot read the accounts to sync: {e}");
                return Passed::Failed;
            }
        };
        let accounts: Vec<EmailAddress> = accounts
            .into_iter()
            .filter(|a| a.provider == Provider::Google && a.is_active())
            .map(|a| a.address)
            .collect();
        // An account gone, or signed out, starts its count again.
        kept.tallies
            .retain(|key, _| accounts.iter().any(|a| a.key() == *key));
        let began = Instant::now();
        let mut all_ok = true;
        let mut synced = Vec::new();
        for address in accounts {
            events(SyncEvent::Started {
                account: address.clone(),
            });
            let progress = progress_of(events, &address);
            let tally = kept.tallies.entry(address.key()).or_default();
            match self.sync_account(&address, events, &progress, tally).await {
                Ok(()) => synced.push((address, progress)),
                Err(e) => all_ok &= trouble(events, options, address, e),
            }
        }
        let mut give_way = kept.share.give_way(wake, began.elapsed());
        let mut gave_way = false;
        for (address, progress) in synced {
            let tally = kept.tallies.entry(address.key()).or_default();
            let due = || give_way.due(options.interval);
            let backfilled = self
                .backfill_until(&address, tally.progress(progress), due)
                .await;
            match tally.count(backfilled) {
                Ok(backfilled) if backfilled.gave_way => gave_way = true,
                Ok(backfilled) => events(SyncEvent::Done {
                    account: address,
                    downloaded: backfilled.report.fetched,
                }),
                Err(e) => all_ok &= trouble(events, options, address, e),
            }
        }
        let woken = give_way.woken();
        kept.share.passed(woken);
        if gave_way || woken {
            let why = if woken { "woken" } else { "polled" };
            tracing::debug!("Gmail: backfill gives way to a pass ({why})");
            Passed::Again
        } else if all_ok {
            Passed::Done
        } else {
            Passed::Failed
        }
    }

    /// Syncs the account's mail, then its rules and calendars.
    async fn sync_account(
        &self,
        account: &EmailAddress,
        events: &SyncEventFn,
        progress: &ProgressFn,
        tally: &mut Tally,
    ) -> Result<(), SyncError> {
        let report = self.sync(account, progress.clone()).await?;
        tally.synced(&report);
        if self.rules_due(account).await {
            // Rules trouble is the rules' to show; mail keeps syncing.
            match self.sync_rules(account).await {
                Ok(_) => self.rules_checked(account),
                Err(SyncError::NeedsSignIn(reason)) => return Err(SyncError::NeedsSignIn(reason)),
                Err(e) => tracing::warn!("{account}: keeping rules in Gmail: {e}"),
            }
        }
        // Calendar trouble is logged; mail keeps syncing.
        match self.sync_calendars(account).await {
            Ok(_) => {}
            Err(SyncError::NeedsSignIn(reason)) => return Err(SyncError::NeedsSignIn(reason)),
            Err(e) => tracing::warn!("{account}: keeping calendars: {e}"),
        }
        events(SyncEvent::Synced {
            account: account.clone(),
            report,
        });
        Ok(())
    }
}

/// The account's progress, as its events.
fn progress_of(events: &SyncEventFn, account: &EmailAddress) -> ProgressFn {
    let (events, account) = (events.clone(), account.clone());
    Arc::new(move |progress| {
        events(SyncEvent::Progress {
            account: account.clone(),
            progress,
        })
    })
}

/// Says what went wrong with the account's sync or backfill. Returns
/// whether the pass may still count as done: an account that must sign
/// in again is left alone, not retried.
fn trouble(
    events: &SyncEventFn,
    options: &KeepOptions,
    account: EmailAddress,
    error: SyncError,
) -> bool {
    match error {
        SyncError::NeedsSignIn(_) => {
            events(SyncEvent::NeedsSignIn { account });
            true
        }
        e => {
            events(SyncEvent::Failed {
                account,
                error: e.to_string(),
                retry_in: options.retry,
            });
            false
        }
    }
}
