//! IMAP and SMTP accounts (docs/imap.md). So far: how a message, a thread
//! and a user folder are named; the account's secret; and connecting and
//! signing in to both servers ([`ImapSession`], [`SmtpConnection`]), which
//! the add-account dialog's check uses ([`check_account`]); OAuth
//! sign-in with Google and Microsoft, and its access tokens
//! ([`ImapOAuth`]); sync, and sending over SMTP with Sent and Drafts
//! ([`ImapSync`]).

mod auth;
mod busy;
mod capabilities;
mod check;
mod connect;
mod folders;
mod ids;
mod oauth;
mod preauth;
mod refusal;
mod reply;
mod sasl;
mod secret;
mod session;
mod smtp;
mod sync;
mod threading;
mod tls;
mod tokens;

pub(crate) use busy::BusyGate;
pub use capabilities::Capabilities;
pub use check::{AccountCheck, CheckFailure, check_account};
pub use connect::Connector;
pub use folders::{ListedFolder, SpecialUse};
pub use ids::{message_id, message_id_without_header, mint_folder_label_id, named_by_message_id};
pub(crate) use oauth::OAuthGrant;
pub use oauth::{ImapOAuth, MicrosoftClient, OAuthClients, OAuthProvider, OAuthSignIn};
pub use reply::ImapStream;
pub use secret::{ImapSecret, ImapSecrets};
pub use session::ImapSession;
pub use smtp::SmtpConnection;
pub use sync::{
    AccountFolder, Added, ImapSync, NewImapAccount, NewSignIn, OnGrant, OpenBrowser, SentLookup,
    SignInAgain, SignInCancel, SignInCanceller, sign_in_cancel,
};
pub use threading::{Own, thread_for};
pub use tls::{Timeouts, TlsConfig};
pub use tokens::{AccessTokens, TokenFuture};
