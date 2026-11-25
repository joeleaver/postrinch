//! A fake IMAP server and a fake SMTP submission server, in memory on
//! loopback ports, so Postrinch's tests never touch a real mailbox.
//!
//! Both speak enough of their protocols for `async-imap` and `lettre` (the
//! clients Postrinch uses), over TLS with a self-signed certificate made
//! when the server starts ([`TestCertificate`]; trust it with
//! [`TestCertificate::client_config`] or [`TestCertificate::der`]).
//!
//! ```no_run
//! # async fn example() -> Result<(), postrinch_mailtest::Error> {
//! use postrinch_mailtest::{ImapConfig, ImapServer, NewMessage};
//!
//! let server = ImapServer::start(ImapConfig::new().user("alice", "secret")).await?;
//! let alice = server.user("alice");
//! alice.create_standard_folders()?;
//! alice.append("INBOX", NewMessage::new(&b"Subject: Hi\r\n\r\nHello\r\n"[..]))?;
//! // Connect a client to server.tls_addr(), trusting server.certificate()...
//! # Ok(()) }
//! ```
//!
//! See [`ImapServer`] and [`SmtpServer`] for what each fake does, and
//! `docs/imap.md` for how Postrinch's IMAP support is tested with them.

mod conn;
pub mod imap;
mod listener;
mod sasl;
pub mod smtp;
mod tls;

pub use imap::{
    Extension, FolderInfo, HeldCommand, ImapConfig, ImapFault, ImapServer, MessageInfo, NewMessage,
    SpecialUse, UserHandle,
};
pub use sasl::Mechanism;
pub use smtp::{ReceivedMessage, SmtpConfig, SmtpFault, SmtpServer, SmtpStage};
pub use tls::{HOST, TestCertificate, crypto_provider};

/// What can go wrong starting a fake server or driving it from a test.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("tls: {0}")]
    Tls(String),
    #[error("no user {0} on the fake server")]
    UnknownUser(String),
    #[error("no folder {0}")]
    UnknownFolder(String),
    #[error("no message with UID {1} in {0}")]
    UnknownMessage(String, u32),
    #[error("flag {0} cannot be set")]
    InvalidFlag(String),
    #[error("folder change refused: {0}")]
    Folder(String),
}
