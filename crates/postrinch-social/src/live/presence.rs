//! Presence (§6.4, §7.5), kept in memory only (docs/ofscp.md, "Storage").
//!
//! An account watches the people it has private conversations with, and
//! anyone the window asks for ([`Social::watch_presence`]). A user's
//! presence is subscribed on a socket to their home provider (§7.5: only
//! users a provider hosts), so it is watched live where the account has a
//! session: on its home provider, and on the providers of its groups and
//! followed channels (docs/ofscp.md, "Sessions"). When a session ends, the
//! presence it gave is no longer known.

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

use postrinch_ofscp::ofscp_core::wire::{DmConversation, Presence};
use postrinch_ofscp::ofscp_core::ws::{Command, Event, PresenceUsers};
use postrinch_ofscp::ofscp_core::{Actor, Authority};
use postrinch_ofscp::ws::RequestError;
use postrinch_store::{PlaceKind, social_account_key};

use super::session::Live;
use super::{Control, LiveEvent};
use crate::{Social, SocialResult, parse_actor};

/// Every account's watched people and what is known of their presence.
#[derive(Default)]
pub(crate) struct Presences {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// By account key, then by user (lower-cased).
    seen: HashMap<String, HashMap<String, Presence>>,
    /// By account key: the people asked for.
    asked: HashMap<String, BTreeSet<Actor>>,
}

impl Presences {
    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Forget what `account`'s sessions saw: it went offline.
    pub(crate) fn forget(&self, account: &str) {
        self.inner().seen.remove(account);
    }

    /// Forget what `account`'s session with `authority` saw: it ended.
    pub(crate) fn lost(&self, account: &str, authority: &Authority) {
        if let Some(seen) = self.inner().seen.get_mut(account) {
            let suffix = format!("@{authority}").to_ascii_lowercase();
            seen.retain(|user, _| !user.ends_with(&suffix));
        }
    }

    fn set(&self, account: &str, user: &Actor, presence: Presence) {
        self.inner()
            .seen
            .entry(account.to_owned())
            .or_default()
            .insert(social_account_key(&user.to_string()), presence);
    }

    fn asked(&self, account: &str) -> BTreeSet<Actor> {
        self.inner().asked.get(account).cloned().unwrap_or_default()
    }
}

impl Social {
    /// What is known of `user`'s presence, as `actor`'s sessions saw it;
    /// `None` while it is not known (not watched, or its provider not
    /// connected).
    pub fn presence(&self, actor: &str, user: &str) -> Option<Presence> {
        let account = social_account_key(&parse_actor(actor).ok()?.to_string());
        let user = social_account_key(&parse_actor(user).ok()?.to_string());
        let inner = self.live.presences.inner();
        inner.seen.get(&account)?.get(&user).cloned()
    }

    /// Watch `users`' presence for `actor`, besides the people it has
    /// private conversations with.
    pub fn watch_presence(&self, actor: &str, users: &[&str]) -> SocialResult<()> {
        let account = social_account_key(&parse_actor(actor)?.to_string());
        let users = users
            .iter()
            .map(|u| parse_actor(u))
            .collect::<SocialResult<Vec<_>>>()?;
        self.live
            .presences
            .inner()
            .asked
            .entry(account.clone())
            .or_default()
            .extend(users);
        self.live.control(&account, Control::Watch);
        Ok(())
    }
}

impl Live<'_> {
    /// Subscribe the presence of the watched people this provider hosts
    /// that are not subscribed yet. Snapshots follow as `presence.update`.
    pub(crate) async fn watch(&mut self) -> SocialResult<()> {
        let mut users = self.cx.social.live.presences.asked(&self.cx.key);
        users.extend(self.partners().await?);
        let me = self.cx.identity.actor();
        let new: Vec<Actor> = users
            .into_iter()
            .filter(|u| u.domain() == &self.cx.authority && u != me)
            .filter(|u| !self.watching.contains(u))
            .collect();
        if new.is_empty() {
            return Ok(());
        }
        let command = Command::PresenceSubscribe(PresenceUsers { users: new });
        match self.conn.request(command).await {
            Ok(envelope) => {
                if let Event::PresenceSubscribed(ack) = envelope.body {
                    self.watching.extend(ack.users);
                }
                Ok(())
            }
            Err(RequestError::Rejected(error)) => {
                tracing::info!(provider = %self.cx.authority, %error.message, "presence refused");
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    /// The other people of the account's private conversations.
    async fn partners(&self) -> SocialResult<Vec<Actor>> {
        let account = self.cx.actor();
        let places = self
            .cx
            .social
            .blocking(move |store| store.social_places(&account))
            .await?;
        let me = self.cx.identity.actor();
        Ok(places
            .into_iter()
            .filter(|p| p.kind == PlaceKind::Conversation)
            .filter_map(|p| serde_json::from_value::<DmConversation>(p.object).ok())
            .flat_map(|c| c.participants)
            .filter(|u| u != me)
            .collect())
    }

    /// Keep and tell a presence the provider sent.
    pub(crate) fn presence_changed(&self, user: Actor, presence: Presence) {
        self.cx
            .social
            .live
            .presences
            .set(&self.cx.key, &user, presence.clone());
        self.cx.tell(LiveEvent::Presence {
            account: self.cx.actor(),
            user: user.to_string(),
            presence,
        });
    }
}
