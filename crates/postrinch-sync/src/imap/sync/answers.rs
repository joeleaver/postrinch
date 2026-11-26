//! What a server's answers say beyond their status: whether a `NO` is
//! the server busy, and for what ([`busy`]); a refusal's code by name
//! ([`code_name`]); the codes of an `OK` that sync reads ([`Completed`]);
//! a folder's RFC 8474 `MAILBOXID` ([`mailbox_id_of`]).

use async_imap::imap_proto::ResponseCode;
use async_imap::imap_proto::rfc4315::UidSetMember;

use crate::imap::refusal::{BusyScope, busy_after_sign_in};
use crate::imap::reply::Tagged;

/// What a tagged `OK` said besides OK.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Completed {
    /// UIDPLUS's `[APPENDUID <uidvalidity> <uid>]`, for an APPEND of one
    /// message (RFC 4315 3).
    pub appended: Option<(u32, u32)>,
}

impl Completed {
    pub(super) fn of(code: Option<&ResponseCode<'_>>) -> Self {
        let appended = match code {
            Some(ResponseCode::AppendUid(uidvalidity, uids)) => match uids.as_slice() {
                [UidSetMember::Uid(uid)] => Some((*uidvalidity, *uid)),
                _ => None,
            },
            _ => None,
        };
        Self { appended }
    }
}

/// Whether a tagged `NO` says the server is busy, and for what
/// ([`busy_after_sign_in`]). imap-proto takes `[ALERT]` out of the text;
/// codes it does not know (RFC 5530's among them) stay at the start of
/// the text.
pub(super) fn busy(code: Option<&ResponseCode<'_>>, text: &str) -> Option<BusyScope> {
    match code {
        Some(ResponseCode::Alert) => busy_after_sign_in(Some("ALERT"), text),
        Some(_) => None,
        None => {
            let parsed = Tagged::parse_text(text);
            busy_after_sign_in(parsed.code.as_deref(), &parsed.text)
        }
    }
}

/// A tagged answer's response code by name: imap-proto parses a few
/// (`TRYCREATE`, `READ-ONLY`), and leaves the others at the start of the
/// text.
pub(super) fn code_name(code: Option<&ResponseCode<'_>>, text: &str) -> Option<String> {
    match code {
        Some(ResponseCode::TryCreate) => Some("TRYCREATE".to_owned()),
        Some(ResponseCode::ReadOnly) => Some("READ-ONLY".to_owned()),
        Some(ResponseCode::Alert) => Some("ALERT".to_owned()),
        Some(ResponseCode::BadCharset(_)) => Some("BADCHARSET".to_owned()),
        Some(_) => None,
        None => Tagged::parse_text(text).code,
    }
}

/// The id in an untagged `OK [MAILBOXID (<id>)]`'s text.
pub(super) fn mailbox_id_of(text: &str) -> Option<String> {
    let code = text
        .get(..11)
        .filter(|p| p.eq_ignore_ascii_case("[MAILBOXID "))?;
    let inner = text[code.len()..].strip_prefix('(')?;
    let id = &inner[..inner.find(')')?];
    (!id.is_empty()).then(|| id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_refusals_are_told_from_partial_ones() {
        use BusyScope::{Account, Folder};
        let busy_ones = [
            (None, "[UNAVAILABLE] Try later", Folder),
            (None, "[INUSE] Locked", Folder),
            (
                Some(ResponseCode::Alert),
                "Account exceeded command or bandwidth limits. (Failure)",
                Account,
            ),
            (
                None,
                "Too many simultaneous connections. (Failure)",
                Account,
            ),
        ];
        for (code, text, scope) in busy_ones {
            assert_eq!(busy(code.as_ref(), text), Some(scope), "{text}");
        }
        let partial = [
            (None, "Some messages could not be FETCHed (Failure)"),
            (None, "[SERVERBUG] Internal error occurred"),
            (None, "[LIMIT] Too many UIDs"),
            (Some(ResponseCode::Alert), "Mailbox is broken"),
        ];
        for (code, text) in partial {
            assert_eq!(busy(code.as_ref(), text), None, "{text}");
        }
    }

    #[test]
    fn appended_uids_are_read_from_the_code() {
        let one = ResponseCode::AppendUid(7, vec![UidSetMember::Uid(42)]);
        assert_eq!(Completed::of(Some(&one)).appended, Some((7, 42)));
        let many = ResponseCode::AppendUid(7, vec![UidSetMember::UidRange(1..=2)]);
        assert_eq!(Completed::of(Some(&many)).appended, None);
        assert_eq!(Completed::of(None), Completed::default());
    }

    #[test]
    fn mailbox_ids_are_read_from_the_text() {
        let id = mailbox_id_of("[MAILBOXID (F2212ea87)] Ok");
        assert_eq!(id.as_deref(), Some("F2212ea87"));
        assert_eq!(mailbox_id_of("[mailboxid (x)]").as_deref(), Some("x"));
        assert_eq!(mailbox_id_of("[MAILBOXID ()] Ok"), None);
        assert_eq!(mailbox_id_of("Ok"), None);
    }
}
