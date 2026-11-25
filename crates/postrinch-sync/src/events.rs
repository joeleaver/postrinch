//! What the keep-in-sync loops report, for every provider
//! ([`crate::MailSync::keep_in_sync`] and [`crate::MailSync::keep_sending`]),
//! and how often they run.

use std::sync::Arc;
use std::time::Duration;

use postrinch_model::EmailAddress;

use crate::{Progress, SendReport, SyncReport};

/// What a running sync reports, for a window or a log to show.
#[derive(Debug, Clone)]
pub enum SyncEvent {
    /// An account's pass began.
    Started { account: EmailAddress },
    Progress {
        account: EmailAddress,
        progress: Progress,
    },
    /// An account's mail is up to date with its provider; its bodies
    /// download next.
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
    /// The provider no longer accepts the account's sign-in. It is marked
    /// so in the store and left alone until it signs in again.
    NeedsSignIn { account: EmailAddress },
    /// A mail server turned the account away for now
    /// ([`SyncError::Busy`](crate::SyncError::Busy)): too many
    /// connections, a bandwidth limit. Its sign-in is fine, so it is not
    /// marked as needing to sign in; it is tried again after `retry_in`,
    /// which grows while the server stays busy ([`crate::Backoff`], one
    /// per account and server). `message` is the server's own sentence.
    /// Only IMAP accounts' servers answer so: Gmail's API has no such
    /// state.
    Busy {
        account: EmailAddress,
        message: String,
        retry_in: Duration,
    },
    /// Changes made here reached the provider; any it refused for good
    /// were dropped, and their messages shown as the provider has them.
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
    /// The pause between passes when every account succeeded. IMAP
    /// accounts are polled this often too while no IDLE watches their
    /// inbox ([`IdleOptions::interval`] while one does).
    pub interval: Duration,
    /// The pause before the next pass when an account failed.
    pub retry: Duration,
    /// How IMAP accounts IDLE on the inbox.
    pub idle: IdleOptions,
}

impl Default for KeepOptions {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(60),
            retry: Duration::from_secs(20),
            idle: IdleOptions::DEFAULT,
        }
    }
}

/// How an IMAP account's IDLE connection watches its inbox (docs/imap.md,
/// "Sync"). Every number is here so tests can make them short.
#[derive(Debug, Clone, Copy)]
pub struct IdleOptions {
    /// How long one IDLE lasts before it is ended and begun again on the
    /// same connection. RFC 2177 lets a server drop an IDLE after 29
    /// minutes without a word; 25 keeps well clear of it.
    pub restart: Duration,
    /// How long after the first news the IDLE waits for the rest of a
    /// burst (a message's `EXISTS` and its flags, a run of expunges)
    /// before it wakes one pass for all of it.
    pub settle: Duration,
    /// The pause between passes while an IDLE watches the inbox, in
    /// place of [`KeepOptions::interval`]: the inbox's news comes at
    /// once, so passes are only for the other folders.
    pub interval: Duration,
    /// How long the IDLE connection gives way when the server says the
    /// account has too many connections open: the account is polled
    /// meanwhile, and its other connections are left alone.
    pub give_way: Duration,
    /// The first wait before connecting again after the connection was
    /// lost or failed, doubling up to [`Self::reconnect_cap`]
    /// ([`crate::Backoff`]).
    pub reconnect: Duration,
    pub reconnect_cap: Duration,
}

impl IdleOptions {
    pub const DEFAULT: Self = Self {
        restart: Duration::from_secs(25 * 60),
        settle: Duration::from_secs(1),
        interval: Duration::from_secs(5 * 60),
        give_way: Duration::from_secs(30 * 60),
        reconnect: crate::Backoff::FIRST,
        reconnect_cap: crate::Backoff::CAP,
    };
}

impl Default for IdleOptions {
    fn default() -> Self {
        Self::DEFAULT
    }
}
