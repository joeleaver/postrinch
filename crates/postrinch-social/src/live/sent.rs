//! The messages the person sent to someone on another provider, followed
//! there (§7.4 *Following your sent messages on the counterparty's
//! provider*): that provider holds them, so their edits, deletes and
//! reactions happen there, and only a `dmId` subscription there brings
//! them live.
//!
//! Each is kept in the conversation under its URL at that provider
//! (docs/ofscp.md, "Private messages"), and its event cursor is that
//! provider's, kept apart from home's ([`Resume::Sent`]).

use postrinch_ofscp::ofscp_core::ws::DmMessage;
use postrinch_ofscp::ofscp_core::{DmId, LocalId};
use postrinch_store::social_account_key;

use super::apply::{self, Arrival, Keeper, Resume};
use super::session::Live;
use super::subscribe::Scope;
use crate::SocialResult;
use crate::uris::dm_message_url;

impl Live<'_> {
    /// A message of a conversation subscribed here as [`Scope::Sent`]: new,
    /// edited or deleted (§7.4 re-sends edits and deletes as `dm.message`).
    /// Anything else is not the person's to keep from here.
    pub(crate) async fn sent_message(&mut self, event: DmMessage) -> SocialResult<()> {
        let Some(place) = self.sent_place(&event.dm_id) else {
            return Ok(());
        };
        // This provider holds only what was written to its user, so the
        // person is the author of all it may send here.
        let me = social_account_key(&self.cx.actor());
        if social_account_key(&event.message.author.to_string()) != me {
            tracing::warn!(provider = %self.cx.authority, place = %place, author = %event.message.author, "a message not the person's, from a provider that holds only theirs");
            return Ok(());
        }
        let cursor = event.cursor.map(Resume::Sent);
        let (arrival, cursor) = self.arrival(&place, Arrival::Any, cursor);
        apply::message(
            &self.keeper_at_home(),
            place,
            *event.message,
            cursor,
            arrival,
        )
        .await
    }

    /// Read again, all of them, the reactions on a message the person sent
    /// in a conversation subscribed here as [`Scope::Sent`].
    pub(crate) async fn sent_reactions(&self, dm: &DmId, message: &LocalId) -> SocialResult<()> {
        let Some(place) = self.sent_place(dm) else {
            return Ok(());
        };
        let all = self.all_dm_reactions(dm, message).await?;
        let kept = dm_message_url(&self.cx.authority, &place, message.as_str());
        apply::reactions(&self.keeper_at_home(), place, kept, all).await
    }

    /// The place of `dm` when it is subscribed on this connection as the
    /// messages the person sent.
    fn sent_place(&self, dm: &DmId) -> Option<String> {
        let id = LocalId::new(dm.as_str()).ok()?;
        match self.subscribed.get(&id)? {
            Scope::Sent { place, .. } => Some(place.clone()),
            Scope::Channel { .. } | Scope::Conversation { .. } => None,
        }
    }

    /// What keeps a conversation's messages: the conversation is home's
    /// place, whichever provider holds each message.
    fn keeper_at_home(&self) -> Keeper<'_> {
        Keeper {
            social: &self.cx.social,
            actor: self.cx.identity.actor(),
            host: self.cx.identity.actor().domain(),
        }
    }
}
