//! Adding a social account, signing it in again, and signing it out
//! (docs/ofscp.md, "Accounts and sign-in").

use std::sync::Arc;
use std::time::Duration;

use postrinch_ofscp::ofscp_core::Actor;
use postrinch_ofscp::ofscp_core::wire::{DiscoveryDocument, ErrorCode};
use postrinch_ofscp::{Endpoint, Identity};
use postrinch_store::{SocialAccount, SocialAccountState};

use crate::sync::Synced;
use crate::{Social, SocialError, SocialResult, parse_actor, refused};

/// What discovery found for an actor, to show the person before they give
/// the password.
#[derive(Debug, Clone)]
pub struct Found {
    /// The actor, canonical.
    pub actor: Actor,
    /// Its home provider's discovery document (§3.1).
    pub discovery: DiscoveryDocument,
}

/// An account added and synced.
#[derive(Debug, Clone)]
pub struct Added {
    pub account: SocialAccount,
    pub synced: Synced,
}

/// What became of this computer's key when an account was removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Revocation {
    /// The provider revoked it.
    Revoked,
    /// It was revoked already, or there was none to revoke.
    AlreadyGone,
    /// The provider could not be asked; the key stays registered there
    /// until revoked from another device. Why, for the person.
    Failed(String),
}

/// An account removed from this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    pub revocation: Revocation,
}

impl Social {
    /// Find `actor`'s home provider (§3.1), before asking for a password.
    /// An actor already added is refused.
    pub async fn look_up(&self, actor: &str) -> SocialResult<Found> {
        let actor = parse_actor(actor)?;
        self.refuse_if_added(&actor).await?;
        let discovery = self
            .transport
            .provider(Endpoint::home_of(&actor))
            .discover()
            .await?;
        Ok(Found { actor, discovery })
    }

    /// Add the account [`Social::look_up`] found: sign in with `password`
    /// (sent once, never kept), register a new device key and keep it in
    /// the keychain, then run the first sync. If any step fails, nothing
    /// is kept: the key is revoked again, and its keychain entry and
    /// whatever was stored are deleted.
    pub async fn add(&self, found: &Found, password: &str) -> SocialResult<Added> {
        let actor = &found.actor;
        self.refuse_if_added(actor).await?;
        let identity = Arc::new(self.sign_in(actor, password).await?);
        match self.keep_and_sync(&identity).await {
            Ok(added) => Ok(added),
            Err(e) => {
                tracing::warn!(%actor, error = %e, "adding failed; undoing it");
                self.revoke(&identity).await;
                if let Err(undo) = self.forget(actor).await {
                    tracing::warn!(%actor, error = %undo, "could not undo adding");
                }
                Err(e)
            }
        }
    }

    /// Keep the new account and its key, then sync it.
    async fn keep_and_sync(&self, identity: &Arc<Identity>) -> SocialResult<Added> {
        self.keep_key(identity).await?;
        let account = SocialAccount {
            actor: identity.actor().to_string(),
            state: SocialAccountState::Active,
            profile: None,
            settings: None,
            key_id: Some(identity.key_id().to_owned()),
        };
        let kept = account.clone();
        self.blocking(move |store| store.put_social_account(&kept))
            .await?;
        let synced = self.sync_as(&account, identity).await?;
        let account = self.account(identity.actor())?;
        Ok(Added { account, synced })
    }

    /// Sign `actor` in again with `password`: a new device key, kept in
    /// place of the old one, which is revoked if the provider still has
    /// it. The account is active again; sync it next.
    pub async fn sign_in_again(&self, actor: &str, password: &str) -> SocialResult<SocialAccount> {
        let actor = parse_actor(actor)?;
        let old = self.account(&actor)?;
        let identity = Arc::new(self.sign_in(&actor, password).await?);
        if let Err(e) = self.keep_key(&identity).await {
            self.revoke(&identity).await;
            return Err(e);
        }
        let key_id = identity.key_id().to_owned();
        let name = actor.to_string();
        let changed = self
            .blocking(move |store| {
                store.change_social_account(&name, |a| {
                    a.state = SocialAccountState::Active;
                    a.key_id = Some(key_id);
                })
            })
            .await?;
        let Some(account) = changed else {
            // Removed meanwhile: the new key is not wanted either.
            self.revoke(&identity).await;
            self.forget_key(&actor).await?;
            return Err(SocialError::NoAccount(actor.to_string()));
        };
        if let Some(old_key) = old.key_id.filter(|k| k != identity.key_id()) {
            let home = self.client(&identity, actor.domain());
            match home.revoke_device_key(&old_key).await {
                Ok(()) => {}
                Err(e) if refused::key_already_gone(&e) => {}
                Err(e) => tracing::warn!(%actor, error = %e, "could not revoke the old key"),
            }
        }
        Ok(account)
    }

    /// Sign `actor` out of this computer and remove it: revoke this
    /// computer's key (§4.7.2), then delete its keychain entry and
    /// everything stored for it. A key the provider cannot be asked to
    /// revoke does not keep the account here: the answer says so.
    pub async fn remove(&self, actor: &str) -> SocialResult<Removed> {
        let actor = parse_actor(actor)?;
        let account = self.account(&actor)?;
        let revocation = self.revoke_own(&account, &actor).await;
        self.forget(&actor).await?;
        Ok(Removed { revocation })
    }

    /// Revoke the key `account` signs with, as far as that can be done.
    async fn revoke_own(&self, account: &SocialAccount, actor: &Actor) -> Revocation {
        let Some(key_id) = &account.key_id else {
            return Revocation::AlreadyGone;
        };
        let entry = match self.kept_key(actor).await {
            Ok(entry) => entry,
            Err(e) => return Revocation::Failed(e.to_string()),
        };
        let identity = match entry.map(|e| crate::keys::identity_from(actor, key_id, &e)) {
            Some(Ok(Some(identity))) => Arc::new(identity),
            // Without its private key, the key cannot sign its revocation.
            Some(Ok(None)) | None => {
                return Revocation::Failed(format!(
                    "this computer no longer has the key {key_id}; revoke it from another device"
                ));
            }
            Some(Err(e)) => return Revocation::Failed(e.to_string()),
        };
        match self.client(&identity, actor.domain()).sign_out().await {
            Ok(()) => Revocation::Revoked,
            Err(e) if refused::key_already_gone(&e) => Revocation::AlreadyGone,
            Err(e) => Revocation::Failed(e.to_string()),
        }
    }

    /// Log in and register a new key (§4.1.2, §4.3).
    async fn sign_in(&self, actor: &Actor, password: &str) -> SocialResult<Identity> {
        let provider = self.transport.provider(Endpoint::home_of(actor));
        match provider
            .sign_in(actor.handle(), password, &self.device_name)
            .await
        {
            Ok(identity) => Ok(identity),
            Err(e) if e.problem().is_some_and(|p| p.is(&ErrorCode::Unauthorized)) => {
                Err(SocialError::WrongPassword {
                    actor: actor.to_string(),
                })
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Revoke a key just registered, as an undo; a failure is only logged,
    /// as the key is unused and its private half is about to be deleted.
    async fn revoke(&self, identity: &Arc<Identity>) {
        let actor = identity.actor();
        if let Err(e) = self.client(identity, actor.domain()).sign_out().await {
            tracing::warn!(%actor, error = %e, "could not revoke the new key");
        }
    }

    /// Delete `actor`'s keychain entry and everything stored for it.
    async fn forget(&self, actor: &Actor) -> SocialResult<()> {
        self.forget_key(actor).await?;
        let name = actor.to_string();
        self.blocking(move |store| {
            store.remove_social_account(&name)?;
            store.collect_garbage(Duration::ZERO).map(|_| ())
        })
        .await
    }

    async fn refuse_if_added(&self, actor: &Actor) -> SocialResult<()> {
        let name = actor.to_string();
        let kept = self
            .blocking(move |store| store.social_account(&name))
            .await?;
        match kept {
            Some(_) => Err(SocialError::AlreadyAdded(actor.to_string())),
            None => Ok(()),
        }
    }
}
