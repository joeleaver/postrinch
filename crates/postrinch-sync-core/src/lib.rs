//! What Postrinch's sync engines share: mail's (`postrinch-sync`) and the
//! ones to come (docs/ofscp.md, "Crates").
//!
//! - **Secrets** ([`SecretStore`]): refresh tokens and passwords live in
//!   the OS keychain ([`Keychain`]), never in the store; tests keep them
//!   in memory ([`MemorySecrets`]). On Linux, a secret is kept only after
//!   making sure the Secret Service has a default keyring to write to.
//! - **Backing off** ([`Backoff`]): how long to wait before trying a busy
//!   server again.

mod backoff;
mod error;
#[cfg(all(unix, not(target_os = "macos")))]
mod secret_service;
mod secrets;

pub use backoff::Backoff;
pub use error::{KeychainError, KeychainResult};
pub use secrets::{Keychain, MemorySecrets, SecretStore};
