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

use async_imap::imap_proto::{
    AttributeValue, MailboxDatum, MessageSection, Response, ResponseCode, SectionPath, Status,
};
use postrinch_model::Server;

use crate::imap::connect::{describe, within};
use crate::imap::session::{ImapSession, imap_error};
use crate::{SyncError, SyncResult};

/// A signed-in session as sync uses it, and whether QRESYNC is enabled.
pub(super) struct Conn {
    session: ImapSession,
    qresync: bool,
}

/// What `SELECT` said about a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Selected {
    pub uidvalidity: u32,
    pub uidnext: Option<u32>,
    /// `None` when the server keeps no MODSEQs for the folder.
    pub highest_modseq: Option<u64>,
    pub exists: u32,
}

/// One message's attributes from a `FETCH` response.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Fetched {
    pub uid: u32,
    pub flags: Option<Vec<String>>,
    pub modseq: Option<u64>,
    pub size: Option<u32>,
    pub internal_date: Option<String>,
    pub header: Option<Vec<u8>>,
    pub body: Option<Vec<u8>>,
}

/// What a `UID FETCH` brought: the messages, and the UIDs the server said
/// had vanished (QRESYNC).
#[derive(Debug, Default)]
pub(super) struct FetchReply {
    pub messages: Vec<Fetched>,
    pub vanished: Vec<u32>,
}

/// How a command ended, short of an error.
enum Done {
    Ok,
    /// `NO`, with the server's text.
    No(String),
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
        self.command("checking the connection", "NOOP", |_| Ok(()))
            .await
            .is_ok()
    }

    /// `ENABLE QRESYNC` once per connection, where it is offered.
    pub(super) async fn enable_qresync(&mut self) -> SyncResult<()> {
        let capabilities = self.session.capabilities();
        if self.qresync || !capabilities.qresync() || !capabilities.enable() {
            return Ok(());
        }
        let done = self
            .command("enabling QRESYNC", "ENABLE QRESYNC", |_| Ok(()))
            .await?;
        // A server refusing it is read the CONDSTORE way.
        self.qresync = matches!(done, Done::Ok);
        Ok(())
    }

    /// `SELECT`s a folder by its name on the server, with `(CONDSTORE)`
    /// where offered. `Ok(None)` when the server refuses it (it was
    /// deleted since it was listed, or cannot be opened now): the folder
    /// is left for the next pass.
    pub(super) async fn select(&mut self, name: &str) -> SyncResult<Option<Selected>> {
        let mut command = format!("SELECT {}", quote(&self.session, name)?);
        if self.condstore() {
            command.push_str(" (CONDSTORE)");
        }
        let (mut uidvalidity, mut uidnext, mut highest_modseq, mut exists) = (None, None, None, 0);
        let done = self
            .command("opening a folder", &command, |response| {
                match response {
                    Response::Data { outcome, .. } => match outcome.code {
                        Some(ResponseCode::UidValidity(v)) => uidvalidity = Some(v),
                        Some(ResponseCode::UidNext(n)) => uidnext = Some(n),
                        Some(ResponseCode::HighestModSeq(m)) => highest_modseq = Some(m),
                        _ => {}
                    },
                    Response::MailboxData(MailboxDatum::Exists(n)) => exists = *n,
                    _ => {}
                }
                Ok(())
            })
            .await?;
        if let Done::No(text) = done {
            tracing::info!("IMAP: {} refused to open a folder: {text}", self.describe());
            return Ok(None);
        }
        let uidvalidity = uidvalidity.ok_or_else(|| self.protocol("SELECT gave no UIDVALIDITY"))?;
        Ok(Some(Selected {
            uidvalidity,
            uidnext,
            highest_modseq: highest_modseq.filter(|_| self.condstore()),
            exists,
        }))
    }

    /// `UID FETCH <set> <items> [modifiers]` in the selected folder.
    pub(super) async fn uid_fetch(
        &mut self,
        set: &str,
        items: &str,
        modifiers: Option<&str>,
    ) -> SyncResult<FetchReply> {
        let mut command = format!("UID FETCH {set} {items}");
        if let Some(modifiers) = modifiers {
            command.push_str(&format!(" ({modifiers})"));
        }
        let mut reply = FetchReply::default();
        let done = self
            .command("reading messages", &command, |response| {
                match response {
                    Response::Fetch(_, attributes) => {
                        if let Some(fetched) = fetched(attributes) {
                            reply.messages.push(fetched);
                        }
                    }
                    Response::Vanished { uids, .. } => {
                        reply.vanished.extend(uids.iter().flat_map(|r| r.clone()));
                    }
                    _ => {}
                }
                Ok(())
            })
            .await?;
        self.refused(done, "UID FETCH")?;
        Ok(reply)
    }

    /// `UID SEARCH <criteria>` in the selected folder.
    pub(super) async fn uid_search(&mut self, criteria: &str) -> SyncResult<Vec<u32>> {
        let mut found = Vec::new();
        let done = self
            .command(
                "listing messages",
                &format!("UID SEARCH {criteria}"),
                |response| {
                    if let Response::MailboxData(MailboxDatum::Search(uids)) = response {
                        found.extend_from_slice(uids);
                    }
                    Ok(())
                },
            )
            .await?;
        self.refused(done, "UID SEARCH")?;
        found.sort_unstable();
        found.dedup();
        Ok(found)
    }

    /// Sends a command and reads every response up to its tagged one,
    /// handing each untagged one to `each`.
    async fn command(
        &mut self,
        what: &str,
        command: &str,
        mut each: impl FnMut(&Response<'_>) -> SyncResult<()>,
    ) -> SyncResult<Done> {
        let server = self.session.server().clone();
        let limit = self.session.connector().timeouts().command;
        let session = self.session.inner();
        let tag = within(limit, &server, what, async {
            session
                .run_command(command)
                .await
                .map_err(|e| imap_error(&server, e))
        })
        .await?;
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
                        Status::Ok => Ok(Done::Ok),
                        Status::No => Ok(Done::No(text)),
                        _ => Err(server_error(
                            &server,
                            &format!("{} refused: {text}", verb(command)),
                        )),
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
                other => each(other)?,
            }
        }
    }

    fn refused(&self, done: Done, command: &str) -> SyncResult<()> {
        match done {
            Done::Ok => Ok(()),
            Done::No(text) => Err(self.protocol(&format!("{command} refused: {text}"))),
        }
    }

    fn protocol(&self, message: &str) -> SyncError {
        server_error(self.session.server(), message)
    }

    fn describe(&self) -> String {
        describe(self.session.server())
    }
}

fn server_error(server: &Server, message: &str) -> SyncError {
    SyncError::Server {
        service: "IMAP",
        server: describe(server),
        message: message.to_owned(),
    }
}

/// A command's first word, to name it in an error without its arguments.
fn verb(command: &str) -> &str {
    let mut words = command.split(' ');
    match (words.next(), words.next()) {
        (Some("UID"), Some(second)) => &command[..4 + second.len()],
        (Some(first), _) => first,
        _ => command,
    }
}

/// A folder name as an IMAP quoted string. Names from LIST are 7-bit
/// (modified UTF-7), so quoting is enough; one that is not cannot be sent.
fn quote(session: &ImapSession, name: &str) -> SyncResult<String> {
    if !name.is_ascii() || name.contains(['\r', '\n', '\0']) {
        return Err(server_error(
            session.server(),
            &format!("the folder name {name:?} cannot be sent back to the server"),
        ));
    }
    Ok(format!(
        "\"{}\"",
        name.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// A FETCH response's attributes; `None` without a UID.
fn fetched(attributes: &[AttributeValue<'_>]) -> Option<Fetched> {
    let mut out = Fetched::default();
    let mut uid = None;
    for attribute in attributes {
        match attribute {
            AttributeValue::Uid(u) => uid = Some(*u),
            AttributeValue::Flags(flags) => {
                out.flags = Some(flags.iter().map(|f| f.to_string()).collect());
            }
            AttributeValue::ModSeq(m) => out.modseq = Some(*m),
            AttributeValue::Rfc822Size(s) => out.size = Some(*s),
            AttributeValue::InternalDate(d) => out.internal_date = Some(d.to_string()),
            AttributeValue::BodySection { section, data, .. } => {
                let data = data.as_ref().map(|d| d.to_vec());
                match section {
                    Some(SectionPath::Full(MessageSection::Header)) => out.header = data,
                    None => out.body = data,
                    _ => {}
                }
            }
            AttributeValue::Rfc822Header(data) => out.header = data.as_ref().map(|d| d.to_vec()),
            AttributeValue::Rfc822(data) => out.body = data.as_ref().map(|d| d.to_vec()),
            _ => {}
        }
    }
    out.uid = uid?;
    Some(out)
}

/// A UID set in IMAP's syntax, runs folded into ranges: `1:4,7,9:10`.
pub(super) fn uid_set(uids: &[u32]) -> String {
    let mut sorted = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut out = String::new();
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        let mut end = start;
        while i + 1 < sorted.len() && sorted[i + 1] == end + 1 {
            i += 1;
            end = sorted[i];
        }
        if !out.is_empty() {
            out.push(',');
        }
        if start == end {
            out.push_str(&start.to_string());
        } else {
            out.push_str(&format!("{start}:{end}"));
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_sets_fold_runs() {
        assert_eq!(uid_set(&[7, 1, 2, 3, 4, 9, 10, 2]), "1:4,7,9:10");
        assert_eq!(uid_set(&[5]), "5");
        assert_eq!(uid_set(&[]), "");
    }

    #[test]
    fn verbs_name_commands_without_arguments() {
        assert_eq!(verb("UID FETCH 1:* (FLAGS)"), "UID FETCH");
        assert_eq!(verb("SELECT \"INBOX\""), "SELECT");
        assert_eq!(verb("NOOP"), "NOOP");
    }
}
