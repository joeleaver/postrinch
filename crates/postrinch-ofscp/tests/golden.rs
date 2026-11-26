//! The spec's conformance vectors: request signing (§4.4), the WebSocket
//! `authenticate` signature (§7.1) and `dmId` (§7.4). Byte-exact, so this
//! crate signs as every other implementation does.

use postrinch_ofscp::sign::{
    CanonicalRequest, DeviceKey, EMPTY_BODY_DIGEST, PublicKey, Request, Signer, content_digest,
    ws_canonical_string,
};
use postrinch_ofscp::{Actor, Authority, DmId};
use serde_json::Value;

fn vector(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/ofscp-v0.1/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn text<'a>(v: &'a Value, pointer: &str) -> &'a str {
    v.pointer(pointer).and_then(Value::as_str).unwrap()
}

fn seed(v: &Value) -> [u8; 32] {
    let hex = text(v, "/key/seed_hex");
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    bytes.try_into().unwrap()
}

#[test]
fn the_key_comes_from_its_seed() {
    for name in ["signing-vector.json", "ws-signing-vector.json"] {
        let v = vector(name);
        let key = DeviceKey::from_seed(&seed(&v));
        assert_eq!(
            key.public_key().to_base64(),
            text(&v, "/key/public_key_base64")
        );
    }
}

#[test]
fn request_signing_vector() {
    let v = vector("signing-vector.json");
    let key = DeviceKey::from_seed(&seed(&v));
    let authority = Authority::parse_canonical(text(&v, "/request/authority")).unwrap();
    let body = text(&v, "/request/body").as_bytes();
    let digest = content_digest(body);
    assert_eq!(digest, text(&v, "/headers/X-OFSCP-Content-Digest"));

    let canonical = CanonicalRequest {
        authority: &authority,
        method: text(&v, "/request/method"),
        path: text(&v, "/request/path"),
        query: Some(text(&v, "/request/query")),
        timestamp: text(&v, "/headers/X-OFSCP-Timestamp"),
        nonce: text(&v, "/headers/X-OFSCP-Nonce"),
        content_digest: &digest,
    }
    .render();
    assert_eq!(canonical, text(&v, "/canonical_string"));

    let actor = Actor::parse_canonical(text(&v, "/headers/X-OFSCP-Actor")).unwrap();
    let signer = Signer {
        actor: &actor,
        key_id: text(&v, "/headers/X-OFSCP-Key-ID"),
        key: &key,
    };
    let request = Request {
        authority: &authority,
        method: text(&v, "/request/method"),
        path: text(&v, "/request/path"),
        query: Some(text(&v, "/request/query")),
        body,
    };
    let headers = signer.sign_request(
        &request,
        text(&v, "/headers/X-OFSCP-Timestamp"),
        text(&v, "/headers/X-OFSCP-Nonce"),
    );
    let expected = text(&v, "/expected/signature");
    assert_eq!(headers.signature, expected);
    for (name, value) in headers.pairs() {
        let want = v["headers"].get(name).and_then(Value::as_str);
        assert_eq!(Some(value), want.or(Some(expected)), "{name}");
    }

    let public = PublicKey::from_base64(text(&v, "/key/public_key_base64")).unwrap();
    assert!(public.verify(&canonical, expected));
    assert!(!public.verify(&canonical.replace("foo=1", "foo=2"), expected));
}

#[test]
fn ws_authenticate_vector() {
    let v = vector("ws-signing-vector.json");
    let key = DeviceKey::from_seed(&seed(&v));
    let authority = Authority::parse_canonical(text(&v, "/challenge/authority")).unwrap();
    let nonce = text(&v, "/challenge/challengeNonce");
    let timestamp = text(&v, "/challenge/timestamp");
    assert_eq!(
        ws_canonical_string(&authority, nonce, timestamp),
        text(&v, "/canonical_string")
    );

    let actor = Actor::parse_canonical("alice@providera.com").unwrap();
    let signer = Signer {
        actor: &actor,
        key_id: "dk_abc123xyz",
        key: &key,
    };
    let signature = signer.sign_ws_authenticate(&authority, nonce, timestamp);
    assert_eq!(signature, text(&v, "/expected/signature"));

    // The same values are in the authenticate sample.
    let sample = vector("ws-authenticate.sample.json");
    assert_eq!(text(&sample, "/data/signature"), signature);
    assert_eq!(text(&sample, "/data/timestamp"), timestamp);
    let challenge = vector("ws-auth-challenge.sample.json");
    assert_eq!(text(&challenge, "/data/nonce"), nonce);
}

#[test]
fn empty_body_digest() {
    assert_eq!(content_digest(b""), EMPTY_BODY_DIGEST);
    assert_eq!(
        EMPTY_BODY_DIGEST,
        "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
    );
}

#[test]
fn dm_id_vector() {
    // §7.4's worked example.
    let alice = Actor::parse_canonical("alice@a.com").unwrap();
    let bob = Actor::parse_canonical("bob@b.com").unwrap();
    let id = "dm_c2a3a0d4bc7aa54700d2f412c42fc0155df6071e502977e4988933eef7e46868";
    assert_eq!(DmId::derive(&alice, &bob).as_str(), id);
    assert_eq!(DmId::derive(&bob, &alice).as_str(), id);
}
