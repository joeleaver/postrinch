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
//! A request refused for its signature (a revoked key, or a clock too far
//! off) puts the account in the state that says so
//! ([`SocialAccountState`]), as mail does.
//!
//! Live sessions, resuming and scrolling back, and the outbox come next
//! (the plan's PRs 9 and 10).

mod accounts;
mod error;
mod keys;
mod refused;
mod sync;
mod uris;

use std::sync::Arc;

use postrinch_ofscp::http::Client;
use postrinch_ofscp::ofscp_core::{Actor, Authority};
use postrinch_ofscp::{Endpoint, Identity, Transport};
use postrinch_store::{SocialAccount, SocialAccountState, Store};

pub use accounts::{Added, Found, Removed, Revocation};
pub use error::{SocialError, SocialResult};
pub use keys::DeviceKeys;
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

    /// The account of `actor`, in any spelling.
    fn account(&self, actor: &Actor) -> SocialResult<SocialAccount> {
        self.store
            .social_account(&actor.to_string())?
            .ok_or_else(|| SocialError::NoAccount(actor.to_string()))
    }

    /// The identity `account` signs as: its keychain key, if it is the one
    /// registered. Without one the account needs to sign in, and is marked
    /// so.
    async fn identity(&self, account: &SocialAccount) -> SocialResult<Arc<Identity>> {
        let actor = parse_actor(&account.actor)?;
        if account.state != SocialAccountState::Active {
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
                Err(self.mark(&actor, SocialAccountState::NeedsSignIn).await?)
            }
        }
    }

    /// Put `actor`'s account in `state`, and give the error that says so.
    async fn mark(&self, actor: &Actor, state: SocialAccountState) -> SocialResult<SocialError> {
        let store = self.store.clone();
        let name = actor.to_string();
        tokio::task::spawn_blocking(move || {
            store.change_social_account(&name, |a| a.state = state)
        })
        .await??;
        Ok(SocialError::Unusable {
            actor: actor.to_string(),
            state,
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
