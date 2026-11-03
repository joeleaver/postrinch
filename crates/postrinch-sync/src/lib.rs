//! Postrinch's providers: signing in, and keeping the store in step with
//! the provider's copy of the mail.
//!
//! Google accounts sign in with OAuth (the installed-app flow: PKCE, the
//! system browser, a loopback redirect) and sync over the Gmail API. Every
//! write goes through `postrinch-store`; nothing here keeps state of its own
//! except refresh tokens, which live in the OS keychain.

mod error;
pub mod google;
mod mime;

pub use error::{SyncError, SyncResult};
pub use mime::{Envelope, parse_headers, parse_message};
