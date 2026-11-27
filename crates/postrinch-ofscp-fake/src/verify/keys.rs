//! Finding a remote actor's keys (§4.6): fetched from the actor's home
//! provider, cached until `cache_until`, re-fetched when the key is not in
//! the cached list or fails against a cached key.
//!
//! The fakes are on loopback IP literals, which §8 *Outbound requests*
//! forbids fetching unless the operator allows them as internal peers. A
//! fake provider is such an operator: it allows loopback, and nothing else
//! (any other authority is answered as a fetch the rules refuse).

use std::collections::HashMap;
use std::sync::Mutex;

use jiff::Timestamp;
use ofscp_core::signing::{VerifyingKey, decode_public_key};
use ofscp_core::wire::UserKeysResponse;
use ofscp_core::{Actor, Authority};

/// The keys of one actor as last fetched.
#[derive(Debug, Clone)]
struct Cached {
    keys: Vec<(String, VerifyingKey)>,
    until: Timestamp,
}

/// What looking a key up found.
#[derive(Debug, Clone)]
pub(crate) enum Lookup {
    /// The key, and until when it may be relied on.
    Found {
        key: VerifyingKey,
        until: Timestamp,
        from_cache: bool,
    },
    /// A completed fetch shows no such key (`200` without it, or `404`).
    Gone,
    /// The keys could not be fetched and no unexpired cached key matches.
    Failed(String),
    /// The outbound-request rules refuse the fetch (§8).
    Refused,
}

pub(crate) struct KeyResolver {
    client: reqwest::Client,
    cache: Mutex<HashMap<Actor, Cached>>,
}

impl KeyResolver {
    pub(crate) fn new() -> Self {
        Self {
            client: crate::tls::outbound_client(),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// The key `key_id` of `actor`, from the unexpired cache when it is
    /// there and `refetch` is not asked for, else from a fresh fetch.
    pub(crate) async fn key(
        &self,
        actor: &Actor,
        key_id: &str,
        now: Timestamp,
        refetch: bool,
    ) -> Lookup {
        let cached = self.cache.lock().unwrap().get(actor).cloned();
        let cached = cached.filter(|c| c.until > now);
        if !refetch && let Some(found) = cached.as_ref().and_then(|c| pick(c, key_id, true)) {
            return found;
        }
        if !allowed_peer(actor.domain()) {
            // A matching unexpired cached key is still used (§4.6).
            return cached
                .and_then(|c| pick(&c, key_id, true))
                .unwrap_or(Lookup::Refused);
        }
        match self.fetch(actor).await {
            Ok(fresh) => {
                let found = pick(&fresh, key_id, false).unwrap_or(Lookup::Gone);
                self.cache.lock().unwrap().insert(actor.clone(), fresh);
                found
            }
            Err(why) => cached
                .and_then(|c| pick(&c, key_id, true))
                .unwrap_or(Lookup::Failed(why)),
        }
    }

    /// One §4.6 fetch, classified: `Ok` for a completed fetch (a `404`
    /// is an empty list), `Err` for a fetch failure.
    async fn fetch(&self, actor: &Actor) -> Result<Cached, String> {
        let url = format!(
            "https://{}/.well-known/ofscp/users/{}/keys",
            actor.domain().as_str(),
            actor.handle()
        );
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("fetching {url}: {e}"))?;
        match response.status().as_u16() {
            200 => {}
            404 => {
                return Ok(Cached {
                    keys: Vec::new(),
                    // Nothing says how long a 404 holds; re-fetch next time.
                    until: Timestamp::UNIX_EPOCH,
                });
            }
            status => return Err(format!("{url} answered {status}")),
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("reading {url}: {e}"))?;
        let body: UserKeysResponse = serde_json::from_slice(&bytes)
            .map_err(|e| format!("{url} is not a keys response: {e}"))?;
        if &body.actor != actor {
            return Err(format!("{url} named {} instead", body.actor));
        }
        let keys = body
            .keys
            .iter()
            .filter_map(|k| Some((k.key_id.clone(), decode_public_key(&k.public_key).ok()?)))
            .collect();
        Ok(Cached {
            keys,
            until: body.cache_until,
        })
    }
}

fn pick(cached: &Cached, key_id: &str, from_cache: bool) -> Option<Lookup> {
    cached
        .keys
        .iter()
        .find(|(id, _)| id == key_id)
        .map(|(_, key)| Lookup::Found {
            key: *key,
            until: cached.until,
            from_cache,
        })
}

/// The outbound-request rules as this fake's operator configures them:
/// loopback peers only.
fn allowed_peer(domain: &Authority) -> bool {
    let host = domain
        .as_str()
        .rsplit_once(':')
        .map_or(domain.as_str(), |(h, _)| h);
    host == "localhost"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}
