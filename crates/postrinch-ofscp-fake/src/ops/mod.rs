//! What REST routes and WebSocket commands share: the message operations
//! and their checks, so both transports apply the same rules (§7.1:
//! "Rules (both transports)").

pub(crate) mod attach;
pub(crate) mod dms;
pub(crate) mod messages;
pub(crate) mod notify;
pub(crate) mod paging;

/// A create that may have been a retry.
#[derive(Debug, Clone)]
pub(crate) enum Made<T> {
    /// Newly made, and fanned out.
    New(T),
    /// Already there (idempotency, §7.1): nothing was made or fanned out.
    Existing(T),
}

impl<T> Made<T> {
    pub(crate) fn into_inner(self) -> T {
        match self {
            Self::New(t) | Self::Existing(t) => t,
        }
    }
}
