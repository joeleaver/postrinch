//! The state every session of one fake server shares: the users' mail,
//! the failures a test has asked for, and the log of commands seen.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard};

use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;

use super::config::{Extension, ImapConfig};
use super::mailbox::Account;

/// What a programmed failure does to a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImapFault {
    /// Answer `NO` with this text (which may start with a response code
    /// such as `[UNAVAILABLE]`) without running the command.
    No(String),
    /// Answer `BAD` with this text without running the command.
    Bad(String),
    /// Close the connection as soon as the command arrives, without
    /// running it or answering.
    DisconnectBefore,
    /// Run the command, then close the connection without sending any of
    /// its responses: the client cannot know whether it took effect.
    DisconnectAfter,
}

#[derive(Debug)]
struct PendingFault {
    command: String,
    remaining: usize,
    fault: ImapFault,
}

/// One command as the server saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoggedCommand {
    pub(crate) name: String,
    pub(crate) text: String,
}

pub(crate) struct State {
    pub(crate) config: ImapConfig,
    pub(crate) accounts: HashMap<String, Account>,
    pub(crate) next_folder_id: u64,
    faults: Vec<PendingFault>,
    pub(crate) log: Vec<LoggedCommand>,
    /// Raised to drop every open session.
    pub(crate) kill_epoch: u64,
}

impl State {
    pub(crate) fn new(config: ImapConfig) -> Self {
        let mut state = Self {
            accounts: HashMap::new(),
            next_folder_id: 0,
            faults: Vec::new(),
            log: Vec::new(),
            kill_epoch: 0,
            config: config.clone(),
        };
        for user in &config.users {
            let mut account = Account {
                password: user.password.clone(),
                oauth_token: user.oauth_token.clone(),
                folders: BTreeMap::new(),
            };
            account
                .create("INBOX", &mut state.next_folder_id)
                .expect("a new account has no INBOX yet");
            state.accounts.insert(user.name.clone(), account);
        }
        state
    }

    pub(crate) fn has(&self, extension: Extension) -> bool {
        self.config.has(extension)
    }

    /// Adds a programmed failure for the next `times` commands named
    /// `command`.
    pub(crate) fn add_fault(&mut self, command: &str, times: usize, fault: ImapFault) {
        self.faults.push(PendingFault {
            command: command.to_ascii_uppercase(),
            remaining: times,
            fault,
        });
    }

    /// Takes the failure, if any, programmed for this command.
    pub(crate) fn take_fault(&mut self, command: &str) -> Option<ImapFault> {
        let i = self
            .faults
            .iter()
            .position(|f| f.command == command && f.remaining > 0)?;
        let pending = &mut self.faults[i];
        pending.remaining -= 1;
        let fault = pending.fault.clone();
        if pending.remaining == 0 {
            self.faults.remove(i);
        }
        Some(fault)
    }
}

/// What the sessions and the test's handle share.
pub(crate) struct Shared {
    state: Mutex<State>,
    changes: watch::Sender<u64>,
    pub(crate) acceptor: TlsAcceptor,
}

impl Shared {
    pub(crate) fn new(state: State, acceptor: TlsAcceptor) -> Self {
        Self {
            state: Mutex::new(state),
            changes: watch::Sender::new(0),
            acceptor,
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("a session panicked while changing the fake IMAP server's state")
    }

    /// Wakes every session waiting for changes (IDLE, and the check for
    /// [`State::kill_epoch`]).
    pub(crate) fn notify(&self) {
        self.changes.send_modify(|v| *v = v.wrapping_add(1));
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }
}
