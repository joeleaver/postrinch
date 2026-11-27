//! The log of HTTP requests a provider received, for tests to inspect.

use ofscp_core::Actor;

/// How a request was authenticated, or why it was not (§4.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Authentication {
    /// No `X-OFSCP-*` header and no bearer token: an anonymous viewer.
    Unsigned,
    /// A bootstrap token (`Authorization: Bearer`, §4.2).
    Bearer,
    /// Every check of §4.5 passed.
    Verified { actor: Actor, key_id: String },
    /// A check of §4.5 failed, answered with this status and code.
    Rejected { status: u16, error_code: String },
    /// A fault answered the signed request before it was checked
    /// ([`Faults::fail_next`](crate::Faults::fail_next)).
    Faulted { status: u16 },
}

/// One HTTP request, as the provider saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRequest {
    pub method: String,
    /// The path exactly as sent, without the query.
    pub path: String,
    /// The raw query, without `?`.
    pub query: Option<String>,
    /// Every header, names lowercased, in the order received.
    pub headers: Vec<(String, String)>,
    /// `X-OFSCP-Nonce`, when sent.
    pub nonce: Option<String>,
    pub authentication: Authentication,
    /// The status the provider answered with.
    pub status: u16,
}

impl RecordedRequest {
    /// The first value of header `name` (lowercase).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}
