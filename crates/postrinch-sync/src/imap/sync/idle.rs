//! An account's IDLE connection (RFC 2177; docs/imap.md, "Sync"): one
//! more connection, beside the ones passes and reads use, that EXAMINEs
//! the inbox and waits for the server to tell of a change there. It never
//! syncs: news ends the IDLE, and the account's keep-in-sync loop runs a
//! pass on its own connection (`mail::imap_idle`).
//!
//! The connection is the account's like the others: when they are let go
//! ([`ImapSync::disconnect`], on removal, a new sign-in or new servers),
//! the IDLE loop is told ([`ImapSync::released`]) and signs out of its
//! own. When another of the account's connections is refused for the
//! account's connection limit, the IDLE loop is asked to give way
//! ([`ImapSync::give_way_asked`], `idle_link.rs`).

use postrinch_model::{EmailAddress, FolderRole, SystemLabel};
use tokio::sync::watch;

use super::ImapSync;
use super::folder_error::FolderError;
use super::idle_link::Holding;
use super::wire::Conn;
use crate::imap::connect::describe;
use crate::imap::session::ImapSession;
use crate::{SyncError, SyncResult};

/// A signed-in connection with the inbox examined, ready to IDLE
/// (`idle_wire.rs`).
pub(crate) struct Idler {
    pub(super) conn: Conn,
    /// Marks the account as holding an IDLE connection while it lives.
    _holding: Holding,
}

impl std::fmt::Debug for Idler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Idler")
            .field("server", &self.conn.describe())
            .finish_non_exhaustive()
    }
}

impl Idler {
    /// What an IDLE the server ended itself as soon as it was asked means:
    /// the server's IDLE does not work, and the connection is to be
    /// dropped and made again later, backing off.
    pub(crate) fn ended_at_once(&self) -> SyncError {
        SyncError::Server {
            service: "IMAP",
            server: self.conn.describe(),
            message: "the server ends each IDLE as soon as it begins".into(),
        }
    }

    /// Signs out and closes the connection; a failure only means it was
    /// gone already.
    pub(crate) async fn logout(self) {
        if let Err(e) = self.conn.into_session().logout().await {
            tracing::debug!("IMAP: signing out of an IDLE connection failed: {e}");
        }
    }
}

impl ImapSync {
    /// Opens the account's IDLE connection: signs in as passes do (an
    /// OAuth account's access token from the same slot, refreshed when it
    /// is about to expire, and fetched afresh once if refused) and
    /// EXAMINEs the inbox. `Ok(None)` when the server does not offer
    /// IDLE: the account is polled only.
    ///
    /// Errors are those of signing in ([`SyncError::NeedsSignIn`],
    /// [`SyncError::Busy`], the connection) and of the EXAMINE: a server
    /// busy for the inbox, or refusing to open it, is
    /// [`SyncError::Server`], tried again later. Nothing here marks the
    /// account: a refused sign-in is the pass's to find and record.
    pub(crate) async fn idle_connection(
        &self,
        account: &EmailAddress,
    ) -> SyncResult<Option<Idler>> {
        let session = self.open_session(account).await?;
        if !session.capabilities().idle() {
            tracing::info!(
                "IMAP: {} offers no IDLE: {account} is polled",
                describe(session.server())
            );
            if let Err(e) = session.logout().await {
                tracing::debug!("IMAP: signing out of {account} failed: {e}");
            }
            return Ok(None);
        }
        let inbox = self.inbox_name(account).await?;
        let mut conn = Conn::new(session);
        let server = conn.describe();
        let refused = |why: String| SyncError::Server {
            service: "IMAP",
            server,
            message: format!("the inbox cannot be watched: {why}"),
        };
        let refusal = match conn.examine(&inbox).await {
            Ok(Some(_)) => {
                return Ok(Some(Idler {
                    conn,
                    _holding: self.links.hold(account),
                }));
            }
            Ok(None) => refused("the server would not open it".into()),
            Err(FolderError::Busy(text)) => refused(text),
            Err(FolderError::Sync(e)) => e,
        };
        if let Err(e) = conn.into_session().logout().await {
            tracing::debug!("IMAP: signing out of {account} failed: {e}");
        }
        Err(refusal)
    }

    /// A count that rises each time the account's connections are let
    /// go ([`Self::disconnect`]): its IDLE connection is to go too.
    pub(crate) fn released(&self, account: &EmailAddress) -> watch::Receiver<u64> {
        self.links.released(account)
    }

    /// A count that rises each time another of the account's connections
    /// is refused for its connection limit while the IDLE connection is
    /// held: the IDLE connection is to sign out, and stay away a while.
    pub(crate) fn give_way_asked(&self, account: &EmailAddress) -> watch::Receiver<u64> {
        self.links.give_way_asked(account)
    }

    /// The inbox's name on the server: the folder kept with the inbox's
    /// role, else `INBOX`, which every server has (RFC 9051 5.1).
    async fn inbox_name(&self, account: &EmailAddress) -> SyncResult<String> {
        let a = account.clone();
        let folders = self.with_store(move |s| s.imap_folders(&a)).await?;
        let inbox = FolderRole::System(SystemLabel::Inbox);
        Ok(folders
            .into_iter()
            .find(|f| f.role.as_ref() == Some(&inbox))
            .map_or_else(|| "INBOX".to_owned(), |f| f.name))
    }

    /// Connects and signs in to the account's server, as every one of its
    /// connections does.
    pub(super) async fn open_session(&self, account: &EmailAddress) -> SyncResult<ImapSession> {
        let settings = self.settings(account).await?;
        let secret = self.secrets.get(account).await?;
        ImapSession::open(
            &self.connector,
            account,
            &settings,
            &secret,
            self.tokens.as_ref(),
        )
        .await
    }
}
