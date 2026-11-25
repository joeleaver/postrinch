//! The fake SMTP submission server.
//!
//! It listens with implicit TLS (the 465-style port) and in plain text
//! with STARTTLS (the 587-style port), signs users in with AUTH PLAIN,
//! LOGIN, XOAUTH2 or OAUTHBEARER, and records every message it accepts for the test to
//! inspect. Failures can be programmed at each stage of a submission,
//! including the uncertain one: the data received and the connection
//! dropped without a reply, either before the message is stored or after
//! it was delivered. Message data is read as strictly as real servers
//! read it (see `data`).

mod data;
mod session;

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

use crate::Error;
use crate::listener;
use crate::sasl::Mechanism;
use crate::tls::TestCertificate;

/// A point in a submission where a failure can be programmed.
///
/// A [`SmtpFault::Reply`] with code 421 closes the connection after the
/// reply at every stage, as RFC 5321 3.8 has a server shutting down do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpStage {
    /// The greeting, sent once as a connection opens. A reply takes the
    /// place of the 220: with 421 the server then closes; with any other
    /// code (554, RFC 5321 3.1) it stays, answering every command but QUIT
    /// with 503. A disconnect closes without a greeting.
    Greeting,
    /// The EHLO command (before or after STARTTLS alike).
    Ehlo,
    /// The STARTTLS command, on the port that offers it. A reply (454,
    /// "TLS not available due to temporary reason", RFC 3207 4) leaves
    /// the connection in plain text.
    StartTls,
    /// The AUTH command.
    Auth,
    /// MAIL FROM.
    Mail,
    /// RCPT TO.
    Rcpt,
    /// The reply after the message data has been received (and found
    /// well-formed and within the size limit).
    Data,
}

/// What a programmed failure does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmtpFault {
    /// Reply with this code (4xx or 5xx) and text instead of going on.
    /// At [`SmtpStage::Data`] the message is not stored.
    Reply { code: u16, text: String },
    /// Close the connection without replying and without doing what was
    /// asked. At [`SmtpStage::Data`] the server has the whole message but
    /// drops it: nothing is delivered, though the client cannot tell, so
    /// a retry delivers it once.
    Disconnect,
    /// Only at [`SmtpStage::Data`]: store the message (it shows in
    /// [`SmtpServer::messages`]), then close the connection without
    /// replying. The client cannot tell this from [`SmtpFault::Disconnect`],
    /// and a retry delivers the message a second time.
    StoreThenDisconnect,
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
    /// The message exactly as received, less the dot-stuffing; every line
    /// ends in CRLF, since data with a bare CR or LF is refused.
    pub data: Vec<u8>,
}

/// The fake SMTP server's setup. By default it listens on both ports,
/// accepts messages up to 35 MB, offers PLAIN, LOGIN and XOAUTH2, and has
/// no users.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpConfig {
    implicit_tls: bool,
    starttls: bool,
    offer_starttls: bool,
    max_size: usize,
    mechanisms: BTreeSet<Mechanism>,
    users: Vec<SmtpUser>,
    challenges_again: bool,
    max_recipients: Option<usize>,
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
            offer_starttls: true,
            max_size: 35 * 1024 * 1024,
            mechanisms: Mechanism::DEFAULT.into_iter().collect(),
            users: Vec::new(),
            challenges_again: false,
            max_recipients: None,
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

    /// Whether the plain-text port offers STARTTLS. On by default. Off, it
    /// is a server with no encryption at all, as misconfigured servers
    /// are: it offers AUTH over plain text. For testing that clients
    /// refuse it.
    pub fn offer_starttls(mut self, on: bool) -> Self {
        self.offer_starttls = on;
        self
    }

    /// Offers exactly these SASL mechanisms ([`Mechanism::Plain`] offers
    /// `PLAIN` and `LOGIN`).
    pub fn mechanisms(mut self, mechanisms: &[Mechanism]) -> Self {
        self.mechanisms = mechanisms.iter().copied().collect();
        self
    }

    /// Whether a refused XOAUTH2 or OAUTHBEARER sign-in misbehaves: after
    /// the client answers the error challenge, the server sends the same
    /// challenge again instead of 535, until the client cancels with `*`
    /// (then 501). Off by default. For testing that clients give up
    /// rather than loop or misreport it.
    pub fn challenges_again(mut self, on: bool) -> Self {
        self.challenges_again = on;
        self
    }

    /// The largest message accepted (advertised as SIZE).
    pub fn max_size(mut self, bytes: usize) -> Self {
        self.max_size = bytes;
        self
    }

    /// Takes at most `count` recipients in one transaction, answering
    /// any RCPT beyond them `452 4.5.3 Too many recipients` (RFC 5321
    /// 4.5.3.1.10). By default there is no limit.
    pub fn max_recipients(mut self, count: usize) -> Self {
        self.max_recipients = Some(count);
        self
    }
}

#[derive(Debug)]
struct PendingFault {
    stage: SmtpStage,
    /// How many submissions reach `stage` unharmed first.
    skip: usize,
    remaining: usize,
    fault: SmtpFault,
}

/// What a test has done with each message the server accepts.
type OnAccept = Arc<dyn Fn(&ReceivedMessage) + Send + Sync>;

pub(crate) struct SmtpState {
    config: SmtpConfig,
    faults: Vec<PendingFault>,
    messages: Vec<ReceivedMessage>,
    log: Vec<String>,
    on_accept: Option<OnAccept>,
}

impl SmtpState {
    fn take_fault(&mut self, stage: SmtpStage) -> Option<SmtpFault> {
        let i = self
            .faults
            .iter()
            .position(|f| f.stage == stage && f.remaining > 0)?;
        let pending = &mut self.faults[i];
        if pending.skip > 0 {
            pending.skip -= 1;
            return None;
        }
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

    /// Keeps an accepted message, then hands it to the test's hook (with
    /// the state unlocked, so the hook may use the server).
    fn accept(&self, message: ReceivedMessage) {
        let hook = {
            let mut state = self.lock();
            state.messages.push(message.clone());
            state.on_accept.clone()
        };
        if let Some(hook) = hook {
            hook(&message);
        }
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
                on_accept: None,
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

    /// Calls `hook` with each message the server accepts from now on,
    /// before it answers the data (also for
    /// [`SmtpFault::StoreThenDisconnect`]): to play a provider that files
    /// sent mail in Sent itself, by appending it there on the fake IMAP
    /// server.
    pub fn on_accept(&self, hook: impl Fn(&ReceivedMessage) + Send + Sync + 'static) {
        self.shared.lock().on_accept = Some(Arc::new(hook));
    }

    /// Makes the next `times` submissions reaching `stage` fail as `fault`
    /// says.
    ///
    /// # Panics
    ///
    /// If `fault` is [`SmtpFault::StoreThenDisconnect`] and `stage` is not
    /// [`SmtpStage::Data`]: only DATA has a message to store.
    pub fn fail_next(&self, stage: SmtpStage, times: usize, fault: SmtpFault) {
        self.fail_after(stage, 0, times, fault);
    }

    /// As [`SmtpServer::fail_next`], once `skip` submissions have reached
    /// `stage` unharmed: `fail_after(SmtpStage::Mail, 1, 1, ...)` fails
    /// the second MAIL only.
    ///
    /// # Panics
    ///
    /// As [`SmtpServer::fail_next`].
    pub fn fail_after(&self, stage: SmtpStage, skip: usize, times: usize, fault: SmtpFault) {
        assert!(
            stage == SmtpStage::Data || fault != SmtpFault::StoreThenDisconnect,
            "StoreThenDisconnect only applies at SmtpStage::Data"
        );
        self.shared.lock().faults.push(PendingFault {
            stage,
            skip,
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
