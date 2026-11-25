//! The fake IMAP server.
//!
//! [`ImapServer::start`] listens on loopback ports (implicit TLS, and
//! optionally plain text with STARTTLS) and serves each connection from
//! the shared in-memory state. The test drives the mail itself through
//! [`ImapServer::user`] (a [`UserHandle`]), programs failures with
//! [`ImapServer::fail_next`], and checks what the client did with
//! [`ImapServer::commands`] and [`ImapServer::command_count`].

mod auth;
mod config;
mod fetch;
mod idle;
mod list;
mod mailbox;
mod mailbox_cmds;
mod message;
mod message_cmds;
mod parser;
mod reader;
mod response;
mod search;
mod select;
mod seqset;
mod session;
mod state;
mod user;

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub use config::{Extension, ImapConfig};
pub use mailbox::{DELIMITER, SpecialUse};
pub use state::ImapFault;
pub use user::{FolderInfo, MessageInfo, NewMessage, UserHandle};

use crate::Error;
use crate::listener;
use crate::tls::TestCertificate;
use session::Session;
use state::{Shared, State};

/// A command left unanswered ([`ImapServer::hold_next`]) until this is
/// let go or dropped.
#[derive(Debug)]
pub struct HeldCommand(oneshot::Sender<()>);

impl HeldCommand {
    /// Lets the command run and be answered.
    pub fn let_go(self) {
        // The session may be gone already; then there is nothing to do.
        let _ = self.0.send(());
    }
}

/// A running fake IMAP server. Dropping it stops the server and every
/// session on it.
pub struct ImapServer {
    shared: Arc<Shared>,
    certificate: TestCertificate,
    tls_addr: Option<SocketAddr>,
    starttls_addr: Option<SocketAddr>,
    listeners: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for ImapServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImapServer")
            .field("tls_addr", &self.tls_addr)
            .field("starttls_addr", &self.starttls_addr)
            .finish_non_exhaustive()
    }
}

impl ImapServer {
    /// Starts a server with a fresh self-signed certificate. Needs a tokio
    /// runtime.
    pub async fn start(config: ImapConfig) -> Result<Self, Error> {
        let certificate = TestCertificate::generate()?;
        let (implicit, starttls) = (config.implicit_tls, config.starttls);
        let shared = Arc::new(Shared::new(State::new(config), certificate.acceptor()?));
        let mut server = Self {
            shared,
            certificate,
            tls_addr: None,
            starttls_addr: None,
            listeners: Vec::new(),
        };
        if implicit {
            server.tls_addr = Some(server.listen(true).await?);
        }
        if starttls {
            server.starttls_addr = Some(server.listen(false).await?);
        }
        Ok(server)
    }

    async fn listen(&mut self, tls: bool) -> Result<SocketAddr, Error> {
        let shared = self.shared.clone();
        let (addr, task) = listener::listen(self.shared.acceptor.clone(), tls, move |conn| {
            Session::new(shared.clone(), conn).run()
        })
        .await?;
        self.listeners.push(task);
        Ok(addr)
    }

    /// The implicit-TLS address (993-style), if it listens on one.
    pub fn tls_addr(&self) -> Option<SocketAddr> {
        self.tls_addr
    }

    /// The plain-text STARTTLS address (143-style), if it listens on one.
    pub fn starttls_addr(&self) -> Option<SocketAddr> {
        self.starttls_addr
    }

    /// The certificate the server presents, for the client to trust.
    pub fn certificate(&self) -> &TestCertificate {
        &self.certificate
    }

    /// The test's access to one user's mail. The user must have been
    /// configured; the handle's methods fail with [`Error::UnknownUser`]
    /// otherwise.
    pub fn user(&self, name: &str) -> UserHandle {
        UserHandle::new(self.shared.clone(), name)
    }

    /// Makes the next `times` commands named `command` fail as `fault`
    /// says. Names are as the client sends them, upper case, with `UID `
    /// before UID commands: `"SELECT"`, `"UID FETCH"`, `"UID MOVE"`.
    pub fn fail_next(&self, command: &str, times: usize, fault: ImapFault) {
        self.shared.lock().add_fault(command, None, times, fault);
    }

    /// As [`ImapServer::fail_next`], for commands whose text as
    /// [`ImapServer::commands`] shows it contains `matching`: for
    /// example `fail_next_matching("SELECT", "\"Work\"", 1, ...)` fails
    /// the next SELECT of Work only.
    pub fn fail_next_matching(
        &self,
        command: &str,
        matching: &str,
        times: usize,
        fault: ImapFault,
    ) {
        self.shared
            .lock()
            .add_fault(command, Some(matching), times, fault);
    }

    /// Runs `action` once, when the next command named `command` (as for
    /// [`ImapServer::fail_next`]) whose text contains `matching` arrives,
    /// before the command runs: for changing the mail in the middle of a
    /// client's work, as another client would. The action may use a
    /// [`UserHandle`].
    pub fn on_next(&self, command: &str, matching: &str, action: impl FnOnce() + Send + 'static) {
        self.shared
            .lock()
            .add_hook(command, matching, Box::new(action));
    }

    /// Leaves the next command named `command` (as for
    /// [`ImapServer::fail_next`]) whose text contains `matching`
    /// unanswered, as a slow server would, until the returned
    /// [`HeldCommand`] is let go or dropped: for acting while a client
    /// waits on it. It is logged ([`ImapServer::commands`]) as it
    /// arrives; then it runs as any other.
    pub fn hold_next(&self, command: &str, matching: &str) -> HeldCommand {
        let (gate, held) = oneshot::channel();
        self.shared.lock().add_hold(command, matching, held);
        HeldCommand(gate)
    }

    /// Closes every open connection at once, as a server restart would.
    pub fn disconnect_all(&self) {
        self.shared.lock().kill_epoch += 1;
        self.shared.notify();
    }

    /// Ends every IDLE under way now with `* BYE <text>` and closes its
    /// connection, as a server going down, or one turning the account
    /// away, would. Sessions not idling are left alone.
    pub fn bye_idling(&self, text: &str) {
        let mut state = self.shared.lock();
        state.idle_bye = (state.idle_bye.0 + 1, text.to_string());
        drop(state);
        self.shared.notify();
    }

    /// From now on, lets each user have at most `max` sessions signed in,
    /// as [`ImapConfig::max_connections`] does from the start. Sessions
    /// already signed in stay; only the next sign-ins are counted against
    /// it.
    pub fn set_max_connections(&self, max: usize) {
        self.shared.lock().config.max_connections = Some(max);
    }

    /// How many sessions are in an IDLE now.
    pub fn idling(&self) -> usize {
        self.shared.lock().idling
    }

    /// Every command received so far, in order, without tags, literals
    /// shown as `{n}` and LOGIN's credentials hidden: for example
    /// `UID FETCH 1:* (FLAGS) (CHANGEDSINCE 4)`.
    pub fn commands(&self) -> Vec<String> {
        self.shared
            .lock()
            .log
            .iter()
            .map(|c| c.text.clone())
            .collect()
    }

    /// How many commands named `command` (as for [`ImapServer::fail_next`])
    /// were received.
    pub fn command_count(&self, command: &str) -> usize {
        let command = command.to_ascii_uppercase();
        self.shared
            .lock()
            .log
            .iter()
            .filter(|c| c.name == command)
            .count()
    }
}

impl Drop for ImapServer {
    fn drop(&mut self) {
        for listener in &self.listeners {
            listener.abort();
        }
    }
}
