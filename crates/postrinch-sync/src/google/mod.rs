//! Google accounts: sign-in, and Gmail sync.

mod api;
mod auth;
mod config;
mod labels;
mod loopback;
mod oauth;
mod pkce;
#[cfg(all(unix, not(target_os = "macos")))]
mod secret_service;
mod secrets;
mod sync;

pub use api::GmailApi;
pub use auth::GoogleAuth;
pub use config::ClientConfig;
pub use oauth::{Endpoints, SCOPES};
pub use secrets::{Keychain, MemorySecrets, SecretStore};
pub use sync::{GmailSync, Progress, ProgressFn, SyncReport};
