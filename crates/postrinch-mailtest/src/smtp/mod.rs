//! The fake SMTP submission server.
//!
//! It listens with implicit TLS (the 465-style port) and in plain text
//! with STARTTLS (the 587-style port), signs users in with AUTH PLAIN,
//! LOGIN or XOAUTH2, and records every message it accepts for the test to
//! inspect. Failures can be programmed at each stage of a submission,
//! including the uncertain one: the data received and the connection
//! dropped without a reply.

mod session;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

use crate::Error;
use crate::listener;
use crate::tls::TestCertificate;

/// A point in a submission where a failure can be programmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpStage {
    /// The AUTH command.
    Auth,
    /// MAIL FROM.
    Mail,
    /// RCPT TO.
    Rcpt,
    /// The reply after the message data has been received.
    Data,
}

/// What a programmed failure does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmtpFault {
    /// Reply with this code (4xx or 5xx) and text instead of going on.
    Reply { code: u16, text: String },
    /// Close the connection without replying. At [`SmtpStage::Data`] this
    /// is the case a client cannot resolve: the server has the whole
    /// message but never said whether it took it.
    Disconnect,
}

/// A message the server accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedMessage {
    /// Who signed in to send it.
    pub auth_user: Option<String>,
    /// The envelope sender (MAIL FROM), without angle brackets.
    pub mail_from: String,
    /// The envelope recipients (RCPT TO), without angle brackets.
    pub rcpt_to: Vec<String>,
    /// The message as received, dot-unstuffed, lines ending in CRLF.
    pub data: Vec<u8>,
}

/// The fake SMTP server's setup. By default it listens on both ports,
/// accepts messages up to 35 MB, and has no users.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpConfig {
    implicit_tls: bool,
    starttls: bool,
    max_size: usize,
    users: Vec<SmtpUser>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SmtpUser {
    name: String,
    password: Option<String>,
    oauth_token: Option<String>,
}

impl Default for SmtpConfig {
    fn default() -> Self {
        Self {
            implicit_tls: true,
            starttls: true,
            max_size: 35 * 1024 * 1024,
            users: Vec::new(),
        }
    }
}

impl SmtpConfig {
    pub fn new() -> Self {
        Self::default()
    }

    fn entry(&mut self, name: &str) -> &mut SmtpUser {
        match self.users.iter().position(|u| u.name == name) {
            Some(i) => &mut self.users[i],
            None => {
                self.users.push(SmtpUser {
                    name: name.to_string(),
                    password: None,
                    oauth_token: None,
                });
                self.users.last_mut().expect("just pushed")
            }
        }
    }

    /// Lets `name` sign in with this password (AUTH PLAIN or LOGIN).
    pub fn user(mut self, name: &str, password: &str) -> Self {
        self.entry(name).password = Some(password.to_string());
        self
    }

    /// Lets `name` sign in with XOAUTH2 and this bearer token.
    pub fn oauth_token(mut self, name: &str, token: &str) -> Self {
        self.entry(name).oauth_token = Some(token.to_string());
        self
    }

    /// Whether to listen with implicit TLS (465-style). On by default.
    pub fn implicit_tls(mut self, on: bool) -> Self {
        self.implicit_tls = on;
        self
    }

    /// Whether to listen in plain text with STARTTLS (587-style). On by
    /// default. Before STARTTLS that port refuses AUTH.
    pub fn starttls(mut self, on: bool) -> Self {
        self.starttls = on;
        self
    }

    /// The largest message accepted (advertised as SIZE).
    pub fn max_size(mut self, bytes: usize) -> Self {
        self.max_size = bytes;
        self
    }
}

#[derive(Debug)]
struct PendingFault {
    stage: SmtpStage,
    remaining: usize,
    fault: SmtpFault,
}

pub(crate) struct SmtpState {
    config: SmtpConfig,
    faults: Vec<PendingFault>,
    messages: Vec<ReceivedMessage>,
    log: Vec<String>,
}

impl SmtpState {
    fn take_fault(&mut self, stage: SmtpStage) -> Option<SmtpFault> {
        let i = self
            .faults
            .iter()
            .position(|f| f.stage == stage && f.remaining > 0)?;
        let pending = &mut self.faults[i];
        pending.remaining -= 1;
        let fault = pending.fault.clone();
        if pending.remaining == 0 {
            self.faults.remove(i);
        }
        Some(fault)
    }

    fn password_ok(&self, user: &str, password: &str) -> bool {
        self.config
            .users
            .iter()
            .any(|u| u.name == user && u.password.as_deref() == Some(password))
    }

    fn token_ok(&self, user: &str, token: &str) -> bool {
        self.config
            .users
            .iter()
            .any(|u| u.name == user && u.oauth_token.as_deref() == Some(token))
    }
}

pub(crate) struct SmtpShared {
    state: Mutex<SmtpState>,
    acceptor: TlsAcceptor,
}

impl SmtpShared {
    fn lock(&self) -> MutexGuard<'_, SmtpState> {
        self.state
            .lock()
            .expect("a session panicked while changing the fake SMTP server's state")
    }
}

/// A running fake SMTP server. Dropping it stops the server and every
/// session on it.
pub struct SmtpServer {
    shared: Arc<SmtpShared>,
    certificate: TestCertificate,
    tls_addr: Option<SocketAddr>,
    starttls_addr: Option<SocketAddr>,
    listeners: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for SmtpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpServer")
            .field("tls_addr", &self.tls_addr)
            .field("starttls_addr", &self.starttls_addr)
            .finish_non_exhaustive()
    }
}

impl SmtpServer {
    /// Starts a server with a fresh self-signed certificate. Needs a tokio
    /// runtime.
    pub async fn start(config: SmtpConfig) -> Result<Self, Error> {
        let certificate = TestCertificate::generate()?;
        let (implicit, starttls) = (config.implicit_tls, config.starttls);
        let shared = Arc::new(SmtpShared {
            state: Mutex::new(SmtpState {
                config,
                faults: Vec::new(),
                messages: Vec::new(),
                log: Vec::new(),
            }),
            acceptor: certificate.acceptor()?,
        });
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
            session::run(shared.clone(), conn)
        })
        .await?;
        self.listeners.push(task);
        Ok(addr)
    }

    /// The implicit-TLS address (465-style), if it listens on one.
    pub fn tls_addr(&self) -> Option<SocketAddr> {
        self.tls_addr
    }

    /// The plain-text STARTTLS address (587-style), if it listens on one.
    pub fn starttls_addr(&self) -> Option<SocketAddr> {
        self.starttls_addr
    }

    /// The certificate the server presents, for the client to trust.
    pub fn certificate(&self) -> &TestCertificate {
        &self.certificate
    }

    /// Every message accepted so far, in order.
    pub fn messages(&self) -> Vec<ReceivedMessage> {
        self.shared.lock().messages.clone()
    }

    /// Makes the next `times` submissions reaching `stage` fail as `fault`
    /// says.
    pub fn fail_next(&self, stage: SmtpStage, times: usize, fault: SmtpFault) {
        self.shared.lock().faults.push(PendingFault {
            stage,
            remaining: times,
            fault,
        });
    }

    /// Every command received so far, in order: the verb and its
    /// arguments, except that AUTH shows only its mechanism.
    pub fn commands(&self) -> Vec<String> {
        self.shared.lock().log.clone()
    }

    /// How many commands with this verb (`"EHLO"`, `"AUTH"`, `"MAIL"`...)
    /// were received.
    pub fn command_count(&self, verb: &str) -> usize {
        self.shared
            .lock()
            .log
            .iter()
            .filter(|c| {
                c.split(' ')
                    .next()
                    .is_some_and(|v| v.eq_ignore_ascii_case(verb))
            })
            .count()
    }
}

impl Drop for SmtpServer {
    fn drop(&mut self) {
        for listener in &self.listeners {
            listener.abort();
        }
    }
}
