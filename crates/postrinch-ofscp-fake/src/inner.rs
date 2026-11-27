//! The shared core of one fake provider, behind its HTTP routes, its
//! WebSocket and the test-facing handle.

use std::sync::{Arc, Mutex};

use jiff::{SignedDuration, Timestamp};
use ofscp_core::signing::VerifyingKey;
use ofscp_core::{Actor, Authority};

use crate::clock::{Clock, whole_seconds};
use crate::faults::FaultState;
use crate::record::RecordedRequest;
use crate::server::ConnRegistry;
use crate::state::State;
use crate::verify::keys::KeyResolver;
use crate::verify::replay::ReplayStore;
use crate::ws::hub::Hub;
use crate::ws::typing::Typing;

/// Settings a test may change; the defaults are what the provider
/// advertises and enforces.
#[derive(Debug, Clone)]
pub(crate) struct Settings {
    /// `capabilities.limits.maxUploadBytes` (§3.1, §5.8).
    pub(crate) max_upload_bytes: u64,
    /// How long the keys endpoint lets verifiers cache (§4.6 `cache_until`).
    pub(crate) key_cache_ttl: SignedDuration,
    /// `permissions.editUntil` after creation for chat lines and memos;
    /// articles get none (§7.1 *Editing*).
    pub(crate) edit_window: SignedDuration,
    /// The per-connection subscription cap (§7.1 *Limits*).
    pub(crate) subscription_cap: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_upload_bytes: 8 * 1024 * 1024,
            key_cache_ttl: SignedDuration::from_hours(1),
            edit_window: SignedDuration::from_hours(1),
            subscription_cap: 500,
        }
    }
}

pub(crate) struct Inner {
    pub(crate) authority: Authority,
    pub(crate) clock: Clock,
    pub(crate) faults: Mutex<FaultState>,
    pub(crate) settings: Mutex<Settings>,
    pub(crate) state: Mutex<State>,
    pub(crate) hub: Hub,
    pub(crate) replay: ReplayStore,
    pub(crate) resolver: KeyResolver,
    pub(crate) log: Mutex<Vec<RecordedRequest>>,
    pub(crate) conns: Arc<ConnRegistry>,
    pub(crate) typing: Typing,
}

impl Inner {
    pub(crate) fn new(authority: Authority) -> Self {
        Self {
            authority,
            clock: Clock::default(),
            faults: Mutex::default(),
            settings: Mutex::default(),
            state: Mutex::default(),
            hub: Hub::default(),
            replay: ReplayStore::default(),
            resolver: KeyResolver::new(),
            log: Mutex::default(),
            conns: Arc::default(),
            typing: Typing::default(),
        }
    }

    /// Now on this provider's clock, in whole seconds as written on the
    /// wire.
    pub(crate) fn now(&self) -> Timestamp {
        whole_seconds(self.clock.now())
    }

    pub(crate) fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// `https://{authority}`.
    pub(crate) fn base_url(&self) -> String {
        format!("https://{}", self.authority.as_str())
    }

    /// The active key `key_id` of a local actor.
    pub(crate) fn local_key(&self, actor: &Actor, key_id: &str) -> Option<VerifyingKey> {
        let state = self.state.lock().unwrap();
        state
            .keys
            .get(key_id)
            .filter(|k| {
                !k.revoked && k.handle == actor.handle() && actor.is_local_to(&self.authority)
            })
            .map(|k| k.key)
    }

    /// Revoke a key (§4.7.1) and close its sockets with 4003.
    pub(crate) fn revoke_key(&self, key_id: &str) -> bool {
        let revoked = {
            let mut state = self.state.lock().unwrap();
            match state.keys.get_mut(key_id) {
                Some(k) if !k.revoked => {
                    k.revoked = true;
                    true
                }
                _ => false,
            }
        };
        if revoked {
            self.hub.close_key(key_id, ofscp_core::ws::CLOSE_REVOKED);
        }
        revoked
    }

    /// How many times each timeline event is delivered.
    pub(crate) fn event_copies(&self) -> usize {
        if self.faults.lock().unwrap().duplicate_events {
            2
        } else {
            1
        }
    }
}
