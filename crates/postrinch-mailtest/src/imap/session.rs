//! One client connection: its state, the command loop, and dispatch.
//!
//! Each command runs as: log it, apply any programmed failure, run it
//! (handlers write untagged responses to [`Session::out`] and return the
//! tagged [`Reply`]), tell the client what other sessions changed in its
//! folder, then send everything.

use std::io;
use std::sync::Arc;

use tokio::sync::watch;

use super::parser::Command;
use super::reader::{self, Read};
use super::response::Args;
use super::select::Selected;
use super::state::{ImapFault, LoggedCommand, Shared};
use crate::conn::Conn;

/// The tagged answer to a command. The text may start with a response
/// code (`[TRYCREATE] ...`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reply {
    Ok(String),
    No(String),
    Bad(String),
}

impl Reply {
    fn line(&self, tag: &str) -> String {
        match self {
            Reply::Ok(t) => format!("{tag} OK {t}\r\n"),
            Reply::No(t) => format!("{tag} NO {t}\r\n"),
            Reply::Bad(t) => format!("{tag} BAD {t}\r\n"),
        }
    }
}

impl From<String> for Reply {
    /// An argument error is a BAD.
    fn from(text: String) -> Self {
        Reply::Bad(text)
    }
}

/// What the session does after a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Next {
    Continue,
    /// Send the reply, then switch the connection to TLS (STARTTLS).
    StartTls,
    /// Send the reply, then close the connection (LOGOUT).
    Close,
    /// Close the connection without sending anything more.
    Drop,
}

pub(crate) struct Session {
    pub(crate) shared: Arc<Shared>,
    pub(crate) conn: Conn,
    /// Untagged responses waiting to be sent.
    pub(crate) out: Vec<u8>,
    /// The signed-in user.
    pub(crate) user: Option<String>,
    pub(crate) selected: Option<Selected>,
    /// CONDSTORE enabled (by ENABLE or by first use, RFC 7162 3.1).
    pub(crate) condstore: bool,
    /// QRESYNC enabled (only by ENABLE).
    pub(crate) qresync: bool,
    pub(crate) changes: watch::Receiver<u64>,
    kill_epoch: u64,
}

/// Commands that must not be answered with EXPUNGE, because the client's
/// sequence numbers would shift under it (RFC 9051 7.5.1).
const NO_EXPUNGE_DURING: [&str; 3] = ["FETCH", "STORE", "SEARCH"];

/// What [`ImapConfig::injects_after_starttls`] sends after STARTTLS's OK.
///
/// [`ImapConfig::injects_after_starttls`]: super::ImapConfig::injects_after_starttls
const INJECTED: &[u8] = b"* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN] injected\r\n";

impl Session {
    pub(crate) fn new(shared: Arc<Shared>, conn: Conn) -> Self {
        let changes = shared.subscribe();
        let kill_epoch = shared.lock().kill_epoch;
        Self {
            shared,
            conn,
            out: Vec::new(),
            user: None,
            selected: None,
            condstore: false,
            qresync: false,
            changes,
            kill_epoch,
        }
    }

    /// Serves the connection until the client leaves or is dropped.
    pub(crate) async fn run(mut self) {
        // An I/O error ends the session as a closed connection would; the
        // test sees it from the client side.
        let _ = self.serve().await;
        self.sign_out();
        self.conn.close().await;
    }

    async fn serve(&mut self) -> io::Result<()> {
        if !self.shared.lock().config.greets {
            // A hung server: read whatever comes, answer nothing.
            while self.conn.fill().await? {}
            return Ok(());
        }
        let bye = self.shared.lock().config.bye_greeting.clone();
        if let Some(text) = bye {
            // Turned away before any command.
            return self
                .conn
                .write_all(format!("* BYE {text}\r\n").as_bytes())
                .await;
        }
        let greeting = format!(
            "* OK [CAPABILITY {}] Fake IMAP ready\r\n",
            self.capabilities()
        );
        self.conn.write_all(greeting.as_bytes()).await?;
        loop {
            if !self.wait_for_input().await? {
                return Ok(());
            }
            let (command, log) = match reader::read_command(&mut self.conn).await? {
                Read::Eof => return Ok(()),
                Read::Invalid { tag, reason } => {
                    let tag = tag.unwrap_or_else(|| "*".to_string());
                    let line = Reply::Bad(reason).line(&tag);
                    self.conn.write_all(line.as_bytes()).await?;
                    continue;
                }
                Read::Command(command, log) => (command, log),
            };
            if !self.handle(command, log).await? {
                return Ok(());
            }
        }
    }

    /// Waits until a whole line has arrived. `false` when the client has
    /// gone or the test dropped every session.
    async fn wait_for_input(&mut self) -> io::Result<bool> {
        loop {
            if self.killed() {
                return Ok(false);
            }
            if self.conn.has_line() {
                return Ok(true);
            }
            tokio::select! {
                more = self.conn.fill() => if !more? { return Ok(false) },
                changed = self.changes.changed() => if changed.is_err() { return Ok(false) },
            }
        }
    }

    /// Whether the test asked to drop every session since this one began.
    pub(crate) fn killed(&self) -> bool {
        self.shared.lock().kill_epoch != self.kill_epoch
    }

    /// Runs one command. `false` when the session must end.
    async fn handle(&mut self, command: Command, log: String) -> io::Result<bool> {
        let (fault, hook, hold) = {
            let mut state = self.shared.lock();
            let text = if command.name == "LOGIN" {
                "LOGIN <credentials>".to_string()
            } else {
                log
            };
            let fault = state.take_fault(&command.name, &text);
            let hook = state.take_hook(&command.name, &text);
            let hold = state.take_hold(&command.name, &text);
            state.log.push(LoggedCommand {
                name: command.name.clone(),
                text,
            });
            (fault, hook, hold)
        };
        // Let go, or dropped with the test's handle: either way, on.
        if let Some(gate) = hold {
            let _ = gate.await;
        }
        if let Some(action) = hook {
            action();
        }
        match fault {
            Some(ImapFault::DisconnectBefore) => return Ok(false),
            Some(ImapFault::No(text)) => return self.finish(&command, Reply::No(text)).await,
            Some(ImapFault::Bad(text)) => return self.finish(&command, Reply::Bad(text)).await,
            Some(ImapFault::Ok(text)) => return self.finish(&command, Reply::Ok(text)).await,
            Some(ImapFault::DisconnectAfter) | None => {}
        }
        let (reply, next) = self.dispatch(&command).await?;
        if fault == Some(ImapFault::DisconnectAfter) {
            return Ok(false);
        }
        match next {
            Next::Continue => self.finish(&command, reply).await,
            Next::Drop => Ok(false),
            Next::Close => {
                self.flush(&command, reply).await?;
                Ok(false)
            }
            Next::StartTls => {
                if self.shared.lock().config.injects_after_starttls {
                    // Sent with the OK, in one write, so the client has
                    // both in hand before its TLS handshake.
                    let mut out = std::mem::take(&mut self.out);
                    out.extend_from_slice(reply.line(&command.tag).as_bytes());
                    out.extend_from_slice(INJECTED);
                    self.conn.write_all(&out).await?;
                } else {
                    self.flush(&command, reply).await?;
                }
                let acceptor = self.shared.acceptor.clone();
                self.conn.start_tls(&acceptor).await?;
                Ok(true)
            }
        }
    }

    /// Sends what is pending after a command, with news of the selected
    /// folder, then the tagged reply.
    async fn finish(&mut self, command: &Command, reply: Reply) -> io::Result<bool> {
        let allow_expunge = !NO_EXPUNGE_DURING.contains(&command.name.as_str());
        if !self.sync_selected(allow_expunge) {
            self.out
                .extend_from_slice(b"* BYE The selected mailbox is gone\r\n");
            self.send_pending().await?;
            return Ok(false);
        }
        self.flush(command, reply).await?;
        Ok(true)
    }

    async fn flush(&mut self, command: &Command, reply: Reply) -> io::Result<()> {
        let mut out = std::mem::take(&mut self.out);
        out.extend_from_slice(reply.line(&command.tag).as_bytes());
        self.conn.write_all(&out).await
    }

    /// Writes whatever is pending without a tagged reply (IDLE updates,
    /// continuation requests).
    pub(crate) async fn send_pending(&mut self) -> io::Result<()> {
        let out = std::mem::take(&mut self.out);
        if out.is_empty() {
            return Ok(());
        }
        self.conn.write_all(&out).await
    }

    async fn dispatch(&mut self, command: &Command) -> io::Result<(Reply, Next)> {
        let args = Args::new(command.args.clone());
        let name = command.name.as_str();
        // Commands that talk to the client more than once, or end the session.
        match name {
            "LOGOUT" => {
                // The session is over before it says so, as a server's
                // is: a client that has seen the answer can sign in again
                // in its place.
                self.sign_out();
                self.out.extend_from_slice(b"* BYE Logging out\r\n");
                return Ok((Reply::Ok("LOGOUT completed".into()), Next::Close));
            }
            "STARTTLS" if self.conn.is_tls() => {
                return Ok((Reply::Bad("Already encrypted".into()), Next::Continue));
            }
            "STARTTLS" if !self.offers_starttls() => {
                return Ok((
                    Reply::Bad("Unknown command STARTTLS".into()),
                    Next::Continue,
                ));
            }
            "STARTTLS" => {
                return Ok(match args.end() {
                    Ok(()) => (
                        Reply::Ok("Begin TLS negotiation now".into()),
                        Next::StartTls,
                    ),
                    Err(e) => (Reply::Bad(e), Next::Continue),
                });
            }
            "AUTHENTICATE" => return Ok((self.authenticate(args).await?, Next::Continue)),
            "IDLE" => return self.idle(args).await,
            _ => {}
        }
        let reply = match (name, self.user.is_some(), self.selected.is_some()) {
            ("CAPABILITY", _, _) => self.capability(args),
            ("NOOP" | "CHECK", true, _) | ("NOOP", false, _) => {
                Reply::Ok(format!("{name} completed"))
            }
            ("LOGIN", false, _) => self.login(args),
            ("LOGIN", true, _) => Reply::Bad("Already logged in".into()),
            (_, false, _) => Reply::Bad(format!("{name} needs a signed-in session")),
            ("ENABLE", _, _) => self.enable(args),
            ("SELECT", _, _) => self.select(args, false),
            ("EXAMINE", _, _) => self.select(args, true),
            ("LIST", _, _) => self.list(args, false),
            ("LSUB", _, _) => self.list(args, true),
            ("CREATE", _, _) => self.create(args),
            ("DELETE", _, _) => self.delete(args),
            ("RENAME", _, _) => self.rename(args),
            ("SUBSCRIBE", _, _) => self.subscribe(args, true),
            ("UNSUBSCRIBE", _, _) => self.subscribe(args, false),
            ("STATUS", _, _) => self.status(args),
            ("APPEND", _, _) => self.append(args),
            (_, _, false) if is_selected_command(name) => {
                Reply::Bad(format!("{name} needs a selected mailbox"))
            }
            ("FETCH", _, _) => self.fetch(args, false),
            ("UID FETCH", _, _) => self.fetch(args, true),
            ("SEARCH", _, _) => self.search(args, false),
            ("UID SEARCH", _, _) => self.search(args, true),
            ("STORE", _, _) => self.store(args, false),
            ("UID STORE", _, _) => self.store(args, true),
            ("COPY", _, _) => self.copy(args, false, false),
            ("UID COPY", _, _) => self.copy(args, true, false),
            ("MOVE", _, _) => self.copy(args, false, true),
            ("UID MOVE", _, _) => self.copy(args, true, true),
            ("EXPUNGE", _, _) => self.expunge(args, false),
            ("UID EXPUNGE", _, _) => self.expunge(args, true),
            ("CLOSE", _, _) => self.close(args, true),
            ("UNSELECT", _, _) => self.close(args, false),
            _ => Reply::Bad(format!("Unknown command {name}")),
        };
        Ok((reply, Next::Continue))
    }
}

fn is_selected_command(name: &str) -> bool {
    matches!(
        name,
        "FETCH"
            | "UID FETCH"
            | "SEARCH"
            | "UID SEARCH"
            | "STORE"
            | "UID STORE"
            | "COPY"
            | "UID COPY"
            | "MOVE"
            | "UID MOVE"
            | "EXPUNGE"
            | "UID EXPUNGE"
            | "CLOSE"
            | "UNSELECT"
    )
}
