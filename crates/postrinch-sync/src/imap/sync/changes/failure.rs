//! Why a change did not reach the server, and what becomes of it
//! (docs/imap.md, "Changing mail").
//!
//! - **Refused for good**: a `NO` whose code says the server will never
//!   take it (a folder that is gone, no permission, a fixed limit). The
//!   outbox entry is dropped at once with the reason, and its messages are
//!   read again, as Gmail's 400 and 404 are.
//! - **Declined**: a `NO` without such a code (or a `BAD`), whose words
//!   may or may not pass. It is tried again, keeping the outbox's order,
//!   and given up on once the entry has failed [`GIVE_UP`] times, so one
//!   change the server will never take cannot hold every later one back
//!   for ever.
//! - **Waits**: the change waits for something that comes in time: sync
//!   to read a folder whose UIDVALIDITY changed, the next send to look
//!   again for a copy the server did not confirm or make, or other mail
//!   marked deleted to go before a plain `EXPUNGE` (until sync has seen
//!   it). Tried again however long it takes, holding back only the later
//!   changes to its messages.
//! - **Wait**: the server busy (for a folder or the account) or out of
//!   reach. Waiting cures it, so it is tried again however long it takes.
//!
//! Only a declined send counts towards [`GIVE_UP`], and only for the entry
//! sent: waiting never makes a change be given up on.

use super::super::folder_error::FolderError;
use super::super::wire::Refusal;
use crate::SyncError;
use crate::imap::reply::Tagged;

/// How many failed sends of an entry the server declines without saying
/// it is for good are allowed before it is dropped.
pub(super) const GIVE_UP: u32 = 5;

/// What stopped a change.
#[derive(Debug)]
pub(super) enum Failure {
    /// Refused for good, with the reason to show.
    Refused(String),
    /// Declined in words that may pass, with the reason.
    Declined(String),
    /// Waiting for sync, or for other mail to go, with the reason.
    Waits(String),
    /// Busy or unreachable: tried again later.
    Wait(SyncError),
}

/// What a change's commands give.
pub(super) type Outcome<T> = Result<T, Failure>;

impl From<SyncError> for Failure {
    /// A `BAD` and other answers out of turn come as [`SyncError::Server`]:
    /// the server's, declined. Everything else is waited out.
    fn from(error: SyncError) -> Self {
        match error {
            SyncError::Server { message, .. } => Self::Declined(message),
            other => Self::Wait(other),
        }
    }
}

impl Failure {
    /// A failure in one folder: busy for it (`[INUSE]`, `[UNAVAILABLE]`)
    /// is waited out, as for the account.
    pub(super) fn in_folder(error: FolderError, server: String) -> Self {
        match error {
            FolderError::Busy(message) => Self::Wait(SyncError::Busy {
                service: "IMAP",
                server,
                message,
            }),
            FolderError::Sync(e) => e.into(),
        }
    }

    /// The error a caller outside the outbox sees (making, renaming or
    /// deleting a label): the server's own words for a refusal.
    pub(super) fn into_error(self, server: String) -> SyncError {
        match self {
            Self::Refused(message) | Self::Declined(message) | Self::Waits(message) => {
                SyncError::Server {
                    service: "IMAP",
                    server,
                    message,
                }
            }
            Self::Wait(e) => e,
        }
    }
}

/// What a tagged `NO` to `what` (a phrase such as `moving mail to
/// "Work"`) means for a change: its response code decides (RFC 5530,
/// RFC 3501's `TRYCREATE` and `READ-ONLY`, RFC 9208's `OVERQUOTA`).
pub(super) fn refusal(what: &str, refusal: &Refusal) -> Failure {
    let words = Tagged::parse_text(&refusal.text).text;
    let reason = if words.is_empty() {
        format!("the server refused {what}")
    } else {
        format!("the server refused {what}: {words}")
    };
    match refusal.code.as_deref() {
        Some(code) if for_good(code) => Failure::Refused(reason),
        _ => Failure::Declined(reason),
    }
}

/// Whether a response code says the server will never take the command
/// as it is. `LIMIT` after sign-in is a fixed implementation limit
/// (docs/imap.md, "Sync"); `OVERQUOTA` lifts only when the person makes
/// room, which an outbox waiting behind it cannot count on.
fn for_good(code: &str) -> bool {
    matches!(
        code,
        "TRYCREATE"
            | "NONEXISTENT"
            | "NOPERM"
            | "CANNOT"
            | "READ-ONLY"
            | "ALREADYEXISTS"
            | "LIMIT"
            | "OVERQUOTA"
            | "CLIENTBUG"
            | "BADCHARSET"
            | "PARSE"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no(code: Option<&str>, text: &str) -> Refusal {
        Refusal {
            text: text.to_owned(),
            code: code.map(str::to_owned),
        }
    }

    #[test]
    fn codes_decide_what_is_for_good() {
        let gone = refusal(
            "moving mail",
            &no(Some("NONEXISTENT"), "[NONEXISTENT] No such mailbox"),
        );
        assert!(
            matches!(&gone, Failure::Refused(r) if r == "the server refused moving mail: No such mailbox"),
            "{gone:?}"
        );
        let trycreate = refusal("copying mail", &no(Some("TRYCREATE"), "No such mailbox"));
        assert!(matches!(trycreate, Failure::Refused(_)));
        let bug = refusal("copying mail", &no(Some("SERVERBUG"), "[SERVERBUG] Oops"));
        assert!(matches!(bug, Failure::Declined(_)));
        let plain = refusal("copying mail", &no(None, ""));
        assert!(
            matches!(&plain, Failure::Declined(r) if r == "the server refused copying mail"),
            "{plain:?}"
        );
    }

    #[test]
    fn busy_and_unreachable_are_waited_out() {
        let busy = Failure::in_folder(FolderError::Busy("[INUSE] Locked".into()), "s".into());
        assert!(matches!(busy, Failure::Wait(SyncError::Busy { .. })));
        let bad = Failure::from(SyncError::Server {
            service: "IMAP",
            server: "s".into(),
            message: "UID MOVE refused: unknown command".into(),
        });
        assert!(matches!(bad, Failure::Declined(_)));
        let gone = Failure::from(SyncError::Unreachable {
            server: "s".into(),
            detail: "closed".into(),
        });
        assert!(matches!(gone, Failure::Wait(_)));
    }
}
