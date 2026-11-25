//! One SMTP transaction, in two steps so the caller can record when its
//! data is about to go ([`begin`]: MAIL, RCPT, DATA; then [`data`]), and
//! with what each answer means for sending it again (docs/imap.md,
//! "Writing and sending"). A server that takes only so many recipients at
//! once says so at RCPT; [`begin`] then goes on with those it took, and
//! the caller sends to the rest in another transaction. One it puts off
//! for now (a 4xx for one recipient, as a mailbox full) is left out of
//! the transaction, for the caller to try again later.
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
use crate::imap::smtp::{connect_error, enhanced_code, reply_text};

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

/// The recipients one transaction is for, as their RCPTs were answered.
#[derive(Debug, Default)]
pub(super) struct Envelope {
    /// Those the server took, in order: the transaction is for them.
    pub taken: Vec<EmailAddress>,
    /// Those the server put off for now (a 4xx for them alone: their
    /// mailbox full, say), with its words: to try again later.
    pub put_off: Vec<(EmailAddress, String)>,
    /// Those not yet asked for, the first of them answered "too many
    /// recipients": for another transaction.
    pub rest: Vec<EmailAddress>,
}

/// MAIL FROM `from`, RCPT TO each of `to`, and, when the server took
/// some of them, DATA, up to the server's 354. The [`Envelope`] says whom
/// the transaction is for: the server may put some recipients off (a
/// 4xx for one recipient), and may answer "too many recipients" (RFC 5321
/// 4.5.3.1.10) after taking some, which has a client send to those and
/// the rest in another transaction. With none taken, no DATA is sent.
/// `Err` is [`Submitted::Failed`] or [`Submitted::Refused`]: nothing has
/// gone in this transaction. Any refused recipient refuses the message,
/// naming the address, so this transaction never goes to some of its
/// recipients only.
pub(super) async fn begin(
    smtp: &mut SmtpConnection,
    limit: Duration,
    from: &EmailAddress,
    to: &[EmailAddress],
    message: &[u8],
) -> Result<Envelope, Submitted> {
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
    let mut envelope = Envelope::default();
    for (at, recipient) in to.iter().enumerate() {
        let some_taken = !envelope.taken.is_empty();
        match rcpt(smtp, &server, limit, recipient, some_taken).await? {
            Rcpt::Taken => envelope.taken.push(recipient.clone()),
            Rcpt::PutOff(words) => envelope.put_off.push((recipient.clone(), words)),
            Rcpt::TooMany => {
                envelope.rest = to[at..].to_vec();
                break;
            }
        }
    }
    if envelope.taken.is_empty() {
        return Ok(envelope);
    }
    let answer = step(smtp, &server, limit, Data).await?;
    if !answer.has_code(354) {
        return Err(Submitted::Failed(SyncError::Server {
            service: "SMTP",
            server: describe(&server),
            message: format!("DATA was answered {}", answer.code()),
        }));
    }
    Ok(envelope)
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
#[derive(Debug, PartialEq, Eq)]
enum Rcpt {
    Taken,
    /// A 4xx for this recipient alone, in the server's words: it is put
    /// off, and the transaction goes on for the others.
    PutOff(String),
    /// "Too many recipients": the transaction takes no more.
    TooMany,
}

/// RCPT TO `recipient`, after `some_taken` recipients were taken in this
/// transaction. "Too many recipients" with none taken is the server
/// taking nobody now: busy for a 4xx, to try again later, refused for a
/// 5xx.
async fn rcpt(
    smtp: &mut SmtpConnection,
    server: &Server,
    limit: Duration,
    recipient: &EmailAddress,
    some_taken: bool,
) -> Result<Rcpt, Submitted> {
    let command = Command::new(address(recipient)?, Vec::new());
    let connection = smtp.inner();
    let answered = within(limit, server, "starting the message", async {
        Ok(connection.command(command).await)
    })
    .await;
    let error = match answered {
        Ok(Ok(_)) => return Ok(Rcpt::Taken),
        Ok(Err(e)) => e,
        Err(e) => return Err(Submitted::Failed(e)),
    };
    let Some(code) = error.status().map(u16::from) else {
        return Err(refused_or_failed(server, error, Some(recipient)));
    };
    match rcpt_answer(code, enhanced_code(&error), some_taken) {
        RcptAnswer::TooMany if some_taken => Ok(Rcpt::TooMany),
        RcptAnswer::TooMany if code < 500 => Err(Submitted::Failed(SyncError::Busy {
            service: "SMTP",
            server: describe(server),
            message: format!("{recipient}: too many recipients"),
            connections: false,
        })),
        RcptAnswer::PutOff => Ok(Rcpt::PutOff(reply_text(&error))),
        RcptAnswer::TooMany | RcptAnswer::Other => {
            Err(refused_or_failed(server, error, Some(recipient)))
        }
    }
}

/// What an answer to RCPT other than 2xx means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RcptAnswer {
    /// "Too many recipients" (RFC 5321 4.5.3.1.10).
    TooMany,
    /// A temporary failure for this recipient alone.
    PutOff,
    /// Anything else: refused (5xx), or the connection closing (421).
    Other,
}

/// Reads `code` (with its enhanced status code, RFC 3463), after
/// `some_taken` recipients were taken in this transaction.
///
/// "Too many recipients" is the enhanced code X.5.3 ("too many
/// recipients", RFC 3463 3.6), or 452 or 552 without an enhanced code
/// once some recipients were taken (RFC 5321 4.5.3.1.10). A 452 with
/// another enhanced code is about this recipient (4.2.2, its mailbox is
/// full, as Gmail answers; 4.3.1, the server out of storage), and is put
/// off like any other 4xx but 421, which closes the connection. A 552
/// that is not "too many" is refused.
fn rcpt_answer(code: u16, enhanced: Option<[u16; 3]>, some_taken: bool) -> RcptAnswer {
    let class = code / 100;
    let too_many = match enhanced {
        Some([enhanced_class, 5, 3]) => enhanced_class == class,
        Some(_) => false,
        None => some_taken && matches!(code, 452 | 552),
    };
    if too_many {
        RcptAnswer::TooMany
    } else if class == 4 && code != 421 {
        RcptAnswer::PutOff
    } else {
        RcptAnswer::Other
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
    fn too_many_recipients_is_told_from_a_recipients_own_failure() {
        use RcptAnswer::{Other, PutOff, TooMany};
        let cases = [
            // RFC 5321 4.5.3.1.10, with and without an enhanced code.
            (452, Some([4, 5, 3]), true, TooMany),
            (452, Some([4, 5, 3]), false, TooMany),
            (452, None, true, TooMany),
            (552, None, true, TooMany),
            (552, Some([5, 5, 3]), false, TooMany),
            (451, Some([4, 5, 3]), true, TooMany),
            // The recipient's mailbox full (Gmail), the server out of
            // storage: this recipient's own, put off.
            (452, Some([4, 2, 2]), true, PutOff),
            (452, Some([4, 3, 1]), true, PutOff),
            (452, None, false, PutOff),
            (450, Some([4, 2, 1]), false, PutOff),
            (451, None, true, PutOff),
            // A 552 that is not "too many" is refused.
            (552, Some([5, 2, 2]), true, Other),
            (552, None, false, Other),
            (550, Some([5, 1, 1]), true, Other),
            (421, None, true, Other),
            (452, Some([5, 5, 3]), true, PutOff),
        ];
        for (code, enhanced, some_taken, meaning) in cases {
            assert_eq!(
                rcpt_answer(code, enhanced, some_taken),
                meaning,
                "{code} {enhanced:?} after some taken: {some_taken}"
            );
        }
    }

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
