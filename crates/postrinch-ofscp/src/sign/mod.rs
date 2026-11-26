//! Signing (spec §4.4, §7.1): the canonical strings, device keys, the
//! `X-OFSCP-*` headers of a signed request, and the WebSocket `authenticate`
//! signature.
//!
//! Nothing here reads a clock or a random generator itself: the timestamp
//! and nonce are inputs, so the same code is exact against the spec's
//! vectors and serves both a client and a verifier.

mod canonical;
mod key;

pub use canonical::{
    CanonicalRequest, EMPTY_BODY_DIGEST, WS_AUTH_SEPARATOR, canonical_query, content_digest,
    ws_canonical_string,
};
pub use key::{DeviceKey, PublicKey, generate_nonce, signing_timestamp};

use crate::{Actor, Authority};

/// The names of the signing headers (§4.4.1).
pub mod header {
    /// The signer, `handle@domain`.
    pub const ACTOR: &str = "X-OFSCP-Actor";
    /// The registered device key's `key_id`.
    pub const KEY_ID: &str = "X-OFSCP-Key-ID";
    /// RFC 3339 UTC time of signing.
    pub const TIMESTAMP: &str = "X-OFSCP-Timestamp";
    /// Fresh per request, at least 128 bits, base64url.
    pub const NONCE: &str = "X-OFSCP-Nonce";
    /// `base64(SHA-256(body))`.
    pub const CONTENT_DIGEST: &str = "X-OFSCP-Content-Digest";
    /// `base64(Ed25519(canonical string))`.
    pub const SIGNATURE: &str = "X-OFSCP-Signature";
}

/// Who signs: an actor and one of its registered device keys.
#[derive(Debug)]
pub struct Signer<'a> {
    /// The actor the key belongs to.
    pub actor: &'a Actor,
    /// The `key_id` the home provider gave the key.
    pub key_id: &'a str,
    /// The private key.
    pub key: &'a DeviceKey,
}

/// One HTTP request to be signed, as it will be sent.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    /// The provider the request goes to.
    pub authority: &'a Authority,
    /// The HTTP method.
    pub method: &'a str,
    /// The path exactly as in the request-target.
    pub path: &'a str,
    /// The raw query string without its `?`, if any.
    pub query: Option<&'a str>,
    /// The raw body bytes (empty for none).
    pub body: &'a [u8],
}

/// The six `X-OFSCP-*` headers of one signed request (§4.4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedHeaders {
    /// `X-OFSCP-Actor`.
    pub actor: String,
    /// `X-OFSCP-Key-ID`.
    pub key_id: String,
    /// `X-OFSCP-Timestamp`.
    pub timestamp: String,
    /// `X-OFSCP-Nonce`.
    pub nonce: String,
    /// `X-OFSCP-Content-Digest`.
    pub content_digest: String,
    /// `X-OFSCP-Signature`.
    pub signature: String,
}

impl SignedHeaders {
    /// Each header's name and value, to add to the request.
    pub fn pairs(&self) -> [(&'static str, &str); 6] {
        [
            (header::ACTOR, &self.actor),
            (header::KEY_ID, &self.key_id),
            (header::TIMESTAMP, &self.timestamp),
            (header::NONCE, &self.nonce),
            (header::CONTENT_DIGEST, &self.content_digest),
            (header::SIGNATURE, &self.signature),
        ]
    }
}

impl Signer<'_> {
    /// Sign `request` at `timestamp` with `nonce` (see
    /// [`signing_timestamp`] and [`generate_nonce`]). A retry, such as after
    /// a `503`, must be signed again with a fresh nonce.
    pub fn sign_request(
        &self,
        request: &Request<'_>,
        timestamp: &str,
        nonce: &str,
    ) -> SignedHeaders {
        let content_digest = content_digest(request.body);
        let canonical = CanonicalRequest {
            authority: request.authority,
            method: request.method,
            path: request.path,
            query: request.query,
            timestamp,
            nonce,
            content_digest: &content_digest,
        }
        .render();
        SignedHeaders {
            actor: self.actor.to_string(),
            key_id: self.key_id.to_owned(),
            timestamp: timestamp.to_owned(),
            nonce: nonce.to_owned(),
            signature: self.key.sign(&canonical),
            content_digest,
        }
    }

    /// The `signature` of a WebSocket `authenticate` command answering the
    /// challenge `challenge_nonce` from `authority`, sent at `timestamp`
    /// (§7.1).
    pub fn sign_ws_authenticate(
        &self,
        authority: &Authority,
        challenge_nonce: &str,
        timestamp: &str,
    ) -> String {
        self.key
            .sign(&ws_canonical_string(authority, challenge_nonce, timestamp))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signed_request_verifies_and_binds_every_part() {
        let key = DeviceKey::from_seed(&[3u8; 32]);
        let actor = Actor::parse_canonical("alice@a.com").unwrap();
        let signer = Signer {
            actor: &actor,
            key_id: "dk_1",
            key: &key,
        };
        let a = Authority::parse_canonical("a.com").unwrap();
        let b = Authority::parse_canonical("b.com").unwrap();
        let request = Request {
            authority: &a,
            method: "POST",
            path: "/api/x",
            query: Some("b=2&a=1"),
            body: br#"{"hello":"world"}"#,
        };
        let ts = "2026-01-01T00:00:00Z";
        let headers = signer.sign_request(&request, ts, "nonce1");
        assert_eq!(headers.actor, "alice@a.com");
        assert_eq!(
            headers.pairs()[5],
            (header::SIGNATURE, headers.signature.as_str())
        );

        let check = |authority: &Authority, path: &str, digest: &str| {
            let canonical = CanonicalRequest {
                authority,
                method: "POST",
                path,
                query: Some("a=1&b=2"),
                timestamp: ts,
                nonce: "nonce1",
                content_digest: digest,
            }
            .render();
            key.public_key().verify(&canonical, &headers.signature)
        };
        assert!(check(&a, "/api/x", &headers.content_digest));
        assert!(
            !check(&b, "/api/x", &headers.content_digest),
            "authority is bound"
        );
        assert!(
            !check(&a, "/api/y", &headers.content_digest),
            "path is bound"
        );
        assert!(
            !check(&a, "/api/x", &content_digest(b"tampered")),
            "body is bound"
        );

        let again = signer.sign_request(&request, ts, "nonce2");
        assert_ne!(again.signature, headers.signature, "nonce is bound");
    }
}
