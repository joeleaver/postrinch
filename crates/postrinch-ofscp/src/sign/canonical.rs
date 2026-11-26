//! The byte-exact strings OFSCP signs (spec §4.4.1, §4.4.2, §7.1).
//!
//! These must reproduce `ofscp/tests/signing-vector.json` and
//! `ofscp/tests/ws-signing-vector.json` exactly, or every signed request
//! silently fails. The same functions serve the signer (the client) and the
//! verifier (a provider, or Postrinch's fake provider in tests).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use sha2::{Digest, Sha256};

use crate::Authority;

/// `X-OFSCP-Content-Digest` of an empty body (§4.4.1).
pub const EMPTY_BODY_DIGEST: &str = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=";

/// The literal second line of the WebSocket canonical string (§7.1).
pub const WS_AUTH_SEPARATOR: &str = "ofscp-ws-authenticate";

/// `base64(SHA-256(body))` (§4.4.1).
pub fn content_digest(body: &[u8]) -> String {
    B64.encode(Sha256::digest(body))
}

/// The `<query>` line (§4.4.2 item 4): the raw `name=value` pairs exactly as
/// sent (no percent-decoding), sorted ascending by bytes, joined with `&`.
///
/// `raw` is the query string without its leading `?`. Empty pairs produced
/// by stray `&`s are dropped, since they are not `name=value` pairs. `None`
/// and `""` both give `""`: the line is still present, and empty.
pub fn canonical_query(raw: Option<&str>) -> String {
    let mut pairs: Vec<&str> = raw
        .unwrap_or("")
        .split('&')
        .filter(|p| !p.is_empty())
        .collect();
    pairs.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    pairs.join("&")
}

/// The components of a §4.4.2 canonical string.
#[derive(Debug, Clone, Copy)]
pub struct CanonicalRequest<'a> {
    /// The target provider, in canonical form.
    pub authority: &'a Authority,
    /// The HTTP method (uppercased when rendered).
    pub method: &'a str,
    /// The path exactly as in the request-target, with no dot-segment or
    /// trailing-slash normalization; `""` becomes `/`.
    pub path: &'a str,
    /// The raw query string without its `?`, if there is one.
    pub query: Option<&'a str>,
    /// The exact `X-OFSCP-Timestamp` value.
    pub timestamp: &'a str,
    /// The exact `X-OFSCP-Nonce` value.
    pub nonce: &'a str,
    /// The exact `X-OFSCP-Content-Digest` value.
    pub content_digest: &'a str,
}

impl CanonicalRequest<'_> {
    /// The seven LF-joined lines, with no trailing newline.
    pub fn render(&self) -> String {
        let path = if self.path.is_empty() { "/" } else { self.path };
        [
            self.authority.as_str(),
            &self.method.to_ascii_uppercase(),
            path,
            &canonical_query(self.query),
            self.timestamp,
            self.nonce,
            self.content_digest,
        ]
        .join("\n")
    }
}

/// The four-line WebSocket `authenticate` canonical string (§7.1):
/// the provider's authority, [`WS_AUTH_SEPARATOR`], the challenge nonce from
/// `auth.challenge`, and the `authenticate` timestamp.
pub fn ws_canonical_string(
    authority: &Authority,
    challenge_nonce: &str,
    timestamp: &str,
) -> String {
    [
        authority.as_str(),
        WS_AUTH_SEPARATOR,
        challenge_nonce,
        timestamp,
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority(s: &str) -> Authority {
        Authority::parse_canonical(s).unwrap()
    }

    #[test]
    fn empty_digest() {
        assert_eq!(content_digest(b""), EMPTY_BODY_DIGEST);
    }

    #[test]
    fn query_sorting() {
        assert_eq!(canonical_query(None), "");
        assert_eq!(canonical_query(Some("")), "");
        assert_eq!(canonical_query(Some("b=2&a=1")), "a=1&b=2");
        assert_eq!(
            canonical_query(Some("foo=1&bar=2&baz=3")),
            "bar=2&baz=3&foo=1"
        );
        // Raw, not decoded; byte order puts upper case before lower.
        assert_eq!(canonical_query(Some("q=a%20b&Z=1&&a=")), "Z=1&a=&q=a%20b");
        assert_eq!(canonical_query(Some("a=1&A=2")), "A=2&a=1");
        // Duplicate names keep both pairs.
        assert_eq!(canonical_query(Some("x=2&x=1")), "x=1&x=2");
    }

    #[test]
    fn seven_lines_and_an_empty_query_line() {
        let a = authority("a.com");
        let c = CanonicalRequest {
            authority: &a,
            method: "get",
            path: "",
            query: None,
            timestamp: "t",
            nonce: "n",
            content_digest: EMPTY_BODY_DIGEST,
        };
        let s = c.render();
        assert_eq!(s, format!("a.com\nGET\n/\n\nt\nn\n{EMPTY_BODY_DIGEST}"));
        assert_eq!(s.split('\n').count(), 7);
        assert!(!s.ends_with('\n'));
    }

    #[test]
    fn path_is_not_normalized() {
        let a = authority("a.com:8443");
        let c = CanonicalRequest {
            authority: &a,
            method: "GET",
            path: "/a/../b/./c/",
            query: None,
            timestamp: "t",
            nonce: "n",
            content_digest: EMPTY_BODY_DIGEST,
        };
        let s = c.render();
        let lines: Vec<&str> = s.split('\n').collect();
        assert_eq!(lines[0], "a.com:8443");
        assert_eq!(lines[2], "/a/../b/./c/");
    }

    #[test]
    fn ws_string_is_four_lines() {
        let s = ws_canonical_string(&authority("a.com"), "nonce", "2026-01-01T00:00:00Z");
        assert_eq!(
            s,
            "a.com\nofscp-ws-authenticate\nnonce\n2026-01-01T00:00:00Z"
        );
    }
}
