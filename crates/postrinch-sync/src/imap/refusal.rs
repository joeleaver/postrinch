//! What an IMAP server's refusal to sign in means: a refused credential,
//! an administrator's policy, a server too busy for the account just now,
//! or a server failing.
//!
//! The response code decides first (RFC 5530). Only a NO with `[ALERT]`
//! or no code at all is read for its words, against
//! [`TEMPORARY_LIMITS`]: providers put both real refusals and temporary
//! limits under `[ALERT]`, and some send no code for either.
//!
//! The same limits turn up outside the answer to LOGIN or AUTHENTICATE:
//! as a `BYE` instead of the greeting ([`is_busy_bye`]), and as a `BAD`
//! to the first commands after signing in (`session.rs`).

/// What a tagged NO to LOGIN or AUTHENTICATE means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefusalKind {
    /// The credential was refused: the account needs to sign in (and an
    /// OAuth token is fetched afresh once first).
    Credential,
    /// An administrator's policy refuses it: signing in again will not
    /// help, nor will a fresh token.
    Admin,
    /// The server turned the account away for now (too many connections,
    /// a bandwidth limit, its sign-in service down): to try again later,
    /// backing off, without a fresh token or asking the person anything.
    Busy,
    /// The server failed: to try again later, not the account's to fix.
    Server,
}

impl RefusalKind {
    /// Classifies a refusal by its response code, then by its text.
    ///
    /// - `AUTHENTICATIONFAILED`, `AUTHORIZATIONFAILED` (the credential is
    ///   good but not for this user) and `EXPIRED` (the password or token
    ///   expired) refuse the credential, whatever the text says.
    /// - `CONTACTADMIN` is the admin's.
    /// - `UNAVAILABLE`, `INUSE` and `LIMIT`, RFC 5530's "try again later"
    ///   codes, are a busy server. Dovecot sends them for its temporary
    ///   authentication failure and its connection and login limits
    ///   (`src/imap-login/client-authenticate.c`); Gmail sends
    ///   `[UNAVAILABLE] Temporary System Error`.
    /// - `ALERT` (RFC 9051 7.1) only asks for its text to be shown, so it
    ///   counts as no code: a known temporary limit ([`is_temporary_limit`])
    ///   is a busy server, anything else a refused credential, as Gmail's
    ///   "[ALERT] Application-specific password required" is.
    /// - Every other code (`SERVERBUG`, `CORRUPTION`, `PRIVACYREQUIRED`,
    ///   which over Postrinch's TLS-only connections is the server's
    ///   fault, and any code Postrinch does not know) is the server's.
    pub(crate) fn of(code: Option<&str>, text: &str) -> Self {
        match code {
            Some("AUTHENTICATIONFAILED" | "AUTHORIZATIONFAILED" | "EXPIRED") => Self::Credential,
            Some("CONTACTADMIN") => Self::Admin,
            Some("UNAVAILABLE" | "INUSE" | "LIMIT") => Self::Busy,
            None | Some("ALERT") if is_temporary_limit(text) => Self::Busy,
            None | Some("ALERT") => Self::Credential,
            Some(_) => Self::Server,
        }
    }
}

/// How servers say "not now" in words, without a code that says so: each
/// is matched anywhere in the text, ignoring case, so Gmail's trailing
/// "(Failure)" and a server's own punctuation do not matter. Lower case.
///
/// Sources, checked 2026-11-24:
///
/// - Gmail, `NO [ALERT] Too many simultaneous connections. (Failure)`:
///   its limit of 15 IMAP connections per account (Atlassian, "Email
///   Handler for Gmail does not process all Email Requests",
///   <https://support.atlassian.com/jira/kb/email-handler-for-gmail-does-not-process-all-email-requests/>;
///   Mozilla support, <https://support.mozilla.org/questions/1045818>).
/// - Gmail, `NO [ALERT] Account exceeded command or bandwidth limits.
///   (Failure)`: the daily IMAP download and upload limits, which lift
///   when the period resets (Google Workspace, "Gmail bandwidth limits",
///   <https://knowledge.workspace.google.com/admin/gmail/gmail-bandwidth-limits>;
///   Mozilla bug 805830, <https://bugzilla.mozilla.org/show_bug.cgi?id=805830>).
/// - Microsoft 365, "User is authenticated but not connected": answered
///   when sessions are opened and closed too fast, and gone a few minutes
///   later (Microsoft Q&A,
///   <https://learn.microsoft.com/en-us/answers/questions/5571236/user-is-authenticated-but-not-connected>).
///   It mostly comes as a BAD to the first command after a sign-in that
///   succeeded, which `ImapSession` reads the same way.
/// - Dovecot, "Maximum number of connections from user+IP exceeded"
///   (`mail_max_userip_connections`), and "Temporary authentication
///   failure." (its `AUTH_TEMP_FAILED_MSG`): current versions send them
///   with `[LIMIT]` or `[UNAVAILABLE]`, older ones as a bare NO (Debian
///   list, <https://lists.debian.org/debian-kde/2013/11/msg00024.html>;
///   Dovecot `src/login-common/client-common-auth.c`). The connection
///   limit also comes as a `BYE` in place of the greeting, when the
///   connection is turned away before any command.
///
/// Gmail's other `[ALERT]`s are real refusals and stay so: "Application-
/// specific password required", "Please log in via your web browser",
/// "Web login required", and "Your account is not enabled for IMAP use"
/// (which the person fixes in Gmail's settings, so it is theirs to see as
/// a sign-in problem, not something to wait out).
pub(crate) const TEMPORARY_LIMITS: &[&str] = &[
    "too many simultaneous connections",
    "account exceeded command or bandwidth limits",
    "user is authenticated but not connected",
    "maximum number of connections from user+ip exceeded",
    "temporary authentication failure",
];

/// Whether a server's text is one of the known temporary limits.
pub(crate) fn is_temporary_limit(text: &str) -> bool {
    let text = text.to_lowercase();
    TEMPORARY_LIMITS.iter().any(|phrase| text.contains(phrase))
}

/// Whether a `BYE` before signing in (in place of the greeting, or
/// while CAPABILITY or STARTTLS was asked) turns the account away for
/// now rather than saying the server failed: RFC 5530's "try again
/// later" codes, or, with `[ALERT]` or no code, a known temporary limit.
/// Any other code, or other words, is the server's.
pub(crate) fn is_busy_bye(code: Option<&str>, text: &str) -> bool {
    match code {
        Some("UNAVAILABLE" | "INUSE" | "LIMIT") => true,
        None | Some("ALERT") => is_temporary_limit(text),
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::RefusalKind::{self, Admin, Busy, Credential, Server};
    use super::*;

    #[test]
    fn codes_decide_before_words() {
        let cases = [
            (Some("AUTHENTICATIONFAILED"), Credential),
            (Some("AUTHORIZATIONFAILED"), Credential),
            (Some("EXPIRED"), Credential),
            (Some("CONTACTADMIN"), Admin),
            (Some("UNAVAILABLE"), Busy),
            (Some("INUSE"), Busy),
            (Some("LIMIT"), Busy),
            (Some("SERVERBUG"), Server),
            (Some("CORRUPTION"), Server),
            (Some("PRIVACYREQUIRED"), Server),
            (Some("CANNOT"), Server),
            (Some("SOMETHINGNEW"), Server),
        ];
        for (code, kind) in cases {
            assert_eq!(RefusalKind::of(code, "Try later"), kind, "{code:?}");
            // A limit's words do not override a code that says otherwise.
            let limit = "Too many simultaneous connections";
            assert_eq!(RefusalKind::of(code, limit), kind, "{code:?}");
        }
    }

    #[test]
    fn gmails_alerts() {
        let cases = [
            ("Too many simultaneous connections. (Failure)", Busy),
            (
                "Account exceeded command or bandwidth limits. (Failure)",
                Busy,
            ),
            (
                "Application-specific password required: https://support.google.com/accounts/answer/185833 (Failure)",
                Credential,
            ),
            (
                "Please log in via your web browser: https://support.google.com/mail/accounts/answer/78754 (Failure)",
                Credential,
            ),
            (
                "Web login required: https://support.google.com/mail/answer/78754 (Failure)",
                Credential,
            ),
            (
                "Your account is not enabled for IMAP use. Please visit your Gmail settings page and enable your account for IMAP access. (Failure)",
                Credential,
            ),
        ];
        for (text, kind) in cases {
            assert_eq!(RefusalKind::of(Some("ALERT"), text), kind, "{text}");
            assert_eq!(RefusalKind::of(None, text), kind, "bare NO: {text}");
        }
    }

    #[test]
    fn other_servers_words_without_a_code() {
        let busy = [
            "User is authenticated but not connected.",
            "Maximum number of connections from user+IP exceeded (mail_max_userip_connections=10)",
            "Temporary authentication failure.",
        ];
        for text in busy {
            assert_eq!(RefusalKind::of(None, text), Busy, "{text}");
        }
        assert_eq!(RefusalKind::of(None, "Invalid credentials"), Credential);
        assert_eq!(RefusalKind::of(None, ""), Credential);
    }

    #[test]
    fn busy_byes() {
        assert!(is_busy_bye(Some("UNAVAILABLE"), "Too busy"));
        assert!(is_busy_bye(Some("INUSE"), ""));
        assert!(is_busy_bye(Some("LIMIT"), "Try later"));
        assert!(is_busy_bye(
            None,
            "Maximum number of connections from user+IP exceeded (mail_max_userip_connections=10)"
        ));
        assert!(is_busy_bye(
            Some("ALERT"),
            "Too many simultaneous connections"
        ));
        assert!(!is_busy_bye(None, "Server shutting down"));
        assert!(!is_busy_bye(
            Some("SERVERBUG"),
            "Too many simultaneous connections"
        ));
    }

    #[test]
    fn matching_ignores_case_and_trailing_words() {
        assert!(is_temporary_limit("TOO MANY SIMULTANEOUS CONNECTIONS"));
        assert!(is_temporary_limit(
            "  account exceeded command or bandwidth limits (Failure)  "
        ));
        assert!(!is_temporary_limit("Too many"));
        for phrase in TEMPORARY_LIMITS {
            assert_eq!(*phrase, phrase.to_lowercase(), "kept lower case");
        }
    }
}
