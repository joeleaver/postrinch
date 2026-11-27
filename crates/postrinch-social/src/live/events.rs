//! What a session does with each event its provider sends (§7.1 "Events").

use postrinch_ofscp::http::PageRequest;
use postrinch_ofscp::ofscp_core::wire::{DmConversation, Member};
use postrinch_ofscp::ofscp_core::ws::{Event, TypingState, Unsubscribed};
use postrinch_ofscp::ofscp_core::{Actor, DmId, LocalId};

use super::apply::{self, Arrival, collect};
use super::session::{End, Live, ends_session};
use super::{LiveEvent, markers, notices};
use crate::sync::Run;
use crate::uris::{channel_uri, group_uri};
use crate::{SocialError, SocialResult};

impl Live<'_> {
    /// Keep what `event` says. Only what ends the session is an error: the
    /// account can no longer sign, or the socket failed. Anything else
    /// that fails is logged, and the event's change left for the next
    /// refresh or backfill.
    pub(crate) async fn handle(&mut self, event: Event) -> Result<(), End> {
        let kind = event.kind().to_owned();
        match self.dispatch(event).await {
            Ok(()) => Ok(()),
            Err(error) if ends_session(&error) => Err(error.into()),
            Err(error) => {
                tracing::warn!(provider = %self.cx.authority, event = %kind, %error, "an event was not kept");
                Ok(())
            }
        }
    }

    async fn dispatch(&mut self, event: Event) -> SocialResult<()> {
        let home = self.cx.is_home();
        match event {
            Event::MessageCreated(m) => {
                let place = self.channel(&m.group_id, &m.channel_id);
                let (arrival, cursor) = self.arrival(&place, Arrival::Any, m.cursor);
                apply::message(&self.cx.keeper(), place, *m.message, cursor, arrival).await?;
            }
            Event::MessageUpdated(m) => {
                let place = self.channel(&m.group_id, &m.channel_id);
                let (arrival, cursor) = self.arrival(&place, Arrival::IfKept, m.cursor);
                apply::message(&self.cx.keeper(), place, *m.message, cursor, arrival).await?;
            }
            Event::MessageDeleted(d) => {
                let place = self.channel(&d.group_id, &d.channel_id);
                let id = d.message_id.to_string();
                let (_, cursor) = self.arrival(&place, Arrival::IfKept, d.cursor);
                let at = d
                    .deleted_at
                    .unwrap_or_else(|| self.cx.client().provider().clock().now());
                apply::deleted(&self.cx.keeper(), place, id, at, cursor).await?;
            }
            Event::ReactionAdded(r) => {
                let message = local_id(r.reaction.reference.id.as_str())?;
                self.channel_reactions(&r.group_id, &r.channel_id, &message)
                    .await?;
            }
            Event::ReactionRemoved(r) => {
                self.channel_reactions(&r.group_id, &r.channel_id, &r.message_id)
                    .await?;
            }
            Event::ChannelTyping(t) => {
                let place = self
                    .subscribed
                    .get(&t.channel_id)
                    .map(|s| s.place().to_owned());
                if let Some(place) = place {
                    self.typing(place, &t.user, &t.state);
                }
            }
            Event::DmMessage(d) if home => {
                let place = d.dm_id.to_string();
                self.know_conversation(&d.dm_id).await?;
                let (arrival, cursor) = self.arrival(&place, Arrival::Any, d.cursor);
                apply::message(&self.cx.keeper(), place, *d.message, cursor, arrival).await?;
            }
            Event::DmConversation(c) if home => self.keep_conversation(&c.conversation).await?,
            Event::DmReaction(r) if home => self.dm_reactions(&r.dm_id, &r.message_id).await?,
            Event::DmTyping(t) => self.typing(t.dm_id.to_string(), &t.user, &t.state),
            Event::PresenceUpdate(p) => self.presence_changed(p.user, p.presence),
            Event::GroupsUpdated(_) if home => {
                if let Err(end) = self.refresh().await {
                    return Err(match end {
                        End::Unusable(state) => SocialError::Unusable {
                            actor: self.cx.actor(),
                            state,
                        },
                        End::Stop => SocialError::NoAccount(self.cx.actor()),
                        End::Retry(why) => SocialError::Task(why),
                    });
                }
            }
            Event::MemberUpdated(m) => self.member_updated(&m.group_id, &m.member).await?,
            Event::NotificationCreated(n) if home => {
                notices::created(self.cx, n.notification).await?;
            }
            Event::ReadUpdated(r) if home => markers::read_updated(self.cx, r.markers).await?,
            Event::Unsubscribed(u) => self.unsubscribed(u),
            Event::Error(error) => {
                tracing::warn!(provider = %self.cx.authority, code = %error.code, %error.message, "the provider reported an error");
            }
            // Pings are answered by the transport; replies to commands go to
            // their commands; calls are a later wave (docs/ofscp.md,
            // Decisions 2); and events for the home provider alone are not
            // taken from another.
            _ => {}
        }
        Ok(())
    }

    /// The place of a channel of this session's provider.
    fn channel(&self, group: &LocalId, channel: &LocalId) -> String {
        channel_uri(&self.cx.authority, group, channel)
    }

    /// Read a channel message's reactions again, all of them.
    async fn channel_reactions(
        &self,
        group: &LocalId,
        channel: &LocalId,
        message: &LocalId,
    ) -> SocialResult<()> {
        let client = self.cx.client();
        let mut all = Vec::new();
        let mut request = PageRequest::newest();
        loop {
            let found = client.reactions(group, channel, message, &request).await;
            let page = self.cx.checked(&client, found).await?;
            match collect(&mut all, page) {
                Some(next) => request = next,
                None => break,
            }
        }
        let place = self.channel(group, channel);
        apply::reactions(&self.cx.keeper(), place, message.to_string(), all).await
    }

    /// Read a private message's reactions again, at home, all of them.
    async fn dm_reactions(&self, dm: &DmId, message: &LocalId) -> SocialResult<()> {
        let client = self.cx.client();
        let mut all = Vec::new();
        let mut request = PageRequest::newest();
        loop {
            let found = client.dm_reactions(dm, message, &request).await;
            let page = self.cx.checked(&client, found).await?;
            match collect(&mut all, page) {
                Some(next) => request = next,
                None => break,
            }
        }
        apply::reactions(&self.cx.keeper(), dm.to_string(), message.to_string(), all).await
    }

    /// How an event in `place` is applied: as it came, or, while the place
    /// waits for its backfill, to kept messages only and without its
    /// cursor. A new message is left for the backfill, which reads back to
    /// what was kept before the gap.
    fn arrival(
        &self,
        place: &str,
        arrival: Arrival,
        cursor: Option<String>,
    ) -> (Arrival, Option<String>) {
        if self.unfilled.contains(place) {
            (Arrival::IfKept, None)
        } else {
            (arrival, cursor)
        }
    }

    /// Make sure the conversation `dm` is kept, reading it from home if it
    /// is new (someone wrote first), and subscribe it.
    async fn know_conversation(&mut self, dm: &DmId) -> SocialResult<()> {
        let (account, place) = (self.cx.actor(), dm.to_string());
        let kept = self
            .cx
            .social
            .blocking(move |store| store.social_place(&account, &place))
            .await?;
        if kept.is_some() {
            return Ok(());
        }
        let home = self.cx.home();
        let found = home.conversations(&PageRequest::newest()).await;
        let conversations = self.cx.checked(&home, found).await?;
        if let Some(conversation) = conversations.items.iter().find(|c| &c.id == dm) {
            self.keep_conversation(conversation).await?;
        }
        Ok(())
    }

    /// Keep a conversation as a place, and subscribe it.
    async fn keep_conversation(&mut self, conversation: &DmConversation) -> SocialResult<()> {
        Run::new(&self.cx.social, &self.cx.identity)
            .conversation(conversation)
            .await?;
        self.cx.tell(LiveEvent::Place {
            account: self.cx.actor(),
            place: conversation.id.to_string(),
        });
        self.resubscribe().await?;
        self.watch().await
    }

    /// The person's own membership changed: keep the new role.
    async fn member_updated(&self, group: &LocalId, member: &Member) -> SocialResult<()> {
        if &member.user != self.cx.identity.actor() {
            return Ok(());
        }
        let (account, uri) = (self.cx.actor(), group_uri(&self.cx.authority, group));
        let role = member.role.as_str().to_owned();
        self.cx
            .social
            .blocking(move |store| {
                if let Some(mut kept) = store.social_group(&account, &uri)? {
                    kept.role = Some(role);
                    store.put_social_group(&kept)?;
                }
                Ok(())
            })
            .await
    }

    /// The provider took channels away unasked (§7.1: `access_revoked`).
    /// They are not subscribed again until the groups are read again.
    fn unsubscribed(&mut self, unsubscribed: Unsubscribed) {
        for id in unsubscribed.channels {
            tracing::info!(provider = %self.cx.authority, channel = %id, reason = ?unsubscribed.reason, "unsubscribed by the provider");
            self.subscribed.remove(&id);
            self.revoked.insert(id);
        }
    }

    fn typing(&self, place: String, user: &Actor, state: &TypingState) {
        self.cx.tell(LiveEvent::Typing {
            account: self.cx.actor(),
            place,
            user: user.to_string(),
            typing: *state == TypingState::Start,
        });
    }
}

/// A message's id as a reaction names it.
fn local_id(id: &str) -> SocialResult<LocalId> {
    LocalId::new(id).map_err(|e| SocialError::Task(format!("a reaction names no message: {e}")))
}
