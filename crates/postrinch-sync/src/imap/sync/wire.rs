//! The commands sync sends, read response by response.
//!
//! async-imap passes responses it did not ask for (QRESYNC's `VANISHED`
//! among them) through a bounded channel that drops what does not fit,
//! and keeps no `MODSEQ` of a `SELECT` it did not send. Sync cannot lose
//! a `VANISHED`, so it sends its commands itself and reads every response
//! up to the tagged one ([`Conn::command`]). Each response read is under
//! the command timeout, so a long listing of a big folder is not cut off
//! while the server keeps answering, and a server that stops answering
//! cannot hang the account.
//!
//! A `NO` saying the server is busy ([`busy`]) ends the pass when it is
//! the account's ([`SyncError::Busy`]), and only the folder when it is
//! the folder's ([`FolderError::Busy`]): the commands sent in a folder
//! return a [`FolderResult`].

use std::time::Duration;

use async_imap::imap_proto::{MailboxDatum, Response, ResponseCode, Status};
use postrinch_model::Server;
use tokio::io::AsyncWriteExt;

pub(super) use super::answers::Completed;
use super::answers::busy;
use super::fetch::Collector;
pub(super) use super::fetch::{FetchReply, Fetched};
use super::folder_error::{FolderError, FolderResult};
use super::names::Command;
pub(super) use super::uids::uid_set;
use crate::imap::connect::{describe, within};
use crate::imap::refusal::BusyScope;
use crate::imap::reply::ImapStream;
use crate::imap::session::{ImapSession, imap_error};
use crate::{SyncError, SyncResult};

/// A signed-in session as sync uses it, and whether QRESYNC is enabled.
pub(super) struct Conn {
    session: ImapSession,
    qresync: bool,
}

/// What `SELECT` (or `EXAMINE`) said about a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Selected {
    pub uidvalidity: u32,
    pub uidnext: Option<u32>,
    /// `None` when the server keeps no MODSEQs for the folder.
    pub highest_modseq: Option<u64>,
    pub exists: u32,
    /// RFC 8474's `MAILBOXID`, where the server offers OBJECTID.
    pub mailbox_id: Option<String>,
}

/// How a command ended, short of an error.
pub(super) enum Done {
    Ok(Completed),
    No(Refusal),
    /// A `NO` saying the server is busy for the folder the command was
    /// in ([`BusyScope::Folder`]), with its text.
    FolderBusy(String),
}

impl Done {
    /// A folder's busy answer as the folder's error; the rest as it is.
    pub(super) fn in_folder(self) -> FolderResult<Self> {
        match self {
            Self::FolderBusy(text) => Err(FolderError::Busy(text)),
            done => Ok(done),
        }
    }
}

/// A tagged `NO` that is not the server busy (that stops the folder, or
/// the pass, to be tried again later): the server would not, or could
/// not, do all of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Refusal {
    /// The server's text, with its response code.
    pub text: String,
}

impl Conn {
    pub(super) fn new(session: ImapSession) -> Self {
        Self {
            session,
            qresync: false,
        }
    }

    pub(super) fn session_mut(&mut self) -> &mut ImapSession {
        &mut self.session
    }

    pub(super) fn into_session(self) -> ImapSession {
        self.session
    }

    /// Whether flags are read by MODSEQ (CONDSTORE or QRESYNC offered).
    pub(super) fn condstore(&self) -> bool {
        let c = self.session.capabilities();
        c.condstore() || c.qresync()
    }

    /// Whether QRESYNC is enabled on this connection.
    pub(super) fn qresync(&self) -> bool {
        self.qresync
    }

    /// Whether the connection still answers (`NOOP`), before it is used
    /// again after waiting.
    pub(super) async fn alive(&mut self) -> bool {
        let noop = Command::plain("NOOP");
        self.command("checking the connection", &noop, |_| Ok(()))
            .await
            .is_ok()
    }

    /// `ENABLE QRESYNC` once per connection, where it is offered.
    pub(super) async fn enable_qresync(&mut self) -> SyncResult<()> {
        let capabilities = self.session.capabilities();
        if self.qresync || !capabilities.qresync() || !capabilities.enable() {
            return Ok(());
        }
        let enable = Command::plain("ENABLE QRESYNC");
        let done = self
            .command("enabling QRESYNC", &enable, |_| Ok(()))
            .await?;
        match done {
            Done::Ok(_) => self.qresync = true,
            // A server refusing it is read the CONDSTORE way.
            Done::No(_) => {}
            // ENABLE is about the connection, not a folder.
            Done::FolderBusy(message) => {
                return Err(SyncError::Busy {
                    service: "IMAP",
                    server: self.describe(),
                    message,
                });
            }
        }
        Ok(())
    }

    /// `SELECT`s a folder by its name on the server, with `(CONDSTORE)`
    /// where offered. `Ok(None)` when the server refuses it (it was
    /// deleted since it was listed, or cannot be opened now), or when its
    /// name cannot be sent back: the pass then counts the folder unread,
    /// as it does a folder the server is busy for ([`FolderError::Busy`]).
    pub(super) async fn select(&mut self, name: &str) -> FolderResult<Option<Selected>> {
        self.open("SELECT", name).await
    }

    /// `EXAMINE`s a folder (a read-only `SELECT`), as [`Self::select`].
    pub(super) async fn examine(&mut self, name: &str) -> FolderResult<Option<Selected>> {
        self.open("EXAMINE", name).await
    }

    async fn open(&mut self, verb: &str, name: &str) -> FolderResult<Option<Selected>> {
        let Some(mut command) = Command::with_mailbox(verb, name) else {
            tracing::warn!(
                "IMAP: {} lists a folder whose name cannot be sent back: {name:?}",
                self.describe()
            );
            return Ok(None);
        };
        if self.condstore() {
            command = command.then(" (CONDSTORE)");
        }
        let (mut uidvalidity, mut uidnext, mut highest_modseq, mut exists) = (None, None, None, 0);
        let mut mailbox_id = None;
        let done = self
            .command("opening a folder", &command, |response| {
                match response {
                    Response::Data { outcome, .. } => match outcome.code {
                        Some(ResponseCode::UidValidity(v)) => uidvalidity = Some(v),
                        Some(ResponseCode::UidNext(n)) => uidnext = Some(n),
                        Some(ResponseCode::HighestModSeq(m)) => highest_modseq = Some(m),
                        // imap-proto does not know RFC 8474's code: it is
                        // left at the start of the text.
                        None => {
                            let text = outcome.information.as_deref();
                            if let Some(id) = text.and_then(mailbox_id_of) {
                                mailbox_id = Some(id);
                            }
                        }
                        _ => {}
                    },
                    Response::MailboxData(MailboxDatum::Exists(n)) => exists = *n,
                    _ => {}
                }
                Ok(())
            })
            .await?
            .in_folder()?;
        if let Done::No(refusal) = done {
            tracing::info!(
                "IMAP: {} refused to open the folder {name:?}: {}",
                self.describe(),
                refusal.text
            );
            return Ok(None);
        }
        let uidvalidity = uidvalidity.ok_or_else(|| self.protocol("SELECT gave no UIDVALIDITY"))?;
        let objectid = self.session.capabilities().has("OBJECTID");
        Ok(Some(Selected {
            uidvalidity,
            uidnext,
            highest_modseq: highest_modseq.filter(|_| self.condstore()),
            exists,
            mailbox_id: mailbox_id.filter(|_| objectid),
        }))
    }

    /// `UID FETCH <set> <items> [modifiers]` in the selected folder: the
    /// messages of `set` the server answered, each once, however many
    /// responses it took ([`Collector`]). A `NO` is not an error: the
    /// reply says so ([`FetchReply::refused`]) and carries what did come.
    /// A server busy for the folder is [`FolderError::Busy`], and what
    /// came is not used.
    pub(super) async fn uid_fetch(
        &mut self,
        set: &str,
        items: &str,
        modifiers: Option<&str>,
    ) -> FolderResult<FetchReply> {
        let mut command = format!("UID FETCH {set} {items}");
        if let Some(modifiers) = modifiers {
            command.push_str(&format!(" ({modifiers})"));
        }
        let mut collector = Collector::new(set);
        let done = self
            .command("reading messages", &Command::plain(command), |response| {
                match response {
                    Response::Fetch(seq, attributes) => collector.fetch(*seq, attributes),
                    Response::Expunge(seq) => collector.expunge(*seq),
                    Response::Vanished { earlier, uids } => {
                        collector.vanished(*earlier, uids.iter().cloned());
                    }
                    _ => {}
                }
                Ok(())
            })
            .await?
            .in_folder()?;
        let mut reply = collector.finish();
        if let Done::No(refusal) = done {
            tracing::info!(
                "IMAP: {} did not answer all of a UID FETCH: {}",
                self.describe(),
                refusal.text
            );
            reply.refused = Some(refusal);
        }
        Ok(reply)
    }

    /// `UID SEARCH <criteria>` in the selected folder, the UIDs in order;
    /// `None` when the server refuses it.
    pub(super) async fn uid_search(&mut self, criteria: &str) -> FolderResult<Option<Vec<u32>>> {
        let mut found = Vec::new();
        let search = Command::plain(format!("UID SEARCH {criteria}"));
        let done = self
            .command("listing messages", &search, |response| {
                if let Response::MailboxData(MailboxDatum::Search(uids)) = response {
                    found.extend_from_slice(uids);
                }
                Ok(())
            })
            .await?
            .in_folder()?;
        if let Done::No(refusal) = done {
            tracing::info!(
                "IMAP: {} refused a UID SEARCH: {}",
                self.describe(),
                refusal.text
            );
            return Ok(None);
        }
        found.sort_unstable();
        found.dedup();
        Ok(Some(found))
    }

    /// Sends a command and reads every response up to its tagged one,
    /// handing each untagged one to `each`. A literal the server must
    /// agree to first (without LITERAL+) is sent after its `+`; a literal
    /// may take longer to send than a command ([`for_bytes`]).
    pub(super) async fn command(
        &mut self,
        what: &str,
        command: &Command,
        mut each: impl FnMut(&Response<'_>) -> SyncResult<()>,
    ) -> SyncResult<Done> {
        let server = self.session.server().clone();
        let limit = self.session.connector().timeouts().command;
        let capabilities = self.session.capabilities();
        let wire = command.wire(capabilities.has("LITERAL+"), capabilities.has("LITERAL-"));
        let verb = command.verb().to_owned();
        let session = self.session.inner();
        let tag = within(limit, &server, what, async {
            session
                .run_command(&wire.first)
                .await
                .map_err(|e| imap_error(&server, e))
        })
        .await?;
        let mut rest = wire.rest.into_iter().peekable();
        // Lines after a non-synchronizing literal go at once.
        while let Some(line) = rest.next_if(|l| !l.wait) {
            write_line(session, &server, limit, what, &line.bytes).await?;
        }
        loop {
            let read = within(limit, &server, what, async {
                Ok(session.read_response().await?)
            })
            .await?;
            let Some(response) = read else {
                return Err(SyncError::Unreachable {
                    server: describe(&server),
                    detail: format!("the connection closed while {what}"),
                });
            };
            match response.parsed() {
                Response::Done {
                    tag: done,
                    status,
                    outcome,
                } if *done == tag => {
                    let text = outcome.information.as_deref().unwrap_or("").to_owned();
                    return match status {
                        Status::Ok => Ok(Done::Ok(Completed::of(outcome.code.as_ref()))),
                        Status::No => match busy(outcome.code.as_ref(), &text) {
                            Some(BusyScope::Account) => Err(SyncError::Busy {
                                service: "IMAP",
                                server: describe(&server),
                                message: text,
                            }),
                            Some(BusyScope::Folder) => Ok(Done::FolderBusy(text)),
                            None => Ok(Done::No(Refusal { text })),
                        },
                        _ => Err(server_error(&server, &format!("{verb} refused: {text}"))),
                    };
                }
                Response::Data {
                    status: Status::Bye,
                    outcome,
                } => {
                    return Err(SyncError::Unreachable {
                        server: describe(&server),
                        detail: format!(
                            "the server closed the connection while {what}: {}",
                            outcome.information.as_deref().unwrap_or("")
                        ),
                    });
                }
                Response::Continue { .. } => {
                    let Some(line) = rest.next() else {
                        return Err(server_error(
                            &server,
                            &format!("{verb}: the server asked for more than was sent"),
                        ));
                    };
                    write_line(session, &server, limit, what, &line.bytes).await?;
                    while let Some(line) = rest.next_if(|l| !l.wait) {
                        write_line(session, &server, limit, what, &line.bytes).await?;
                    }
                }
                other => each(other)?,
            }
        }
    }

    fn protocol(&self, message: &str) -> SyncError {
        server_error(self.session.server(), message)
    }

    pub(super) fn describe(&self) -> String {
        describe(self.session.server())
    }
}

/// Writes one line of a command after its first (a literal and what
/// follows it) and its CRLF, straight to the connection: async-imap sends
/// only text, and a literal may be any bytes.
async fn write_line(
    session: &mut async_imap::Session<ImapStream>,
    server: &Server,
    limit: Duration,
    what: &str,
    bytes: &[u8],
) -> SyncResult<()> {
    let stream = session.get_mut();
    within(for_bytes(limit, bytes.len()), server, what, async {
        stream.write_all(bytes).await?;
        stream.write_all(b"\r\n").await?;
        stream.flush().await?;
        Ok(())
    })
    .await
}

/// The time allowed to send `len` bytes: the command timeout, and a
/// second more for every 64 KiB, so a large message on a slow link is not
/// cut off while it is still going (25 MB takes up to about 7 minutes).
pub(crate) fn for_bytes(limit: Duration, len: usize) -> Duration {
    limit + Duration::from_secs((len / 65_536) as u64)
}

fn server_error(server: &Server, message: &str) -> SyncError {
    SyncError::Server {
        service: "IMAP",
        server: describe(server),
        message: message.to_owned(),
    }
}

/// The id in an untagged `OK [MAILBOXID (<id>)]`'s text.
fn mailbox_id_of(text: &str) -> Option<String> {
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
    fn mailbox_ids_are_read_from_the_text() {
        let id = mailbox_id_of("[MAILBOXID (F2212ea87)] Ok");
        assert_eq!(id.as_deref(), Some("F2212ea87"));
        assert_eq!(mailbox_id_of("[mailboxid (x)]").as_deref(), Some("x"));
        assert_eq!(mailbox_id_of("[MAILBOXID ()] Ok"), None);
        assert_eq!(mailbox_id_of("Ok"), None);
    }
}
