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
//!   connection is dropped, and the next pass, with the new sign-in or
//!   servers, is waited for.
//! - **A busy server** is the account's: the IDLE waits out the busy
//!   gate the other loops share, and a busy answer of its own takes the
//!   gate's next wait and says so ([`SyncEvent::Busy`]), never asking the
//!   person to sign in. When the server says the account has too many
//!   connections, the IDLE connection is the one that gives way: it
//!   stays away for [`IdleOptions::give_way`] and the account is polled,
//!   its other connections untouched.
//! - **A refused sign-in** is left to the pass, which records it and
//!   asks the person, as for any refused sign-in: the IDLE wakes a pass
//!   and waits for the next one that works.
//! - **Anything else** (the connection lost, a `BYE`, a timeout) wakes a
//!   pass, as news may have been missed, and connects again after a
//!   backoff ([`IdleOptions::reconnect`]) that one IDLE ending well
//!   resets. A server without IDLE is left alone until its connections
//!   are let go.
//!
//! While an IDLE is begun, passes come every [`IdleOptions::interval`]
//! in place of [`KeepOptions::interval`]; once it is lost, the pass it
//! wakes goes back to the shorter pause.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use postrinch_model::EmailAddress;
use tokio::sync::{Notify, watch};

use super::imap_keep::ImapKeeper;
use crate::imap::{Idled, Idler};
use crate::{Backoff, IdleOptions, KeepOptions, SyncError, SyncEvent};

const IMAP: &str = "IMAP";

/// What an account's sync loop and IDLE loop share.
pub(super) struct Watching {
    /// How many passes have worked, for the IDLE to connect after.
    passes: watch::Sender<u64>,
    /// Whether an IDLE is begun on the inbox.
    idling: AtomicBool,
}

impl Watching {
    pub(super) fn new() -> Self {
        Self {
            passes: watch::channel(0).0,
            idling: AtomicBool::new(false),
        }
    }

    /// A pass worked.
    pub(super) fn passed(&self) {
        self.passes.send_modify(|n| *n += 1);
    }

    /// The pause after a pass that worked.
    pub(super) fn interval(&self, options: &KeepOptions) -> Duration {
        if self.idling.load(Ordering::Relaxed) {
            options.idle.interval
        } else {
            options.interval
        }
    }

    fn set_idling(&self, idling: bool) {
        self.idling.store(idling, Ordering::Relaxed);
    }
}

/// How one connection's IDLE ended.
enum Ended {
    /// The account's connections were let go.
    Released,
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
            reconnect: Backoff::with(options.reconnect, options.reconnect_cap),
            options,
        };
        let mut passes = watching.passes.subscribe();
        let mut after = 0;
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
                }
                continue;
            }
            let ended = idle.connection().await;
            let next_pass = match ended {
                Ended::Released => true,
                Ended::NoIdle => {
                    idle.until_released().await;
                    true
                }
                Ended::Failed(error) => idle.failed(error).await,
            };
            if next_pass {
                after = *passes.borrow();
            }
        }
    }
}

impl Idle<'_> {
    /// Connects, and IDLEs until the connection ends.
    async fn connection(&mut self) -> Ended {
        let opened = tokio::select! {
            opened = self.keeper.sync.idle_connection(self.account) => opened,
            () = let_go(&mut self.released) => return Ended::Released,
        };
        let mut idler = match opened {
            Ok(Some(idler)) => idler,
            Ok(None) => return Ended::NoIdle,
            Err(e) => return Ended::Failed(e),
        };
        let ended = self.idle_on(&mut idler).await;
        let was_idling = self.watching.idling.swap(false, Ordering::Relaxed);
        match ended {
            Ended::Released => idler.logout().await,
            // News may have come while the IDLE was lost, and passes are
            // to come on the shorter pause again. One that never began
            // watched nothing.
            _ if was_idling => self.wake.notify_one(),
            _ => {}
        }
        ended
    }

    /// IDLEs on `idler` again and again, waking a pass on news.
    async fn idle_on(&mut self, idler: &mut Idler) -> Ended {
        let IdleOptions {
            restart, settle, ..
        } = self.options;
        let watching = self.watching;
        loop {
            let idled = tokio::select! {
                idled = idler.idle(restart, settle, || watching.set_idling(true)) => idled,
                () = let_go(&mut self.released) => return Ended::Released,
            };
            match idled {
                Ok(Idled::News) => self.wake.notify_one(),
                Ok(Idled::Restart) => {}
                Err(e) => return Ended::Failed(e),
            }
            // One IDLE ended as it should: the connection works.
            self.reconnect.reset();
        }
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
                tracing::info!(
                    "{account}: too many connections for IDLE ({message}); polled for {:?}",
                    self.options.give_way
                );
                self.pause(self.options.give_way).await
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
            () = let_go(&mut self.released) => true,
        }
    }

    async fn until_released(&mut self) {
        let_go(&mut self.released).await;
    }
}

/// Until the account's connections are next let go. The count's sender is
/// the engine's, which outlives every keeper; were it gone, nothing would
/// ever be let go again.
async fn let_go(released: &mut watch::Receiver<u64>) {
    if released.changed().await.is_err() {
        std::future::pending::<()>().await;
    }
}
