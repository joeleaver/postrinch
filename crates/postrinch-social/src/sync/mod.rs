//! The first sync of an account (docs/ofscp.md, "Sync engine"; the plan's
//! PR 8): what a new computer needs to show the account.
//!
//! 1. The home provider: its discovery document and clock, the person's
//!    profile and notification settings, and the read markers (§7.7).
//! 2. The places ([`places`]): every readable channel of every group the
//!    home provider lists the person in (§6.5), on whichever provider
//!    hosts it; channels followed without joining (§7.6); and private
//!    conversations (§7.4).
//! 3. Each place's newest page of messages ([`pages`]), read or unread as
//!    the read marker says. History beyond it is fetched on scrolling, and
//!    what happens next arrives live (the plan's PR 9).
//!
//! Writes are upserts by id, so a sync run again refreshes what it finds
//! and keeps what is local to a place (its cursors, its notices). A group
//! or channel the person can no longer read (`403`, `404`) is left out
//! rather than stopping the sync, and what the home provider no longer
//! lists is forgotten ([`gone`]); a refused signature stops it and puts
//! the account in the state that says why.

mod gone;
mod pages;
mod places;

pub(crate) mod message;

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use postrinch_ofscp::http::Client;
use postrinch_ofscp::ofscp_core::Authority;
use postrinch_ofscp::{Endpoint, Identity};
use postrinch_store::{SocialAccount, SocialProvider};

use crate::{Social, SocialError, SocialResult, parse_actor, refused};

/// What a sync kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Synced {
    /// Groups the person is in.
    pub groups: usize,
    /// Channels and conversations.
    pub places: usize,
    /// Messages kept or refreshed.
    pub messages: usize,
    /// Groups and channels left out: no longer there, or not readable.
    pub skipped: usize,
}

impl Social {
    /// Sync `actor`'s account: its profile, groups, places and each
    /// place's newest page. An account that must sign in again (or whose
    /// clock is off) is refused with the reason.
    pub async fn sync(&self, actor: &str) -> SocialResult<Synced> {
        let actor = parse_actor(actor)?;
        let account = self.account(&actor)?;
        let identity = self.identity(&account).await?;
        self.sync_as(&account, &identity).await
    }

    /// Sync `account`, signing as `identity`.
    pub(crate) async fn sync_as(
        &self,
        account: &SocialAccount,
        identity: &Arc<Identity>,
    ) -> SocialResult<Synced> {
        let mut run = Run {
            social: self,
            identity: Arc::clone(identity),
            home: self.client(identity, identity.actor().domain()),
            providers: BTreeSet::new(),
            markers: HashMap::new(),
            synced: Synced::default(),
        };
        run.providers.insert(identity.actor().domain().clone());
        run.home(account).await?;
        let targets = run.places().await?;
        for target in &targets {
            run.page(target).await?;
        }
        run.record_providers().await?;
        Ok(run.synced)
    }
}

/// One sync under way.
pub(crate) struct Run<'a> {
    social: &'a Social,
    identity: Arc<Identity>,
    /// The signed client for the home provider.
    home: Client,
    /// Every provider talked to, whose discovery and clock are kept.
    providers: BTreeSet<Authority>,
    /// The home provider's read markers, by scope id (a channel's local
    /// id, or a dmId): the newest message read, `None` for none (§7.7).
    markers: HashMap<String, Option<String>>,
    synced: Synced,
}

impl Run<'_> {
    /// The account's actor, as stored.
    fn actor(&self) -> String {
        self.identity.actor().to_string()
    }

    /// The signed client for `authority`, noting the provider as one
    /// talked to.
    fn client_for(&mut self, authority: &Authority) -> Client {
        self.providers.insert(authority.clone());
        self.social.client(&self.identity, authority)
    }

    /// A signed request's answer, a refused signature marking the account.
    async fn checked<T>(
        &self,
        client: &Client,
        result: postrinch_ofscp::Result<T>,
    ) -> SocialResult<T> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => match refused::account_state(client, &error) {
                Some(state) => {
                    tracing::warn!(actor = %self.actor(), ?state, %error, "signature refused");
                    Err(self.social.mark(self.identity.actor(), state).await?)
                }
                None => Err(SocialError::Transport(error)),
            },
        }
    }

    /// A signed request's answer, or `None` for something out of reach
    /// (counted as skipped).
    async fn reachable<T>(
        &mut self,
        client: &Client,
        result: postrinch_ofscp::Result<T>,
    ) -> SocialResult<Option<T>> {
        match result {
            Err(error) if refused::is_out_of_reach(&error) => {
                tracing::info!(actor = %self.actor(), %error, "left out of the sync");
                self.synced.skipped += 1;
                Ok(None)
            }
            result => self.checked(client, result).await.map(Some),
        }
    }

    /// The person's profile, settings and read markers, from home.
    async fn home(&mut self, account: &SocialAccount) -> SocialResult<()> {
        let home = self.home.clone();
        let me = self.checked(&home, home.me().await).await?;
        let notices = self
            .checked(&home, home.notification_settings().await)
            .await?;
        let markers = self.checked(&home, home.read_markers().await).await?;
        self.markers = markers
            .scopes
            .into_iter()
            .map(|s| {
                let read = s.last_read_message_id.map(|id| id.to_string());
                (s.scope_id.to_string(), read)
            })
            .collect();
        let profile = serde_json::to_value(&me.profile).ok();
        let settings = serde_json::to_value(&notices).ok();
        let name = account.actor.clone();
        self.social
            .blocking(move |store| {
                store.change_social_account(&name, |a| {
                    a.profile = profile;
                    a.settings = settings;
                })
            })
            .await?;
        Ok(())
    }

    /// Keep each provider talked to: its discovery document and how far
    /// its clock is from this one's.
    async fn record_providers(&mut self) -> SocialResult<()> {
        for authority in std::mem::take(&mut self.providers) {
            let provider = self
                .social
                .transport
                .provider(Endpoint::new(authority.clone()));
            let discovery = match provider.discover().await {
                Ok(discovery) => discovery,
                // Kept for display only: a provider that served the sync
                // but not its document now is tried again next sync.
                Err(error) => {
                    tracing::info!(%authority, %error, "no discovery document");
                    continue;
                }
            };
            let record = SocialProvider {
                domain: authority.to_string(),
                discovery: serde_json::to_value(&discovery).unwrap_or_default(),
                capabilities: serde_json::to_value(&discovery.capabilities).unwrap_or_default(),
                clock_offset_ms: provider.clock().offset_secs().saturating_mul(1000),
            };
            self.social
                .blocking(move |store| store.put_social_provider(&record))
                .await?;
        }
        Ok(())
    }
}
