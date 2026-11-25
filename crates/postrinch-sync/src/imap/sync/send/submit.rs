//! One SMTP transaction, in two steps so the caller can record when its
//! data is about to go ([`begin`]: MAIL, RCPT, DATA; then [`data`]), and
//! with what each answer means for sending it again (docs/imap.md,
//! "Writing and sending"). A server that takes only so many recipients at
//! once says so at RCPT; [`begin`] then goes on with those it took, and
//! the caller sends to the rest in another transaction.
//!
//! lettre's own `send` cannot be used: it reports every failure alike, and
//! the one thing that matters here is whether the message may have
//! reached the server.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lettre::Address;
use lettre::transport::smtp::commands::{Data, Mail, Rcpt as Command};
use lettre::transport::smtp::extension::{Extension, MailBodyParameter, MailParameter};
use lettre::transport::smtp::response::{Response, Severity};
use postrinch_model::{EmailAddress, Server};

use super::super::wire::for_bytes;
use crate::SyncError;
use crate::imap::SmtpConnection;
use crate::imap::connect::within;
use crate::imap::smtp::{connect_error, reply_text};

/// How a submission ended.
#[derive(Debug)]
pub(super) enum Submitted {
    /// The server took it (a 2xx after the data).
    Accepted,
    /// It did not go: a 4xx before the data was taken (the server busy,
    /// [`SyncError::Busy`]), or the connection lost before all the data
    /// was sent. To try again.
    Failed(SyncError),
    /// The server refused it for good (5xx), in its words: it waits for
    /// the person.
    Refused(String),
    /// All the data was sent and no answer came: it may have gone.
    Uncertain(String),
}

/// "Too many recipients" (RFC 5321 4.5.3.1.10), the answer to a RCPT
/// beyond what the server takes in one transaction.
const TOO_MANY_RECIPIENTS: u16 = 452;

/// MAIL FROM `from`, RCPT TO each of `to`, DATA, up to the server's 354.
/// Returns how many of `to`, from the first, the transaction is for: all
/// of them, or fewer when the server answered a RCPT with "too many
/// recipients" (452) after taking some, as RFC 5321 4.5.3.1.10 has a
/// client then send to those and the rest in another transaction. `Err`
/// is [`Submitted::Failed`] or [`Submitted::Refused`]: nothing has gone
/// in this transaction. Any refused recipient refuses the message, naming
/// the address, so this transaction never goes to some of its recipients
/// only.
pub(super) async fn begin(
    smtp: &mut SmtpConnection,
    limit: Duration,
    from: &EmailAddress,
    to: &[EmailAddress],
    message: &[u8],
) -> Result<usize, Submitted> {
    let server = smtp.server().clone();
    let info = smtp.inner().server_info();
    let mut parameters = Vec::new();
    let addresses = || std::iter::once(from).chain(to);
    if let Some(wide) = addresses().find(|a| !a.to_string().is_ascii()) {
        if !info.supports_feature(Extension::SmtpUtfEight) {
            return Err(Submitted::Refused(format!(
                "{} cannot send mail for {wide}: it takes only ASCII addresses",
                describe(&server)
            )));
        }
        parameters.push(MailParameter::SmtpUtfEight);
    }
    if !message.is_ascii() {
        if !info.supports_feature(Extension::EightBitMime) {
            return Err(Submitted::Refused(format!(
                "{} takes only 7-bit messages",
                describe(&server)
            )));
        }
        parameters.push(MailParameter::Body(MailBodyParameter::EightBitMime));
    }
    let mail = Mail::new(Some(address(from)?), parameters);
    step(smtp, &server, limit, mail).await?;
    let mut taken = 0;
    for recipient in to {
        match rcpt(smtp, &server, limit, recipient).await? {
            Rcpt::Taken => taken += 1,
            // None taken: the server can take nobody now, which is it
            // busy (a 4xx), to try again later.
            Rcpt::TooMany if taken == 0 => {
                return Err(Submitted::Failed(SyncError::Busy {
                    service: "SMTP",
                    server: describe(&server),
                    message: format!("{recipient}: too many recipients"),
                }));
            }
            Rcpt::TooMany => break,
        }
    }
    let answer = step(smtp, &server, limit, Data).await?;
    if !answer.has_code(354) {
        return Err(Submitted::Failed(SyncError::Server {
            service: "SMTP",
            server: describe(&server),
            message: format!("DATA was answered {}", answer.code()),
        }));
    }
    Ok(taken)
}

/// Sends the message's data, after [`begin`].
///
/// The data counts as sent once all of it was written: the server cannot
/// take a message before its end, so a connection lost while writing it
/// is [`Submitted::Failed`], and one lost after is
/// [`Submitted::Uncertain`]. A 4xx or 5xx answer after the data means it
/// did not go.
pub(super) async fn data(smtp: &mut SmtpConnection, limit: Duration, message: &[u8]) -> Submitted {
    let server = smtp.server().clone();
    let written = AtomicBool::new(false);
    // lettre asks for the next part once the message is written, before
    // the end-of-data line.
    let parts = std::iter::once(message).chain(std::iter::from_fn(|| {
        written.store(true, Ordering::Release);
        None
    }));
    // Writing it, then the server's answer, which may take a while
    // (spam and virus checks).
    let allowed = for_bytes(limit, message.len()) + limit;
    let connection = smtp.inner();
    let answered = within(allowed, &server, "sending the message", async {
        Ok(connection.message_iter(parts).await)
    })
    .await;
    let written = written.load(Ordering::Acquire);
    match answered {
        Ok(Ok(answer)) if answer.code().severity == Severity::PositiveCompletion => {
            Submitted::Accepted
        }
        Ok(Ok(answer)) => Submitted::Uncertain(format!(
            "{} answered the message {}",
            describe(&server),
            answer.code()
        )),
        Ok(Err(e)) if e.status().is_some() => refused_or_failed(&server, e, None),
        Ok(Err(e)) if written => Submitted::Uncertain(reply_text(&e)),
        Ok(Err(e)) => Submitted::Failed(connect_error(&server, e)),
        Err(e) if written => Submitted::Uncertain(e.to_string()),
        Err(e) => Submitted::Failed(e),
    }
}

/// How the server answered one RCPT, short of refusing it.
enum Rcpt {
    Taken,
    /// "Too many recipients": the transaction takes no more.
    TooMany,
}

/// RCPT TO `recipient`.
async fn rcpt(
    smtp: &mut SmtpConnection,
    server: &Server,
    limit: Duration,
    recipient: &EmailAddress,
) -> Result<Rcpt, Submitted> {
    let command = Command::new(address(recipient)?, Vec::new());
    let connection = smtp.inner();
    let answered = within(limit, server, "starting the message", async {
        Ok(connection.command(command).await)
    })
    .await;
    match answered {
        Ok(Ok(_)) => Ok(Rcpt::Taken),
        Ok(Err(e)) if e.status().map(u16::from) == Some(TOO_MANY_RECIPIENTS) => Ok(Rcpt::TooMany),
        Ok(Err(e)) => Err(refused_or_failed(server, e, Some(recipient))),
        Err(e) => Err(Submitted::Failed(e)),
    }
}

/// Sends one command of the envelope and reads its answer.
async fn step(
    smtp: &mut SmtpConnection,
    server: &Server,
    limit: Duration,
    command: impl std::fmt::Display,
) -> Result<Response, Submitted> {
    let connection = smtp.inner();
    let answered = within(limit, server, "starting the message", async {
        Ok(connection.command(command).await)
    })
    .await;
    match answered {
        Ok(Ok(answer)) => Ok(answer),
        Ok(Err(e)) => Err(refused_or_failed(server, e, None)),
        Err(e) => Err(Submitted::Failed(e)),
    }
}

/// A 5xx is the message refused, naming the recipient it was about; a
/// 4xx is the server busy ([`connect_error`] makes it
/// [`SyncError::Busy`]), and a lost connection is unreachable: both to
/// try again.
fn refused_or_failed(
    server: &Server,
    error: lettre::transport::smtp::Error,
    recipient: Option<&EmailAddress>,
) -> Submitted {
    if error.is_permanent() {
        let words = reply_text(&error);
        return Submitted::Refused(match recipient {
            Some(recipient) => format!("{recipient}: {words}"),
            None => words,
        });
    }
    Submitted::Failed(connect_error(server, error))
}

/// An address as lettre takes it.
fn address(address: &EmailAddress) -> Result<Address, Submitted> {
    address
        .to_string()
        .parse()
        .map_err(|_| Submitted::Refused(format!("{address} cannot be sent to over SMTP")))
}

fn describe(server: &Server) -> String {
    crate::imap::connect::describe(server)
}

/// The message as it goes to its recipients: without its `Bcc` header,
/// which Sent keeps but nobody it goes to may see (RFC 5322 3.6.3).
/// Only the message's own header block is touched, not those of its
/// parts.
pub(super) fn without_bcc(raw: &[u8]) -> Vec<u8> {
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map_or(raw.len(), |at| at + 2);
    let mut out = Vec::with_capacity(raw.len());
    let mut skipping = false;
    for line in raw[..end].split_inclusive(|&b| b == b'\n') {
        let folded = line.first().is_some_and(|&b| b == b' ' || b == b'\t');
        if !folded {
            skipping = line.len() >= 4 && line[..4].eq_ignore_ascii_case(b"bcc:");
        }
        if !skipping {
            out.extend_from_slice(line);
        }
    }
    out.extend_from_slice(&raw[end..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bcc_is_taken_out_of_the_header_only() {
        let raw = b"From: a@example.org\r\nBcc: b@example.org,\r\n c@example.org\r\n\
                    Subject: Hi\r\n\r\nBcc: stays in the body\r\n";
        assert_eq!(
            without_bcc(raw),
            b"From: a@example.org\r\nSubject: Hi\r\n\r\nBcc: stays in the body\r\n"
        );
        let plain = b"From: a@example.org\r\nSubject: Hi\r\n\r\nHello\r\n";
        assert_eq!(without_bcc(plain), plain);
        assert_eq!(
            without_bcc(b"BCC: x@example.org\r\nSubject: y\r\n"),
            b"Subject: y\r\n"
        );
    }
}
