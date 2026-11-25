//! Postrinch's providers: signing in, and keeping the store in step with
//! the provider's copy of the mail.
//!
//! Google accounts sign in with OAuth (the installed-app flow: PKCE, the
//! system browser, a loopback redirect) and sync over the Gmail API
//! ([`google`]). Other accounts are IMAP and SMTP ([`imap`],
//! docs/imap.md). [`MailSync`] is the one sync the app, the command line
//! and the agents use: it sends each call to the account's provider.
//! Every write goes through `postrinch-store`; nothing here keeps state of
//! its own except secrets (refresh tokens and passwords), which live in
//! the OS keychain ([`Keychain`]).

mod accounts;
mod backoff;
mod compose;
mod error;
mod events;
pub mod google;
pub mod imap;
mod mail;
mod mime;
mod oauth;
mod outbox_runs;
mod reparse;
mod report;
#[cfg(all(unix, not(target_os = "macos")))]
mod secret_service;
mod secrets;

pub use accounts::check_address_free;
pub use backoff::Backoff;
pub use compose::{build_message, outgoing_message_id};
pub use error::{SyncError, SyncResult};
pub use events::{IdleOptions, KeepOptions, SyncEvent, SyncEventFn};
pub use mail::MailSync;
pub use mime::{Envelope, parse_headers, parse_message};
pub use oauth::OAuthTimeouts;
pub use reparse::reparse;
pub use report::{Phase, Progress, ProgressFn, SendReport, SyncReport};
pub use secrets::{Keychain, MemorySecrets, SecretStore};
