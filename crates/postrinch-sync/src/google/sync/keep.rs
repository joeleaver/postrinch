//! Keeping every Google account in sync for as long as the app runs.
//!
//! One pass syncs each signed-in account, then downloads the bodies its
//! sync left as headers. Passes repeat every [`KeepOptions::interval`], or
//! after [`KeepOptions::retry`] when an account failed: a network that drops
//! for minutes at a time costs a wait, not the sync.
//!
//! Backfill gives way once the next pass is due, woken or with the
//! interval gone by since this one began (`giving_way`): it starts no
//! more downloads, and the accounts after it in the pass do one round of
//! downloads at most. The next pass then begins at once, with no
//! [`SyncEvent::Done`] for an account whose backfill gave way, and each
//! backfill carries on after its account's sync, its count with it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use postrinch_model::{EmailAddress, Provider};
use tokio::sync::Notify;

use super::GmailSync;
use crate::giving_way::{Backfilled, GiveWay, Tally};
use crate::{KeepOptions, ProgressFn, SyncError, SyncEvent, SyncEventFn};

/// How a pass over the accounts ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Passed {
    /// Every account synced, and its bodies are downloaded.
    Done,
    /// An account failed: the next pass comes after the retry.
    Failed,
    /// An account's backfill gave way: the next pass is due now.
    GaveWay,
}

impl GmailSync {
    /// Sync every signed-in Google account, again and again, until the
    /// future is dropped. Notifying `wake` starts the next pass at once:
    /// after an account signs in, or when the person asks.
    pub async fn keep_in_sync(&self, options: KeepOptions, events: SyncEventFn, wake: Arc<Notify>) {
        // Each account's backfill count, by its address's key.
        let mut tallies: HashMap<String, Tally> = HashMap::new();
        loop {
            let pause = match self.pass(&options, &events, &wake, &mut tallies).await {
                Passed::Done => options.interval,
                Passed::Failed => options.retry,
                Passed::GaveWay => {
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

    /// One pass over the accounts, each synced and then backfilled until
    /// the next pass is due.
    async fn pass(
        &self,
        options: &KeepOptions,
        events: &SyncEventFn,
        wake: &Notify,
        tallies: &mut HashMap<String, Tally>,
    ) -> Passed {
        let accounts = match self.with_store(|s| s.accounts()).await {
            Ok(accounts) => accounts,
            Err(e) => {
                tracing::error!("cannot read the accounts to sync: {e}");
                return Passed::Failed;
            }
        };
        let kept: Vec<EmailAddress> = accounts
            .into_iter()
            .filter(|a| a.provider == Provider::Google && a.is_active())
            .map(|a| a.address)
            .collect();
        // An account gone, or signed out, starts its count again.
        tallies.retain(|key, _| kept.iter().any(|a| a.key() == *key));
        let mut give_way = GiveWay::new(wake);
        let (mut all_ok, mut gave_way) = (true, false);
        for address in kept {
            events(SyncEvent::Started {
                account: address.clone(),
            });
            let tally = tallies.entry(address.key()).or_default();
            let due = || give_way.due(options.interval);
            match self.sync_and_download(&address, events, tally, due).await {
                Ok(backfilled) if backfilled.gave_way => gave_way = true,
                Ok(backfilled) => events(SyncEvent::Done {
                    account: address,
                    downloaded: backfilled.report.fetched,
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
        if gave_way {
            let why = if give_way.woken() { "woken" } else { "polled" };
            tracing::debug!("Gmail: backfill gives way to a pass ({why})");
            Passed::GaveWay
        } else if all_ok {
            Passed::Done
        } else {
            Passed::Failed
        }
    }

    async fn sync_and_download(
        &self,
        account: &EmailAddress,
        events: &SyncEventFn,
        tally: &mut Tally,
        give_way: impl FnMut() -> bool + Send,
    ) -> Result<Backfilled, SyncError> {
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
        let backfilled = self
            .backfill_until(account, tally.progress(progress), give_way)
            .await?;
        tally.ended(&backfilled);
        Ok(backfilled)
    }
}
