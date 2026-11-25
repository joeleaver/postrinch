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
    /// accounts are polled this often too, until IDLE tells them of new
    /// mail at once.
    pub interval: Duration,
    /// The pause before the next pass when an account failed.
    pub retry: Duration,
}

impl Default for KeepOptions {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(60),
            retry: Duration::from_secs(20),
        }
    }
}
