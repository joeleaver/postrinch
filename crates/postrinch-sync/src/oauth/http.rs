//! The HTTP client OAuth requests go out on: code exchange, refresh,
//! userinfo and revocation, for every provider.

use std::time::Duration;

use crate::{SyncError, SyncResult};

/// How long an OAuth request may take. A provider that never answers
/// fails the request rather than hanging it, and with it the account's
/// refresh, which other callers for the account wait on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OAuthTimeouts {
    /// To open the connection (TCP and TLS).
    pub connect: Duration,
    /// For the whole request, from sending it to the last byte of the
    /// answer.
    pub request: Duration,
}

impl Default for OAuthTimeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            request: Duration::from_secs(30),
        }
    }
}

/// The client for OAuth requests, with `timeouts`.
pub(crate) fn http_client(timeouts: OAuthTimeouts) -> SyncResult<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(timeouts.connect)
        .timeout(timeouts.request)
        .build()
        .map_err(|e| SyncError::Config(format!("the HTTP client for sign-in: {e}")))
}
