//! Google accounts: sign-in, and Gmail sync.

mod accounts;
mod api;
mod auth;
mod config;
mod labels;
mod loopback;
mod oauth;
mod pkce;
mod quota;
#[cfg(all(unix, not(target_os = "macos")))]
mod secret_service;
mod secrets;
mod sync;

pub use accounts::{remove_google_account, sign_in_google};
pub use api::GmailApi;
pub(crate) use api::chain as error_chain;
pub use auth::GoogleAuth;
pub use config::ClientConfig;
pub use oauth::{Endpoints, SCOPES};
pub use secrets::{Keychain, MemorySecrets, SecretStore};
pub use sync::{
    GmailSync, KeepOptions, Phase, Progress, ProgressFn, SyncEvent, SyncEventFn, SyncReport,
};
