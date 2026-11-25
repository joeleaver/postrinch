//! Why work in one folder stopped short: the server too busy for that
//! folder alone ([`crate::imap::refusal::BusyScope::Folder`]), or an
//! error that ends the pass.
//!
//! A busy folder is kept apart from [`SyncError`] so that it cannot end
//! a pass by mistake: every command sent in a folder returns a
//! [`FolderResult`], and only the code that walks the folders can turn
//! it into "this folder was not read" (`pass.rs`, `bodies.rs`,
//! `renames.rs`).

use crate::SyncError;

/// Why work in a folder stopped.
#[derive(Debug)]
pub(super) enum FolderError {
    /// The server is busy for this folder (`NO [INUSE] Mailbox is
    /// locked`): nothing more is asked in it this pass, the folder counts
    /// as not read, and the pass goes on. Carries the server's text.
    Busy(String),
    /// Anything else, which ends the pass as it would anywhere.
    Sync(SyncError),
}

/// What a command sent in a folder, or work on one folder, gives.
pub(super) type FolderResult<T> = Result<T, FolderError>;

impl From<SyncError> for FolderError {
    fn from(error: SyncError) -> Self {
        Self::Sync(error)
    }
}
