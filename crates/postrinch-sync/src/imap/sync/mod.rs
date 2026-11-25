//! Keeping the store in step with an IMAP account (docs/imap.md, "Sync").
//!
//! A pass lists the folders and keeps the account's labels in step with
//! them ([`folders`]), then reads each folder: everything when its
//! UIDVALIDITY is new, otherwise the mail that arrived, the flags that
//! changed and the mail that went ([`pass`]). Messages are stored by their
//! headers, newest first, and [`ImapSync::backfill`] downloads the bodies
//! after; [`ImapSync::download_now`] fetches the ones the person opens.
//!
//! [`ImapSync::send_outgoing`] delivers the mail written here: messages
//! over SMTP, filed in Sent, and drafts kept in Drafts ([`send`]).
//!
//! Connections are few and kept: per account, one for passes and
//! backfill, one for what the person opens and one for outgoing mail,
//! each reused while the server keeps it open (Gmail allows about 15 at
//! once per account), and one that IDLEs on the inbox where the server
//! offers it ([`idle`]).

mod account_settings;
mod accounts;
mod answers;
mod bodies;
mod changes;
mod cursor;
mod fetch;
mod folder_error;
mod folders;
mod headers;
mod idle;
mod idle_wire;
mod marks;
mod names;
mod pass;
mod pending;
mod places;
mod read;
mod reads;
mod renames;
mod send;
mod uids;
mod utf7;
mod wire;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use postrinch_model::{AccountState, EmailAddress, ImapSettings};
use postrinch_store::{Store, StoreResult};
use tokio::sync::Mutex as AsyncMutex;

pub use self::account_settings::{AccountFolder, SignInAgain};
pub use self::accounts::{
    Added, NewImapAccount, NewSignIn, OnGrant, OpenBrowser, SignInCancel, SignInCanceller,
    sign_in_cancel,
};
pub(crate) use self::idle::Idler;
use self::idle::Releases;
pub(crate) use self::idle_wire::Idled;
pub use self::send::SentLookup;
use self::wire::Conn;
use super::{AccessTokens, BusyGate, Connector, ImapSecrets};
use crate::{ProgressFn, SyncReport};
use crate::{SyncError, SyncResult};

/// A connection slot, kept between calls.
type Slot = Arc<AsyncMutex<Option<Conn>>>;

/// An account's connections.
#[derive(Default, Clone)]
struct Lanes {
    /// Passes and backfill, one after another.
    sync: Slot,
    /// Messages the person opens, so they never wait behind a pass.
    reader: Slot,
    /// Filing sent mail and keeping drafts ([`send`]), so they never
    /// wait behind a pass either.
    outgoing: Slot,
    /// Held while the account's outgoing mail is delivered, so two
    /// deliveries never take the same message ([`send`]).
    sending: Arc<AsyncMutex<()>>,
}

/// A connection taken from its slot while a call uses it.
struct Held {
    slot: tokio::sync::OwnedMutexGuard<Option<Conn>>,
    conn: Conn,
}

impl Held {
    /// Puts the connection back for the next call.
    fn keep(self) {
        let Held { mut slot, conn } = self;
        *slot = Some(conn);
    }
}

/// Which of an account's connections a call uses.
#[derive(Debug, Clone, Copy)]
enum Lane {
    Sync,
    Reader,
    Outgoing,
}

/// IMAP sync into the store, for every IMAP account. Cheap to clone.
#[derive(Clone)]
pub struct ImapSync {
    store: Store,
    connector: Connector,
    secrets: ImapSecrets,
    tokens: Arc<dyn AccessTokens>,
    lanes: Arc<Mutex<HashMap<String, Lanes>>>,
    sent_lookup: SentLookup,
    /// The accounts' busy servers' waits, for the keep-in-sync loops.
    busy: BusyGate,
    /// Tells each account's IDLE loop when its connections are let go.
    releases: Releases,
}

impl ImapSync {
    /// Sync over `connector`, signing in with the accounts' secrets and,
    /// for OAuth accounts, access tokens from `tokens`.
    pub fn new(
        store: Store,
        connector: Connector,
        secrets: ImapSecrets,
        tokens: Arc<dyn AccessTokens>,
    ) -> Self {
        Self {
            store,
            connector,
            secrets,
            tokens,
            lanes: Arc::default(),
            sent_lookup: SentLookup::default(),
            busy: BusyGate::default(),
            releases: Releases::default(),
        }
    }

    /// Waits out busy servers from `first`, doubling up to `cap`, in place
    /// of [`crate::Backoff::new`]'s (for tests). Clones made before this
    /// keep the waits they had.
    pub fn with_busy_backoff(mut self, first: Duration, cap: Duration) -> Self {
        self.busy = BusyGate::with_policy(first, cap);
        self
    }

    /// The accounts' busy servers' waits.
    pub(crate) fn busy(&self) -> &BusyGate {
        &self.busy
    }

    /// Looks for sent mail in Sent as `lookup` says, in place of the
    /// default waits (for tests).
    pub fn with_sent_lookup(mut self, lookup: SentLookup) -> Self {
        self.sent_lookup = lookup;
        self
    }

    /// Brings the store up to date with the account: its folders as
    /// labels, and each folder's mail, by headers. When the server refuses
    /// the account's credential, the account is marked as needing to sign
    /// in and [`SyncError::NeedsSignIn`] is returned. Changes sent from the
    /// outbox before it began are forgotten once it succeeds.
    pub async fn sync(
        &self,
        account: &EmailAddress,
        progress: ProgressFn,
    ) -> SyncResult<SyncReport> {
        let started = places::now();
        let result = self
            .over(account, Lane::Sync, async |conn| {
                self.pass(conn, account, &progress).await
            })
            .await;
        if result.is_ok() {
            let account = account.clone();
            self.with_store(move |s| s.forget_sent(&account, started))
                .await?;
        }
        self.note_sign_in(account, &result).await?;
        result
    }

    /// Forgets the account's kept connections, signing out of them, as
    /// when the account is removed or its settings change. Its IDLE
    /// connection goes too ([`Self::released`]).
    pub async fn disconnect(&self, account: &EmailAddress) {
        self.releases.release(account);
        let lanes = self
            .lanes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&account.key());
        let Some(lanes) = lanes else { return };
        for slot in [lanes.sync, lanes.reader, lanes.outgoing] {
            if let Some(conn) = slot.lock().await.take()
                && let Err(e) = conn.into_session().logout().await
            {
                tracing::debug!("IMAP: signing out of {account} failed: {e}");
            }
        }
    }

    /// Marks the account as needing to sign in when `result` says so.
    async fn note_sign_in<T>(
        &self,
        account: &EmailAddress,
        result: &SyncResult<T>,
    ) -> SyncResult<()> {
        if let Err(SyncError::NeedsSignIn(reason)) = result {
            let state = AccountState::NeedsSignIn(reason.clone());
            let account = account.clone();
            self.with_store(move |s| s.set_account_state(&account, &state))
                .await?;
        }
        Ok(())
    }

    /// Holds one of the account's connections: the kept one while it
    /// still answers, otherwise a new one. It is kept for the next call
    /// only when [`Held::keep`] is called, after the work succeeded.
    async fn hold(&self, account: &EmailAddress, lane: Lane) -> SyncResult<Held> {
        let mut slot = self.slot(account, lane).lock_owned().await;
        let kept = match slot.take() {
            Some(mut conn) => conn.alive().await.then_some(conn),
            None => None,
        };
        let conn = match kept {
            Some(conn) => conn,
            None => self.open(account).await?,
        };
        Ok(Held { slot, conn })
    }

    /// Runs `work` over one of the account's connections ([`Self::hold`]),
    /// keeping the connection only when the work succeeded.
    async fn over<T>(
        &self,
        account: &EmailAddress,
        lane: Lane,
        work: impl AsyncFnOnce(&mut Conn) -> SyncResult<T>,
    ) -> SyncResult<T> {
        let mut held = self.hold(account, lane).await?;
        let result = work(&mut held.conn).await;
        if result.is_ok() {
            held.keep();
        }
        result
    }

    /// The lock an account's deliveries take turns on.
    fn sending(&self, account: &EmailAddress) -> Arc<AsyncMutex<()>> {
        let mut lanes = self.lanes.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(&lanes.entry(account.key()).or_default().sending)
    }

    fn slot(&self, account: &EmailAddress, lane: Lane) -> Slot {
        let mut lanes = self.lanes.lock().unwrap_or_else(|e| e.into_inner());
        let lanes = lanes.entry(account.key()).or_default();
        match lane {
            Lane::Sync => Arc::clone(&lanes.sync),
            Lane::Reader => Arc::clone(&lanes.reader),
            Lane::Outgoing => Arc::clone(&lanes.outgoing),
        }
    }

    /// Connects and signs in to the account's server.
    async fn open(&self, account: &EmailAddress) -> SyncResult<Conn> {
        let mut conn = Conn::new(self.open_session(account).await?);
        conn.enable_qresync().await?;
        Ok(conn)
    }

    async fn settings(&self, account: &EmailAddress) -> SyncResult<ImapSettings> {
        let a = account.clone();
        self.with_store(move |s| s.account(&a))
            .await?
            .and_then(|stored| stored.imap)
            .ok_or_else(|| SyncError::Config(format!("{account} is not an IMAP account")))
    }

    /// Runs store calls on a blocking thread: every store call may wait
    /// on the disk.
    async fn with_store<T: Send + 'static>(
        &self,
        call: impl FnOnce(&Store) -> StoreResult<T> + Send + 'static,
    ) -> SyncResult<T> {
        self.blocking(move |s| Ok(call(s)?)).await
    }

    /// Runs a call that uses the store and may fail with a sync error on a
    /// blocking thread.
    async fn blocking<T: Send + 'static>(
        &self,
        call: impl FnOnce(&Store) -> SyncResult<T> + Send + 'static,
    ) -> SyncResult<T> {
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || call(&store)).await {
            Ok(result) => result,
            Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
            Err(e) => Err(SyncError::Io(std::io::Error::other(e))),
        }
    }
}
