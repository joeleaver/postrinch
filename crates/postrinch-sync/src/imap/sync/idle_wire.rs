//! One IDLE on the inbox (RFC 2177), as the wire has it:
//!
//! ```text
//! C: a7 IDLE
//! S: + idling
//! S: * 4 EXISTS          news: the IDLE ends a moment later
//! C: DONE
//! S: a7 OK IDLE terminated
//! ```
//!
//! [`Idler::idle`] waits until the server tells of news (new mail,
//! expunges, flags: `EXISTS`, `EXPUNGE`, `VANISHED`, `FETCH`), waits a
//! moment more for the rest of a burst, and ends the IDLE; or ends it
//! when it is time to restart, as servers may drop an IDLE that lasts
//! (RFC 2177 says 29 minutes; Postrinch restarts sooner). Everything sent
//! before the `+` and after `DONE` is under the command timeout; while
//! idling, silence is what the server is meant to send.
//!
//! A `BYE` ends it: in words that say the server is busy it is
//! [`SyncError::Busy`] (a connection limit's said so,
//! [`names_connections`]), otherwise the connection lost
//! ([`SyncError::Unreachable`]), and so is a connection that closes.

use std::time::Duration;

use async_imap::imap_proto::{MailboxDatum, RequestId, Response, Status};
use postrinch_model::Server;
use tokio::time::Instant;

use super::answers::{busy, code_name};
use super::idle::Idler;
use crate::imap::connect::{describe, within};
use crate::imap::refusal::{BusyScope, is_busy, names_connections};
use crate::imap::reply::ImapStream;
use crate::imap::session::imap_error;
use crate::{SyncError, SyncResult};

/// How one IDLE ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Idled {
    /// The server told of changes in the inbox: a pass is due.
    News,
    /// Time to begin again, with nothing to tell (or the server ended the
    /// IDLE itself).
    Restart,
}

/// What a response read during an IDLE means.
enum Heard {
    /// The server is ready: the IDLE has begun.
    Ready,
    /// A change in the inbox.
    News,
    /// The IDLE's tagged answer: `true` when it is `OK`.
    Ended,
    /// Anything else (`* OK Still here`, `RECENT`).
    Nothing,
}

type Session = async_imap::Session<ImapStream>;

impl Idler {
    /// IDLEs once: until news (and then `settle` more, for the rest of a
    /// burst), or until `restart` has passed, whichever comes first.
    /// `ready` is called once the server has begun the IDLE.
    pub(crate) async fn idle(
        &mut self,
        restart: Duration,
        settle: Duration,
        ready: impl FnOnce(),
    ) -> SyncResult<Idled> {
        let session = self.conn.session_mut();
        let server = session.server().clone();
        let limit = session.connector().timeouts().command;
        let session = session.inner();
        let restart_at = Instant::now() + restart;
        let tag = within(limit, &server, "starting IDLE", async {
            session
                .run_command("IDLE")
                .await
                .map_err(|e| imap_error(&server, e))
        })
        .await?;
        let mut news = false;
        loop {
            match hear(session, &tag, &server, limit, "starting IDLE").await? {
                Heard::Ready => {
                    ready();
                    break;
                }
                Heard::News => news = true,
                // Ended before it began: the next one begins at once.
                Heard::Ended => return Ok(ended(news)),
                Heard::Nothing => {}
            }
        }
        let mut settle_at = news.then(|| Instant::now() + settle);
        loop {
            let deadline = settle_at.map_or(restart_at, |at| at.min(restart_at));
            let read = tokio::select! {
                read = session.read_response() => Some(read),
                () = tokio::time::sleep_until(deadline) => None,
            };
            let Some(read) = read else { break };
            let response = received(read, &server, "idling")?;
            match heard(response.parsed(), &tag, &server)? {
                Heard::News => {
                    news = true;
                    settle_at.get_or_insert_with(|| Instant::now() + settle);
                }
                // A server may end an IDLE itself (as for a restart).
                Heard::Ended => return Ok(ended(news)),
                Heard::Ready | Heard::Nothing => {}
            }
        }
        within(limit, &server, "ending IDLE", async {
            session
                .run_command_untagged("DONE")
                .await
                .map_err(|e| imap_error(&server, e))
        })
        .await?;
        loop {
            match hear(session, &tag, &server, limit, "ending IDLE").await? {
                Heard::Ended => return Ok(ended(news)),
                Heard::News => news = true,
                Heard::Ready | Heard::Nothing => {}
            }
        }
    }
}

fn ended(news: bool) -> Idled {
    if news { Idled::News } else { Idled::Restart }
}

/// Reads the next response within the command timeout, and what it
/// means.
async fn hear(
    session: &mut Session,
    tag: &RequestId,
    server: &Server,
    limit: Duration,
    what: &str,
) -> SyncResult<Heard> {
    let read = within(limit, server, what, async {
        Ok(session.read_response().await)
    })
    .await?;
    heard(received(read, server, what)?.parsed(), tag, server)
}

/// A response read, or the connection closed.
fn received<T>(read: std::io::Result<Option<T>>, server: &Server, what: &str) -> SyncResult<T> {
    read?.ok_or_else(|| SyncError::Unreachable {
        server: describe(server),
        detail: format!("the connection closed while {what}"),
    })
}

/// What a response means for the IDLE tagged `tag`; a `BYE`, or a
/// tagged answer other than `OK`, is the error it says.
fn heard(response: &Response<'_>, tag: &RequestId, server: &Server) -> SyncResult<Heard> {
    Ok(match response {
        Response::Continue { .. } => Heard::Ready,
        Response::MailboxData(MailboxDatum::Exists(_))
        | Response::Expunge(_)
        | Response::Vanished { .. }
        | Response::Fetch(..) => Heard::News,
        Response::Done {
            tag: done,
            status,
            outcome,
        } if done == tag => {
            let text = outcome.information.as_deref().unwrap_or("").to_owned();
            match status {
                Status::Ok => Heard::Ended,
                Status::No if busy(outcome.code.as_ref(), &text) == Some(BusyScope::Account) => {
                    return Err(SyncError::Busy {
                        service: "IMAP",
                        server: describe(server),
                        connections: names_connections(
                            code_name(outcome.code.as_ref(), &text).as_deref(),
                            &text,
                        ),
                        message: text,
                    });
                }
                _ => {
                    return Err(SyncError::Server {
                        service: "IMAP",
                        server: describe(server),
                        message: format!("IDLE refused: {text}"),
                    });
                }
            }
        }
        Response::Data {
            status: Status::Bye,
            outcome,
        } => {
            let text = outcome.information.as_deref().unwrap_or("").to_owned();
            let code = code_name(outcome.code.as_ref(), &text);
            return Err(bye(server, code.as_deref(), text));
        }
        _ => Heard::Nothing,
    })
}

/// What a `BYE` while idling means: the server busy, in its words (as a
/// `BYE` before sign-in is read, [`is_busy`]), or the connection lost.
fn bye(server: &Server, code: Option<&str>, text: String) -> SyncError {
    // imap-proto leaves codes it does not know at the start of the text.
    let words = crate::imap::reply::Tagged::parse_text(&text).text;
    if is_busy(code, &words) {
        return SyncError::Busy {
            service: "IMAP",
            server: describe(server),
            connections: names_connections(code, &words),
            message: words,
        };
    }
    SyncError::Unreachable {
        server: describe(server),
        detail: format!("the server closed the connection while idling: {words}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postrinch_model::Security;

    fn server() -> Server {
        Server {
            host: "imap.example.org".into(),
            port: 993,
            security: Security::Tls,
            username: "ada".into(),
        }
    }

    #[test]
    fn a_bye_is_busy_only_in_busy_words() {
        let busy = bye(
            &server(),
            Some("ALERT"),
            "Account exceeded command or bandwidth limits. (Failure)".into(),
        );
        assert!(
            matches!(&busy, SyncError::Busy { connections: false, message, .. } if message.contains("bandwidth")),
            "{busy:?}"
        );
        let crowded = bye(
            &server(),
            None,
            "[UNAVAILABLE] Too many simultaneous connections".into(),
        );
        assert!(
            matches!(
                crowded,
                SyncError::Busy {
                    connections: true,
                    ..
                }
            ),
            "{crowded:?}"
        );
        let lost = bye(&server(), None, "Server shutting down".into());
        assert!(matches!(lost, SyncError::Unreachable { .. }), "{lost:?}");
    }
}
