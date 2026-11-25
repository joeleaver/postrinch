//! What a sync or a send did, and how a sync is getting on: the same for
//! every provider, so the window, the command line and the agents read
//! one shape whichever engine did the work ([`crate::MailSync`]).

use std::sync::Arc;

/// What a sync did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// Whether it listed the whole account rather than reading what
    /// changed since the last sync (Gmail's history; for IMAP, a folder
    /// read whole because its UIDVALIDITY was new).
    pub full: bool,
    /// Messages stored from their headers alone, to be downloaded later
    /// (backfill).
    pub headers: usize,
    /// Messages downloaded whole and stored.
    pub fetched: usize,
    /// Stored messages whose labels or flags were brought up to date.
    pub relabelled: usize,
    /// Messages removed because the provider no longer has them.
    pub removed: usize,
}

impl SyncReport {
    pub(crate) fn add(&mut self, other: SyncReport) {
        self.full |= other.full;
        self.headers += other.headers;
        self.fetched += other.fetched;
        self.relabelled += other.relabelled;
        self.removed += other.removed;
    }
}

/// How a sync is getting on, for a progress display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// A full sync has listed this many message ids so far.
    Listing { found: usize },
    /// Working through messages: `done` of `total` in this phase.
    Working {
        phase: Phase,
        done: usize,
        total: usize,
    },
}

/// What a sync is working through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Storing the headers of messages new to the store.
    Headers,
    /// Applying changes: new messages, labels, deletions.
    Changes,
    /// Downloading whole messages known only by their headers.
    Bodies,
}

pub type ProgressFn = Arc<dyn Fn(Progress) + Send + Sync>;

/// What sending an account's outbox did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SendReport {
    pub sent: usize,
    /// Why the provider refused the changes it would never take. Each was
    /// dropped and its messages shown as the provider has them.
    pub refused: Vec<String>,
}
