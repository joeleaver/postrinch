//! Keeping every account in sync, and sending what is made here, for as
//! long as the app runs: Gmail's loops for Google accounts
//! ([`GmailSync::keep_in_sync`](crate::google::GmailSync::keep_in_sync),
//! [`GmailSync::keep_sending`](crate::google::GmailSync::keep_sending)),
//! and a loop per IMAP account (`imap_keep`), side by side.
//!
//! A wake is meant for every account, so each engine gets its own and the
//! caller's is passed on to both: a `Notify` woken once wakes one waiter.

use std::future::pending;
use std::sync::Arc;

use tokio::sync::Notify;

use super::MailSync;
use super::imap_keep::{Duty, ImapKeeper};
use crate::{KeepOptions, SyncEventFn};

impl MailSync {
    /// Sync every signed-in account, again and again, until the future is
    /// dropped: Google accounts one after another, as Gmail's quota is the
    /// account's; IMAP accounts each on their own, polled every
    /// [`KeepOptions::interval`], a busy server waited out with a growing
    /// wait ([`crate::SyncEvent::Busy`]). Notifying `wake` starts every
    /// account's next pass at once (a busy server's wait excepted): after
    /// an account signs in, or when the person asks.
    pub async fn keep_in_sync(&self, options: KeepOptions, events: SyncEventFn, wake: Arc<Notify>) {
        let (gmail_wake, imap_wake) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        let relayed = relay(wake, [gmail_wake.clone(), imap_wake.clone()]);
        let gmail = async {
            match &self.gmail {
                Some(gmail) => {
                    gmail
                        .keep_in_sync(options, events.clone(), gmail_wake)
                        .await
                }
                None => pending().await,
            }
        };
        let keeper = self.keeper(options, events.clone());
        tokio::join!(relayed, gmail, keeper.keep(Duty::Sync, imap_wake));
    }

    /// Send every account's changes and mail, again and again, until the
    /// future is dropped. It runs beside [`Self::keep_in_sync`], so a long
    /// first sync never holds a change back. Notifying `wake` sends at
    /// once: after a change is made. Each IMAP account sends on its own,
    /// so one whose server is slow to show sent mail in Sent never holds
    /// up another's.
    pub async fn keep_sending(&self, options: KeepOptions, events: SyncEventFn, wake: Arc<Notify>) {
        let (gmail_wake, imap_wake) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        let relayed = relay(wake, [gmail_wake.clone(), imap_wake.clone()]);
        let gmail = async {
            match &self.gmail {
                Some(gmail) => {
                    gmail
                        .keep_sending(options, events.clone(), gmail_wake)
                        .await
                }
                None => pending().await,
            }
        };
        let keeper = self.keeper(options, events.clone());
        tokio::join!(relayed, gmail, keeper.keep(Duty::Send, imap_wake));
    }

    fn keeper(&self, options: KeepOptions, events: SyncEventFn) -> ImapKeeper {
        ImapKeeper {
            sync: self.imap.clone(),
            store: self.store.clone(),
            busy: self.busy.clone(),
            options,
            events,
        }
    }
}

/// Passes each wake of `from` on to every one of `to`, for ever.
async fn relay<const N: usize>(from: Arc<Notify>, to: [Arc<Notify>; N]) {
    loop {
        from.notified().await;
        for wake in &to {
            wake.notify_one();
        }
    }
}
