//! Signature verification, as §4.5 orders it, for REST requests and the
//! WebSocket `authenticate` (§7.1).

pub(crate) mod keys;
pub(crate) mod replay;

use axum::http::HeaderMap;
use jiff::Timestamp;
use ofscp_core::signing::{
    CanonicalRequest, VerifyingKey, content_digest, headers, verify, ws_canonical_string,
};
use ofscp_core::ws::Authenticate;
use ofscp_core::{Actor, Authority};

use crate::inner::Inner;
use crate::problem::Problem;
use keys::Lookup;
use replay::{SKEW, Signer};

/// Who a verified signature speaks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Verified {
    pub(crate) actor: Actor,
    pub(crate) key_id: String,
    /// For a remote actor, until when its key may be relied on (§7.1:
    /// a connection is re-checked before then).
    pub(crate) until: Option<Timestamp>,
}

/// What a signed request carried.
pub(crate) struct SignedRequest<'a> {
    pub(crate) method: &'a str,
    pub(crate) path: &'a str,
    pub(crate) query: Option<&'a str>,
    pub(crate) headers: &'a HeaderMap,
    pub(crate) body: &'a [u8],
}

/// Does the request carry any `X-OFSCP-*` signature header? Then it is a
/// signed request and every check applies (§4.5 *Anonymous viewers*).
pub(crate) fn is_signed(map: &HeaderMap) -> bool {
    headers::ALL.iter().any(|name| map.contains_key(*name))
}

fn header<'a>(map: &'a HeaderMap, name: &str) -> Option<&'a str> {
    map.get(name).and_then(|v| v.to_str().ok())
}

/// The six headers a client sends (§4.4.1).
const CLIENT_HEADERS: [&str; 6] = [
    headers::ACTOR,
    headers::KEY_ID,
    headers::TIMESTAMP,
    headers::NONCE,
    headers::CONTENT_DIGEST,
    headers::SIGNATURE,
];

/// Run the seven checks of §4.5 in order, then record the nonce.
pub(crate) async fn verify_request(
    inner: &Inner,
    req: SignedRequest<'_>,
) -> Result<Verified, Problem> {
    // 1. All six headers.
    let mut values = [""; 6];
    for (slot, name) in values.iter_mut().zip(CLIENT_HEADERS) {
        *slot = header(req.headers, name)
            .ok_or_else(|| Problem::unauthorized(format!("missing {name}")))?;
    }
    let [actor, key_id, timestamp, nonce, digest, signature] = values;
    let actor = Actor::parse_canonical(actor)
        .map_err(|_| Problem::unauthorized("X-OFSCP-Actor is not a canonical actor"))?;

    // 2. The authority signed for is the one this provider serves. The
    // canonical string is rebuilt with the request's own Host, so a
    // signature for another host fails here.
    let host = header(req.headers, "host").unwrap_or_default();
    let served = Authority::canonicalize(host).ok();
    if served.as_ref() != Some(&inner.authority) {
        return Err(Problem::unauthorized(format!(
            "signed for {host}, not this provider"
        )));
    }

    // 3. ±300 s. A timestamp inside the window but from before the
    // replay store's warm-up ended (§4.5 *Restarts*) is answered 503, so
    // a client re-signs rather than thinking its credentials are bad.
    let now = inner.clock.now();
    let signed_at = parse_timestamp(timestamp)?;
    check_skew(signed_at, now)?;
    inner.replay.check_warm_up(signed_at, now)?;

    // 4. Replay.
    if inner.replay.seen(Signer::Actor, key_id, nonce) {
        return Err(Problem::unauthorized("nonce already used"));
    }

    // 5. Digest.
    if content_digest(req.body) != digest {
        return Err(Problem::bad_request(
            "X-OFSCP-Content-Digest does not match the body",
        ));
    }

    // 6 and 7. The key, and the signature over the canonical string.
    let canonical = CanonicalRequest {
        authority: inner.authority.as_str(),
        method: req.method,
        path: req.path,
        query: req.query,
        timestamp,
        nonce,
        content_digest: digest,
    }
    .render()
    .map_err(|_| Problem::unauthorized("a signed component contains a line break"))?;
    let until = check_signature(inner, &actor, key_id, &canonical, signature, now).await?;

    match inner
        .replay
        .record(Signer::Actor, key_id, nonce, signed_at, now)?
    {
        true => Ok(Verified {
            actor,
            key_id: key_id.to_owned(),
            until,
        }),
        false => Err(Problem::unauthorized("nonce already used")),
    }
}

/// Verify a WebSocket `authenticate` against this connection's challenge
/// (§7.1: §4.5 steps 3, 6 and 7).
pub(crate) async fn verify_ws(
    inner: &Inner,
    auth: &Authenticate,
    challenge: &str,
) -> Result<Verified, Problem> {
    let now = inner.clock.now();
    check_skew(parse_timestamp(&auth.timestamp)?, now)?;
    let canonical = ws_canonical_string(inner.authority.as_str(), challenge, &auth.timestamp)
        .map_err(|_| Problem::unauthorized("a signed component contains a line break"))?;
    let until = check_signature(
        inner,
        &auth.actor,
        &auth.key_id,
        &canonical,
        &auth.signature,
        now,
    )
    .await?;
    Ok(Verified {
        actor: auth.actor.clone(),
        key_id: auth.key_id.clone(),
        until,
    })
}

fn parse_timestamp(s: &str) -> Result<Timestamp, Problem> {
    if !s.ends_with('Z') {
        return Err(Problem::unauthorized(
            "the timestamp is not RFC 3339 UTC (Z)",
        ));
    }
    s.parse()
        .map_err(|_| Problem::unauthorized("the timestamp is not RFC 3339"))
}

fn check_skew(signed_at: Timestamp, now: Timestamp) -> Result<(), Problem> {
    let skew = signed_at.duration_since(now).abs();
    if skew > SKEW {
        return Err(Problem::unauthorized(format!(
            "the timestamp is {}s from this provider's clock",
            skew.as_secs()
        )));
    }
    Ok(())
}

/// Steps 6 and 7. Returns, for a remote actor, until when its key holds.
async fn check_signature(
    inner: &Inner,
    actor: &Actor,
    key_id: &str,
    canonical: &str,
    signature: &str,
    now: Timestamp,
) -> Result<Option<Timestamp>, Problem> {
    if actor.is_local_to(&inner.authority) {
        let key = inner
            .local_key(actor, key_id)
            .ok_or_else(|| Problem::unauthorized("no such active key"))?;
        return signature_matches(&key, canonical, signature).map(|()| None);
    }
    let mut lookup = inner.resolver.key(actor, key_id, now, false).await;
    // A key from the cache that fails is re-fetched before the final
    // rejection, in case it was rotated (§4.6).
    if let Lookup::Found {
        key,
        from_cache: true,
        ..
    } = &lookup
        && !verify(key, canonical, signature)
    {
        lookup = inner.resolver.key(actor, key_id, now, true).await;
    }
    match lookup {
        Lookup::Found { key, until, .. } => {
            signature_matches(&key, canonical, signature).map(|()| Some(until))
        }
        Lookup::Gone => Err(Problem::unauthorized("no such active key")),
        Lookup::Refused => Err(Problem::forbidden(
            "the outbound-request rules refuse fetching this actor's keys",
        )),
        Lookup::Failed(why) => Err(Problem::unavailable(
            format!("the actor's keys could not be fetched: {why}"),
            Some(1),
        )),
    }
}

fn signature_matches(key: &VerifyingKey, canonical: &str, signature: &str) -> Result<(), Problem> {
    if verify(key, canonical, signature) {
        Ok(())
    } else {
        Err(Problem::unauthorized("the signature does not verify"))
    }
}
