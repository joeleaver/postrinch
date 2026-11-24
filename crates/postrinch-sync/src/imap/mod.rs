//! IMAP and SMTP accounts (docs/imap.md). So far: how a message, a thread
//! and a user folder are named; the account's secret; and connecting and
//! signing in to both servers ([`ImapSession`], [`SmtpConnection`]), which
//! the add-account dialog's check uses ([`check_account`]).

mod auth;
mod capabilities;
mod check;
mod connect;
mod folders;
mod ids;
mod preauth;
mod reply;
mod sasl;
mod secret;
mod session;
mod smtp;
mod threading;
mod tls;
mod tokens;

pub use capabilities::Capabilities;
pub use check::{AccountCheck, CheckFailure, check_account};
pub use connect::Connector;
pub use folders::{ListedFolder, SpecialUse};
pub use ids::{message_id, message_id_without_header, mint_folder_label_id};
pub use reply::ImapStream;
pub use secret::{ImapSecret, ImapSecrets};
pub use session::ImapSession;
pub use smtp::SmtpConnection;
pub use threading::{Own, thread_for};
pub use tls::{Timeouts, TlsConfig};
pub use tokens::{AccessTokens, TokenFuture};
