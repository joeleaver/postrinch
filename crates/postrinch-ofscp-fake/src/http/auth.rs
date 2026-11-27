//! Discovery (§3.1), the keys endpoint (§4.6), registration and login
//! (§4.1), bootstrap tokens (§4.2) and device keys (§4.3, §4.7).

use std::sync::Arc;

use axum::Extension;
use axum::extract::{Path, State};
use ofscp_core::signing::decode_public_key;
use ofscp_core::wire::{
    Authentication, BOOTSTRAP_TOKEN_TYPE, BootstrapResponse, Capabilities, DeviceKeyCreated,
    DeviceKeyInfo, DeviceKeyRegistration, DeviceKeysResponse, DiscoveryDocument, ErrorCode,
    FederationCapabilities, KeyAlgorithm, Limits, LoginRequest, MessageType, ProviderInfo,
    PublicKey, RealtimeDelivery, RegistrationMode, Software, Tier, UserKeysResponse,
};
use ofscp_core::{Actor, is_valid_handle};

use super::RequestBody;
use super::respond::{Caller, Reply, body, created, no_content, ok};
use crate::inner::Inner;
use crate::problem::Problem;
use crate::state::DeviceKey;

/// How long a bootstrap token lasts (§4.2, RECOMMENDED 300 s).
const BOOTSTRAP_TTL: u64 = 300;

pub(super) async fn discovery(State(inner): State<Arc<Inner>>) -> Reply {
    let settings = inner.settings();
    ok(&DiscoveryDocument {
        provider: ProviderInfo {
            domain: inner.authority.clone(),
            protocol_version: ofscp_core::PROTOCOL_VERSION.to_owned(),
            software: Software {
                name: "postrinch-ofscp-fake".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            contact: "mailto:postmaster@localhost".to_owned(),
            authentication: Authentication {
                login_endpoint: "/api/auth/login".to_owned(),
                registration_endpoint: None,
                registration: Some(RegistrationMode::Closed),
            },
            public_keys: Vec::new(),
        },
        capabilities: Capabilities {
            message_types: Some(vec![
                MessageType::Message,
                MessageType::Memo,
                MessageType::Article,
            ]),
            tiers: Some(vec![
                Tier::Private,
                Tier::Group,
                Tier::Public,
                Tier::Discoverable,
            ]),
            metadata_schemas: None,
            limits: Some(Limits {
                max_upload_bytes: Some(settings.max_upload_bytes),
            }),
            federation: Some(FederationCapabilities {
                realtime_delivery: Some(RealtimeDelivery::DirectWs),
            }),
            discovery: None,
        },
        endpoints: None,
    })
}

/// `GET /.well-known/ofscp/users/{handle}/keys`: the same shape for an
/// unknown handle as for a keyless one (§4.6).
pub(super) async fn user_keys(
    State(inner): State<Arc<Inner>>,
    Path(handle): Path<String>,
) -> Reply {
    if !is_valid_handle(&handle) {
        return Err(Problem::not_found("not a valid handle"));
    }
    let actor = Actor::new(&handle, inner.authority.clone())
        .map_err(|_| Problem::not_found("not a valid handle"))?;
    let cache_until = inner.now() + inner.settings().key_cache_ttl;
    let state = inner.state.lock().unwrap();
    let keys = state
        .keys
        .iter()
        .filter(|(_, k)| !k.revoked && k.handle == handle)
        .map(|(id, k)| PublicKey {
            key_id: id.clone(),
            algorithm: KeyAlgorithm::Ed25519,
            public_key: k.public_key.clone(),
            created_at: k.created_at,
        })
        .collect();
    ok(&UserKeysResponse {
        actor,
        keys,
        cache_until,
    })
}

/// Registration is closed: accounts are seeded by the test (§4.1.1).
pub(super) async fn register() -> Reply {
    Err(Problem::new(
        ErrorCode::RegistrationClosed,
        "accounts are made by the test",
    ))
}

pub(super) async fn login(
    State(inner): State<Arc<Inner>>,
    Extension(RequestBody(bytes)): Extension<RequestBody>,
) -> Reply {
    let req: LoginRequest = body(&bytes)?;
    let expires = inner.now() + jiff::SignedDuration::from_secs(BOOTSTRAP_TTL as i64);
    let mut state = inner.state.lock().unwrap();
    let good = state
        .users
        .get(&req.handle)
        .is_some_and(|u| u.password == req.password);
    if !good {
        return Err(Problem::unauthorized("wrong handle or password"));
    }
    let token = crate::random::token(16);
    state.bootstrap.insert(token.clone(), (req.handle, expires));
    ok(&BootstrapResponse {
        bootstrap_token: token,
        token_type: BOOTSTRAP_TOKEN_TYPE.to_owned(),
        expires_in: BOOTSTRAP_TTL,
    })
}

/// `POST /api/auth/device-keys` with a bootstrap token, consumed on
/// success.
pub(super) async fn register_key(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Extension(RequestBody(bytes)): Extension<RequestBody>,
) -> Reply {
    let Caller::Bearer(token) = caller else {
        return Err(Problem::unauthorized("a bootstrap token is needed"));
    };
    let now = inner.now();
    let mut state = inner.state.lock().unwrap();
    let handle = match state.bootstrap.get(&token) {
        Some((handle, expires)) if *expires > now => handle.clone(),
        _ => return Err(Problem::unauthorized("the bootstrap token is not valid")),
    };
    let req: DeviceKeyRegistration = body(&bytes)?;
    if req
        .algorithm
        .as_ref()
        .is_some_and(|a| *a != KeyAlgorithm::Ed25519)
    {
        return Err(Problem::bad_request("algorithm must be Ed25519"));
    }
    let key = decode_public_key(&req.public_key)
        .map_err(|_| Problem::bad_request("public_key is not a raw Ed25519 key"))?;
    state.bootstrap.remove(&token);
    let key_id = format!("dk_{}", crate::random::token(12));
    state.keys.insert(
        key_id.clone(),
        DeviceKey {
            handle,
            key,
            public_key: req.public_key,
            device_name: req.device_name,
            created_at: now,
            revoked: false,
        },
    );
    created(&DeviceKeyCreated {
        key_id,
        created_at: now,
    })
}

pub(super) async fn list_keys(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
) -> Reply {
    let actor = caller.local(&inner)?;
    let state = inner.state.lock().unwrap();
    let keys = state
        .keys
        .iter()
        .filter(|(_, k)| !k.revoked && k.handle == actor.handle())
        .map(|(id, k)| DeviceKeyInfo {
            key_id: id.clone(),
            algorithm: KeyAlgorithm::Ed25519,
            public_key: k.public_key.clone(),
            device_name: k.device_name.clone(),
            created_at: k.created_at,
        })
        .collect();
    ok(&DeviceKeysResponse { keys })
}

pub(super) async fn revoke_key(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path(key_id): Path<String>,
) -> Reply {
    let actor = caller.local(&inner)?;
    let mine = {
        let state = inner.state.lock().unwrap();
        state
            .keys
            .get(&key_id)
            .is_some_and(|k| !k.revoked && k.handle == actor.handle())
    };
    if !mine || !inner.revoke_key(&key_id) {
        return Err(Problem::not_found("no such active key of yours"));
    }
    no_content()
}
