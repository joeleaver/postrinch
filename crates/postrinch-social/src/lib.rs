//! Postrinch's OFSCP sync engine (docs/ofscp.md, "Sync engine"): social
//! accounts and what they hold, on top of the transport
//! ([`postrinch_ofscp`]) and into the store ([`postrinch_store`]).
//!
//! This is its first part (the plan's PR 8):
//!
//! - **Adding an account** ([`Social::look_up`], then [`Social::add`]):
//!   discovery first, so the person sees which provider they are signing
//!   in to before giving the password; then the password, sent once and
//!   never kept; a new device key, registered as "Postrinch on
//!   {hostname}" and kept in the keychain ([`DeviceKeys`]); then the first
//!   sync. Nothing is kept unless every step passes.
//! - **Signing in again** ([`Social::sign_in_again`]) when the account
//!   needs it: a new key, the old one revoked.
//! - **Signing out** ([`Social::remove`]): this computer's key revoked,
//!   then its keychain entry and everything stored for the account
//!   deleted.
//! - **The first sync** ([`Social::sync`]): the profile, the provider, the
//!   groups and their channels, followed channels, private conversations,
//!   and each place's newest page of messages, read as the home provider's
//!   read markers say.
//!
//! A request to the home provider refused for its signature (a revoked
//! key, or a clock too far off) puts the account in the state that says so
//! ([`SocialAccountState`]), as mail does. An account whose clock was off
//! may sync again: the transport signs on the provider's clock, and a sync
//! the home provider answers makes it active again.
//!
//! Each account's operations run one at a time: adding, signing in again,
//! syncing and removing it wait for one another.
//!
//! Its second part (the plan's PR 9) keeps accounts live ([`Social::go_live`],
//! [`Social::keep_connected`]): a socket to each provider an account has
//! places on, connected again with backoff, resuming from each place's
//! cursor and reading over REST what a resume cannot give; read markers,
//! notifications and presence as they change. What changed is told on
//! [`Social::live_events`].
//!
//! Its third part (the plan's PR 10) is the outbox ([`Social::send_outbox`]):
//! the posts, edits, deletes, reactions, read markers and follows the
//! commands queue in the store, sent in order, per place, and kept as their
//! providers answer. A live account sends by itself, and again with
//! backoff after a failure; [`Social::outbox_changed`] says there is more.

mod accounts;
mod error;
mod keys;
mod live;
mod locks;
mod outbox;
mod refused;
mod sync;
mod uris;

use std::sync::Arc;

use postrinch_ofscp::http::Client;
use postrinch_ofscp::ofscp_core::{Actor, Authority};
use postrinch_ofscp::{Endpoint, Identity, Transport};
use postrinch_store::{SocialAccount, SocialAccountState, Store, social_account_key};
use tokio::sync::OwnedMutexGuard;

pub use accounts::{Added, Found, Removed, Revocation};
pub use error::{SocialError, SocialResult};
pub use keys::DeviceKeys;
pub use live::{LiveEvent, LiveOptions, MessageChange, SessionStatus};
pub use outbox::Sent;
pub use postrinch_sync_core::Keychain;
pub use sync::Synced;

/// The social engine: the store, the transport every provider is reached
/// through, and where device keys are kept. Cheap to clone.
#[derive(Clone)]
pub struct Social {
    store: Store,
    transport: Transport,
    keys: Arc<dyn DeviceKeys>,
    device_name: String,
    /// One lock per account (by its store key), held for the whole of an
    /// operation on it.
    locks: locks::Locks,
    /// One lock per account, held for the whole of a pass over its outbox,
    /// so two passes never send the same change at once.
    passes: locks::Locks,
    /// The accounts kept live, and where what changed is told.
    live: Arc<live::Registry>,
}

impl Social {
    /// The engine over `store`, reaching providers through `transport`,
    /// keeping device keys in `keys` ([`Keychain::OFSCP`] outside tests).
    /// Keys are registered as "Postrinch on {this computer's name}".
    pub fn new(store: Store, transport: Transport, keys: Arc<dyn DeviceKeys>) -> Self {
        let host = gethostname::gethostname();
        let host = host.to_string_lossy();
        let device_name = if host.trim().is_empty() {
            "Postrinch".to_owned()
        } else {
            format!("Postrinch on {}", host.trim())
        };
        Self {
            store,
            transport,
            keys,
            device_name,
            locks: locks::Locks::default(),
            passes: locks::Locks::default(),
            live: Arc::new(live::Registry::new(LiveOptions::default())),
        }
    }

    /// The same, registering keys as `name`.
    pub fn with_device_name(mut self, name: impl Into<String>) -> Self {
        self.device_name = name.into();
        self
    }

    /// The name new device keys are registered under.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// The store.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Wait for, then hold, `actor`'s lock: no other operation on the
    /// account runs until the guard is dropped.
    async fn hold(&self, actor: &Actor) -> OwnedMutexGuard<()> {
        self.locks
            .hold(social_account_key(&actor.to_string()))
            .await
    }

    /// The account of `actor`, in any spelling.
    async fn account(&self, actor: &Actor) -> SocialResult<SocialAccount> {
        let name = actor.to_string();
        self.blocking(move |store| store.social_account(&name))
            .await?
            .ok_or_else(|| SocialError::NoAccount(actor.to_string()))
    }

    /// The identity `account` signs as: its keychain key, if it is the one
    /// registered. Without one the account needs to sign in, and is marked
    /// so. An account whose clock was off is let try again.
    async fn identity(&self, account: &SocialAccount) -> SocialResult<Arc<Identity>> {
        let actor = parse_actor(&account.actor)?;
        if account.state == SocialAccountState::NeedsSignIn {
            return Err(SocialError::Unusable {
                actor: actor.to_string(),
                state: account.state,
            });
        }
        let entry = match &account.key_id {
            Some(_) => self.kept_key(&actor).await?,
            None => None,
        };
        let identity = match (&account.key_id, entry) {
            (Some(key_id), Some(entry)) => keys::identity_from(&actor, key_id, &entry)?,
            _ => None,
        };
        match identity {
            Some(identity) => Ok(Arc::new(identity)),
            None => {
                tracing::warn!(%actor, "no device key kept for the registered key id");
                let unusable = SocialError::Unusable {
                    actor: actor.to_string(),
                    state: SocialAccountState::NeedsSignIn,
                };
                let key_id = account.key_id.clone();
                Err(self
                    .mark(&actor, key_id, SocialAccountState::NeedsSignIn, unusable)
                    .await?)
            }
        }
    }

    /// Put `actor`'s account in `state` if it still signs with `key_id`
    /// (another process may have signed it in again meanwhile), and give
    /// the error that says so; `otherwise` when it signs with another key.
    async fn mark(
        &self,
        actor: &Actor,
        key_id: Option<String>,
        state: SocialAccountState,
        otherwise: SocialError,
    ) -> SocialResult<SocialError> {
        let name = actor.to_string();
        let marked = self
            .blocking(move |store| {
                let mut marked = false;
                store.change_social_account(&name, |a| {
                    if a.key_id == key_id {
                        a.state = state;
                        marked = true;
                    }
                })?;
                Ok(marked)
            })
            .await?;
        Ok(if marked {
            SocialError::Unusable {
                actor: actor.to_string(),
                state,
            }
        } else {
            otherwise
        })
    }

    /// `identity`'s signed client for the provider at `authority`.
    fn client(&self, identity: &Arc<Identity>, authority: &Authority) -> Client {
        self.transport
            .client(Arc::clone(identity), Endpoint::new(authority.clone()))
    }

    /// The keychain entry kept for `actor`.
    async fn kept_key(&self, actor: &Actor) -> SocialResult<Option<String>> {
        let keys = Arc::clone(&self.keys);
        let key = keys::key_of(actor);
        Ok(tokio::task::spawn_blocking(move || keys.get(&key)).await??)
    }

    /// Keep `identity`'s key in the keychain.
    async fn keep_key(&self, identity: &Identity) -> SocialResult<()> {
        let keys = Arc::clone(&self.keys);
        let key = keys::key_of(identity.actor());
        let entry = keys::entry_for(identity);
        Ok(tokio::task::spawn_blocking(move || keys.set(&key, &entry)).await??)
    }

    /// Delete `actor`'s keychain entry.
    async fn forget_key(&self, actor: &Actor) -> SocialResult<()> {
        let keys = Arc::clone(&self.keys);
        let key = keys::key_of(actor);
        Ok(tokio::task::spawn_blocking(move || keys.delete(&key)).await??)
    }

    /// Run a store call on a blocking thread.
    async fn blocking<T: Send + 'static>(
        &self,
        call: impl FnOnce(&Store) -> postrinch_store::StoreResult<T> + Send + 'static,
    ) -> SocialResult<T> {
        let store = self.store.clone();
        Ok(tokio::task::spawn_blocking(move || call(&store)).await??)
    }
}

/// An actor as the person typed it, made canonical (§2.2: the domain
/// lower-cased, its default port dropped).
pub(crate) fn parse_actor(input: &str) -> SocialResult<Actor> {
    Actor::canonicalize(input.trim()).map_err(|source| SocialError::NotAnActor {
        input: input.to_owned(),
        source,
    })
}
