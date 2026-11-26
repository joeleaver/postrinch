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
//! (RFC 2177 says 29 minutes; Postrinch restarts sooner); or when told to
//! stop, so that the connection can sign out with no IDLE under way.
//! Everything sent before the `+` and after `DONE` is under the command
//! timeout; while idling, silence is what the server is meant to send.
//! A server may end an IDLE itself, and [`Idled`] says so: the caller
//! decides whether one that ended too soon means IDLE is not working.
//!
//! A `BYE` ends it: in words that say the server is busy it is
//! [`SyncError::Busy`] (a connection limit's said so,
//! [`names_connections`]), otherwise the connection lost
//! ([`SyncError::Unreachable`]), and so is a connection that closes.

use std::time::Duration;

use async_imap::imap_proto::{MailboxDatum, RequestId, Response, ResponseCode, Status};
use postrinch_model::Server;
use tokio::time::Instant;

use super::answers::{busy, code_name};
use super::idle::Idler;
use crate::imap::connect::{describe, within};
use crate::imap::refusal::{BusyScope, is_busy, names_connections};
use crate::imap::reply::ImapStream;
use crate::imap::session::imap_error;
use crate::{SyncError, SyncResult};

/// How one IDLE went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Idled<S> {
    /// The server told of changes in the inbox: a pass is due.
    pub(crate) news: bool,
    pub(crate) end: IdleEnd<S>,
}

/// What ended an IDLE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdleEnd<S> {
    /// Postrinch did, with `DONE`: news had settled, or it was time to
    /// begin again.
    Done,
    /// The server did, with its tagged `OK`, before or after its `+`.
    Server,
    /// The `stop` given to [`Idler::idle`] came, with this, and the IDLE
    /// was ended with `DONE`: the connection is ready to sign out.
    Stopped(S),
}

/// What a response read during an IDLE means.
#[derive(Debug)]
enum Heard {
    /// The server is ready: the IDLE has begun.
    Ready,
    /// A change in the inbox.
    News,
    /// The IDLE's tagged `OK` (any other tagged answer is an error).
    Ended,
    /// Anything else (`* OK Still here`, `RECENT`).
    Nothing,
}

type Session = async_imap::Session<ImapStream>;

impl Idler {
    /// IDLEs once: until news (and then `settle` more, for the rest of a
    /// burst), until `restart` has passed, or until `stop` comes,
    /// whichever is first. `ready` is called once the server has begun
    /// the IDLE. `stop` is heard while idling only: before the `+` and
    /// after `DONE`, the server's answer is waited for (under the command
    /// timeout), so the connection is left with no command under way.
    pub(crate) async fn idle<S>(
        &mut self,
        restart: Duration,
        settle: Duration,
        ready: impl FnOnce(),
        stop: impl Future<Output = S>,
    ) -> SyncResult<Idled<S>> {
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
                // Ended before it began.
                Heard::Ended => {
                    return Ok(Idled {
                        news,
                        end: IdleEnd::Server,
                    });
                }
                Heard::Nothing => {}
            }
        }
        let mut stop = std::pin::pin!(stop);
        let mut stopped = None;
        let mut settle_at = news.then(|| Instant::now() + settle);
        loop {
            let deadline = settle_at.map_or(restart_at, |at| at.min(restart_at));
            let read = tokio::select! {
                read = session.read_response() => read,
                () = tokio::time::sleep_until(deadline) => break,
                why = &mut stop => {
                    stopped = Some(why);
                    break;
                }
            };
            let response = received(read, &server, "idling")?;
            match heard(response.parsed(), &tag, &server)? {
                Heard::News => {
                    news = true;
                    settle_at.get_or_insert_with(|| Instant::now() + settle);
                }
                Heard::Ended => {
                    return Ok(Idled {
                        news,
                        end: IdleEnd::Server,
                    });
                }
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
                Heard::Ended => break,
                Heard::News => news = true,
                Heard::Ready | Heard::Nothing => {}
            }
        }
        let end = stopped.map_or(IdleEnd::Done, IdleEnd::Stopped);
        Ok(Idled { news, end })
    }
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
            return Err(bye(server, outcome.code.as_ref(), text));
        }
        _ => Heard::Nothing,
    })
}

/// What a `BYE` while idling means, from its response code as imap-proto
/// parsed it and its text: the server busy, in its words (as a `BYE`
/// before sign-in is read, [`is_busy`]), or the connection lost.
fn bye(server: &Server, code: Option<&ResponseCode<'_>>, text: String) -> SyncError {
    // imap-proto leaves codes it does not know at the start of the text.
    let code = code_name(code, &text);
    let code = code.as_deref();
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

    /// What the IDLE tagged `a7` makes of `line`, parsed as the wire
    /// gives it: the path every response read while idling takes.
    fn hear_line(line: &str) -> SyncResult<Heard> {
        let (rest, response) =
            Response::parse(line.as_bytes()).unwrap_or_else(|e| panic!("{line:?} parses: {e:?}"));
        assert!(rest.is_empty(), "{line:?} parsed whole");
        heard(&response, &RequestId("a7".into()), &server())
    }

    #[test]
    fn a_bye_is_busy_only_in_busy_words() {
        let busy =
            hear_line("* BYE [ALERT] Account exceeded command or bandwidth limits. (Failure)\r\n");
        assert!(
            matches!(&busy, Err(SyncError::Busy { connections: false, message, .. }) if message.contains("bandwidth")),
            "{busy:?}"
        );
        // imap-proto parses no `UNAVAILABLE` or `LIMIT`: they are left at
        // the start of the text, and read from there.
        for line in [
            "* BYE [UNAVAILABLE] Too many simultaneous connections\r\n",
            "* BYE [LIMIT] Try again later\r\n",
        ] {
            let crowded = hear_line(line);
            assert!(
                matches!(
                    &crowded,
                    Err(SyncError::Busy { connections: true, message, .. }) if !message.starts_with('[')
                ),
                "{line}: {crowded:?}"
            );
        }
        let lost = hear_line("* BYE Server shutting down\r\n");
        assert!(
            matches!(lost, Err(SyncError::Unreachable { .. })),
            "{lost:?}"
        );
    }

    #[test]
    fn what_is_heard_while_idling() {
        assert!(matches!(hear_line("+ idling\r\n"), Ok(Heard::Ready)));
        for news in [
            "* 4 EXISTS\r\n",
            "* 2 EXPUNGE\r\n",
            "* 1 FETCH (FLAGS (\\Seen))\r\n",
        ] {
            assert!(matches!(hear_line(news), Ok(Heard::News)), "{news}");
        }
        assert!(matches!(
            hear_line("* OK Still here\r\n"),
            Ok(Heard::Nothing)
        ));
        assert!(matches!(
            hear_line("a7 OK IDLE terminated\r\n"),
            Ok(Heard::Ended)
        ));
        // Another command's answer is not the IDLE's.
        assert!(matches!(hear_line("a6 OK done\r\n"), Ok(Heard::Nothing)));
        let refused = hear_line("a7 NO Not now\r\n");
        assert!(
            matches!(refused, Err(SyncError::Server { .. })),
            "{refused:?}"
        );
    }
}
