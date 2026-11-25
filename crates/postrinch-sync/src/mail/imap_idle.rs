//! An IMAP account's IDLE loop (docs/imap.md, "Sync"): beside its sync
//! loop, in the same task, so it starts and ends with it
//! (`imap_account.rs`). It keeps one more connection IDLEing on the inbox
//! ([`ImapSync::idle_connection`](crate::imap::ImapSync)) and, when the
//! server tells of news, wakes the sync loop for a pass; it never syncs
//! itself.
//!
//! - **After a pass.** It first connects once the sync loop has had a
//!   pass that worked, so the inbox's folder is known and the account's
//!   sign-in has just been shown to work, and again after each time it
//!   lets go, waiting for the next such pass.
//! - **Let go** when the account's connections are
//!   ([`ImapSync::disconnect`](crate::imap::ImapSync::disconnect): a
//!   sign-in again, new servers, the account added again or removed): the
//!   IDLE is ended with `DONE` and the connection signs out, and the next
//!   pass, with the new sign-in or servers, is waited for.
//! - **A busy server** is the account's: the IDLE waits out the busy
//!   gate the other loops share, and a busy answer of its own takes the
//!   gate's next wait and says so ([`SyncEvent::Busy`]), never asking the
//!   person to sign in. When the server says the account has too many
//!   connections, the IDLE connection is the one that gives way: refused
//!   itself, or asked to by another of the account's connections that was
//!   refused (`ImapSync::give_way_asked`), it stays away for
//!   [`IdleOptions::give_way`] and the account is polled.
//! - **A refused sign-in** is left to the pass, which records it and
//!   asks the person, as for any refused sign-in: the IDLE wakes a pass
//!   and waits for the next one that works.
//! - **Anything else** (the connection lost, a `BYE`, a timeout, a server
//!   that ends each IDLE as soon as it begins) wakes a pass, as news may
//!   have been missed, and connects again after a backoff
//!   ([`IdleOptions::reconnect`]) that one IDLE ending well resets. A
//!   server without IDLE is left alone until its connections are let go.
//!
//! While an IDLE is begun, passes come every [`IdleOptions::interval`]
//! in place of [`KeepOptions::interval`]; once it is lost, the pass it
//! wakes goes back to the shorter pause. A connection's first IDLE wakes
//! a pass when the connection was made again without waiting for a pass,
//! or while a pass is under way: the last pass, or that one, may have
//! read the inbox before this connection EXAMINEd it, and mail delivered
//! in between, which the EXAMINE counts but no IDLE tells of, would
//! otherwise wait for a poll ([`Watching::began_idling`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use postrinch_model::EmailAddress;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

use super::imap_keep::ImapKeeper;
use crate::imap::{IdleEnd, Idler};
use crate::{Backoff, IdleOptions, KeepOptions, SyncError, SyncEvent};

const IMAP: &str = "IMAP";

/// An IDLE the server ends itself sooner than this, news or not, is not
/// working: a server that answers IDLE with its tagged `OK` at once
/// would otherwise have it sent again and again without a pause. It is
/// dropped as a failed connection is, and made again after a backoff.
const SHORTEST_IDLE: Duration = Duration::from_secs(1);

/// What an account's sync loop and IDLE loop share.
pub(super) struct Watching {
    /// How many passes have worked, for the IDLE to connect after.
    passes: watch::Sender<u64>,
    /// Whether an IDLE is begun on the inbox.
    idling: AtomicBool,
    /// Whether a pass is under way.
    passing: AtomicBool,
}

impl Watching {
    pub(super) fn new() -> Self {
        Self {
            passes: watch::channel(0).0,
            idling: AtomicBool::new(false),
            passing: AtomicBool::new(false),
        }
    }

    /// A pass begins.
    pub(super) fn pass_began(&self) {
        self.passing.store(true, Ordering::Relaxed);
    }

    /// The pass ended; `worked` when it did.
    pub(super) fn pass_ended(&self, worked: bool) {
        self.passing.store(false, Ordering::Relaxed);
        if worked {
            self.passes.send_modify(|n| *n += 1);
        }
    }

    /// The pause after a pass that worked.
    pub(super) fn interval(&self, options: &KeepOptions) -> Duration {
        if self.idling.load(Ordering::Relaxed) {
            options.idle.interval
        } else {
            options.interval
        }
    }

    /// An IDLE has begun. Returns whether it is to wake a pass: on its
    /// connection's first IDLE, when the connection was made again
    /// without waiting for a pass (`reconnected`: after a lost
    /// connection, say), or while a pass is under way. The last pass, or
    /// the one under way, may have read the inbox before the connection
    /// EXAMINEd it; mail delivered in between is in the EXAMINE's count
    /// but never told of by the IDLE, and would wait for the next poll,
    /// the longer pause after a pass that ends while the IDLE is begun.
    /// A connection made after a pass that worked, with none under way,
    /// wakes none: that pass chose the shorter pause, before the IDLE
    /// began.
    fn began_idling(&self, reconnected: bool) -> bool {
        let first = !self.idling.swap(true, Ordering::Relaxed);
        first && (reconnected || self.passing.load(Ordering::Relaxed))
    }
}

/// How one connection's IDLE ended.
enum Ended {
    /// The account's connections were let go.
    Released,
    /// Another of the account's connections was refused for the account's
    /// connection limit: this one signed out for it.
    GaveWay,
    /// The server offers no IDLE.
    NoIdle,
    Failed(SyncError),
}

/// The IDLE loop's own state.
struct Idle<'a> {
    keeper: &'a ImapKeeper,
    account: &'a EmailAddress,
    wake: &'a Notify,
    watching: &'a Watching,
    released: watch::Receiver<u64>,
    give_way: watch::Receiver<u64>,
    reconnect: Backoff,
    options: IdleOptions,
}

impl ImapKeeper {
    /// Keeps the account's inbox watched, for as long as the future is
    /// kept: dropping it (the sync loop ended) drops the connection.
    pub(super) async fn keep_idling(
        &self,
        account: &EmailAddress,
        wake: &Notify,
        watching: &Watching,
    ) {
        let options = self.options.idle;
        let mut idle = Idle {
            keeper: self,
            account,
            wake,
            watching,
            released: self.sync.released(account),
            give_way: self.sync.give_way_asked(account),
            reconnect: Backoff::with(options.reconnect, options.reconnect_cap),
            options,
        };
        let mut passes = watching.passes.subscribe();
        let mut after = 0;
        // Whether the IDLE connects again without waiting for a pass, as
        // after a lost connection.
        let mut reconnecting = false;
        loop {
            // A pass newer than the last let-go, or the start. The sender
            // lives in `watching`, beside this loop, for as long as it;
            // were it gone, no pass would come, and returning would end
            // the account's sync loop with this one.
            if passes.wait_for(|n| *n > after).await.is_err() {
                std::future::pending::<()>().await;
            }
            idle.released.mark_unchanged();
            if let Some(wait) = self.busy.remaining(account, IMAP) {
                if idle.pause(wait).await {
                    after = *passes.borrow();
                    reconnecting = false;
                }
                continue;
            }
            let next_pass = match idle.connection(reconnecting).await {
                Ended::Released => true,
                Ended::GaveWay => idle.give_way("another connection needed its place").await,
                Ended::NoIdle => {
                    idle.until_released().await;
                    true
                }
                Ended::Failed(error) => idle.failed(error).await,
            };
            if next_pass {
                after = *passes.borrow();
            }
            reconnecting = !next_pass;
        }
    }
}

impl Idle<'_> {
    /// Connects, and IDLEs until the connection ends.
    async fn connection(&mut self, reconnected: bool) -> Ended {
        // Only an ask made while this connection is held is for it. It is
        // held from before it signs in: an ask made while it is being
        // made is heard before its first IDLE (`Self::asked_to_stop`).
        self.give_way.mark_unchanged();
        let opened = tokio::select! {
            opened = self.keeper.sync.idle_connection(self.account) => opened,
            () = next_change(&mut self.released) => return Ended::Released,
        };
        let mut idler = match opened {
            Ok(Some(idler)) => idler,
            Ok(None) => return Ended::NoIdle,
            Err(e) => return Ended::Failed(e),
        };
        let ended = self.idle_on(&mut idler, reconnected).await;
        let was_idling = self.watching.idling.swap(false, Ordering::Relaxed);
        match ended {
            // Ended with DONE: nothing is under way, so it signs out.
            Ended::Released | Ended::GaveWay => idler.logout().await,
            _ => drop(idler),
        }
        // News may have come while the IDLE was lost, and passes are to
        // come on the shorter pause again. One that never began watched
        // nothing; let go, the next pass is waited for anyway.
        if was_idling && !matches!(ended, Ended::Released) {
            self.wake.notify_one();
        }
        ended
    }

    /// IDLEs on `idler` again and again, waking a pass on news, until
    /// the connection fails or is to be let go or give way.
    async fn idle_on(&mut self, idler: &mut Idler, reconnected: bool) -> Ended {
        let IdleOptions {
            restart, settle, ..
        } = self.options;
        let (watching, wake) = (self.watching, self.wake);
        loop {
            if let Some(stop) = self.asked_to_stop() {
                return stop;
            }
            let began = Instant::now();
            let ready = || {
                if watching.began_idling(reconnected) {
                    wake.notify_one();
                }
            };
            let stop = stop_asked(&mut self.released, &mut self.give_way);
            let idled = match idler.idle(restart, settle, ready, stop).await {
                Ok(idled) => idled,
                Err(e) => return Ended::Failed(e),
            };
            if idled.news {
                wake.notify_one();
            }
            match idled.end {
                IdleEnd::Stopped(stop) => return stop,
                IdleEnd::Server if began.elapsed() < SHORTEST_IDLE => {
                    return Ended::Failed(idler.ended_at_once());
                }
                // One IDLE ended as it should: the connection works.
                IdleEnd::Done | IdleEnd::Server => self.reconnect.reset(),
            }
        }
    }

    /// Whether the connection was asked to let go or give way since it
    /// was made, or since its last IDLE: no IDLE is begun then.
    fn asked_to_stop(&self) -> Option<Ended> {
        if self.released.has_changed().unwrap_or(false) {
            Some(Ended::Released)
        } else if self.give_way.has_changed().unwrap_or(false) {
            Some(Ended::GaveWay)
        } else {
            None
        }
    }

    /// Stays away for [`IdleOptions::give_way`], the account polled
    /// meanwhile. Returns whether its connections were let go meanwhile.
    async fn give_way(&mut self, why: &str) -> bool {
        tracing::info!(
            "{}: IDLE gave way ({why}); polled for {:?}",
            self.account,
            self.options.give_way
        );
        self.pause(self.options.give_way).await
    }

    /// Waits after `error` as it calls for. Returns whether to wait for
    /// the next pass before connecting again: after a refused sign-in, or
    /// when the account's connections were let go meanwhile.
    async fn failed(&mut self, error: SyncError) -> bool {
        let account = self.account;
        match error {
            SyncError::NeedsSignIn(_) => {
                tracing::info!("{account}: IDLE's sign-in was refused; a pass will see to it");
                self.wake.notify_one();
                let wait = self.reconnect.next_delay();
                self.pause(wait).await;
                true
            }
            SyncError::Busy {
                connections: true,
                message,
                ..
            } => {
                self.give_way(&format!("too many connections: {message}"))
                    .await
            }
            SyncError::Busy {
                service, message, ..
            } => {
                let retry_in = self.keeper.busy.busy(account, service);
                (self.keeper.events)(SyncEvent::Busy {
                    account: account.clone(),
                    message,
                    retry_in,
                });
                self.pause(retry_in).await
            }
            error => {
                let wait = self.reconnect.next_delay();
                tracing::info!("{account}: IDLE stopped ({error}); again in {wait:?}");
                self.pause(wait).await
            }
        }
    }

    /// Waits `wait`, or until the account's connections are let go.
    /// Returns whether they were.
    async fn pause(&mut self, wait: Duration) -> bool {
        tokio::select! {
            () = tokio::time::sleep(wait) => false,
            () = next_change(&mut self.released) => true,
        }
    }

    async fn until_released(&mut self) {
        next_change(&mut self.released).await;
    }
}

/// Until the connection is asked to let go, or to give way.
async fn stop_asked(
    released: &mut watch::Receiver<u64>,
    give_way: &mut watch::Receiver<u64>,
) -> Ended {
    tokio::select! {
        () = next_change(released) => Ended::Released,
        () = next_change(give_way) => Ended::GaveWay,
    }
}

/// Until the count next rises. Its sender is the engine's, which outlives
/// every keeper; were it gone, it would never rise again.
async fn next_change(count: &mut watch::Receiver<u64>) {
    if count.changed().await.is_err() {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_idle_wakes_a_pass_only_when_one_may_have_missed_mail() {
        // Made after a pass that worked, with none under way: that pass's
        // pause is the shorter one, chosen before this IDLE began.
        let fresh = Watching::new();
        assert!(!fresh.began_idling(false));
        // Begun again on the same connection: it has watched all along.
        assert!(!fresh.began_idling(true));

        let again = Watching::new();
        assert!(again.began_idling(true), "made again without a pass");

        let during = Watching::new();
        during.pass_began();
        assert!(during.began_idling(false), "a pass under way");
        during.pass_ended(true);
        assert!(!during.began_idling(false), "begun once already");
    }

    #[test]
    fn passes_while_idling_pause_longer() {
        let options = KeepOptions::default();
        let watching = Watching::new();
        assert_eq!(watching.interval(&options), options.interval);
        watching.began_idling(false);
        assert_eq!(watching.interval(&options), options.idle.interval);
    }
}
