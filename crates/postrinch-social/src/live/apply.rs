//! Messages kept as a provider gives them: live timeline events (new
//! messages, edits, deletes and reactions), and the outbox's answers.
//!
//! Delivery is at least once (§7.1), and a resume may replay what already
//! arrived, so every write is checked against what is kept: an event that
//! changes nothing is a duplicate, and is neither written nor told.

use postrinch_ofscp::http::{Direction, PageRequest};
use postrinch_ofscp::ofscp_core::wire::{Message, Paged, Reaction, Timestamp};
use postrinch_ofscp::ofscp_core::{Actor, Authority};
use postrinch_store::{
    MessagePut, PlaceKind, SocialMessage, SocialPlace, Store, StoreResult, social_account_key,
};

use super::{LiveEvent, MessageChange};
use crate::sync::message::{Held, Kept, Reader, dm_holder, keep};
use crate::{Social, SocialResult};

/// Who keeps a provider's messages: the engine, the account, and the
/// provider hosting the place (home, for a conversation).
pub(crate) struct Keeper<'a> {
    pub social: &'a Social,
    pub actor: &'a Actor,
    pub host: &'a Authority,
}

impl Keeper<'_> {
    fn tell(&self, event: LiveEvent) {
        self.social.live.tell(event);
    }
}

/// What a message event is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arrival {
    /// `message.created`, or a private message (`dm.message` is sent for
    /// new, edited and deleted messages alike, §7.4), or the provider's
    /// answer to a send or an edit: kept whether or not it is here
    /// already.
    Any,
    /// `message.updated`: an edit of a message that is not kept (older
    /// than what is here) waits for scrolling back to it.
    IfKept,
}

/// Where an event's cursor is from, so a resume starts from it: the
/// place's host, or, in a conversation with someone on another provider,
/// that provider, which holds the messages the person sent (§7.4: a
/// cursor issued by one participant's provider is never valid on the
/// other's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Resume {
    /// The place's host: a channel's provider, or home.
    Host(String),
    /// The other participant's provider, by a `dmId` subscription there.
    Sent(String),
}

/// Keep `message`, which arrived in `place` with `cursor`. When it is the
/// copy of a post of the account's still queued, keeping it settles the
/// post, which is told as sent ([`sent`]): its copy may arrive on a socket
/// before the provider's answer.
pub(crate) async fn message(
    k: &Keeper<'_>,
    place: String,
    message: Message,
    cursor: Option<Resume>,
    arrival: Arrival,
) -> SocialResult<()> {
    let actor = k.actor.clone();
    let host = k.host.clone();
    let told = k
        .social
        .blocking(move |store| {
            let account = actor.to_string();
            let Some(kept_place) = store.social_place(&account, &place)? else {
                return Ok(Vec::new());
            };
            let reader = Reader {
                account: &actor,
                place: &place,
                host: &host,
            };
            let held = Held {
                holder: holder_in(&kept_place, &message.author, &actor)
                    .unwrap_or_else(|| host.clone()),
                message,
                reactions: Vec::new(),
            };
            // Looked up by the key it is kept under, which names the
            // provider holding it (docs/ofscp.md, "Private messages").
            let mut kept = keep(&reader, &held, false);
            let existing = store.social_message(&account, &place, &kept.message.id)?;
            if existing.is_none() && arrival == Arrival::IfKept {
                advance(store, &kept_place, cursor, None)?;
                return Ok(Vec::new());
            }
            if existing.is_none() && is_new_unread(store, &kept_place, &held.message, &actor)? {
                kept.message.unread = true;
            }
            // Events carry no reactions: those kept stay, as does whether
            // it is read.
            if let Some(existing) = &existing {
                kept.message.reactions = existing.reactions.clone();
                kept.message.unread = existing.unread;
            }
            let told = write(store, existing.as_ref(), &kept)?;
            advance(store, &kept_place, cursor, Some(kept.message.created))?;
            Ok(told)
        })
        .await?;
    for event in told {
        k.tell(event);
    }
    Ok(())
}

/// What keeping `message` tells besides the message itself: that it sent
/// the account's queued post it is the copy of, if it settled one. Every
/// write of a message tells it, so a post is told sent exactly once,
/// however its copy arrived (an answer, a socket, a sync or a backfill).
pub(crate) fn sent(message: &SocialMessage, put: &MessagePut) -> Option<LiveEvent> {
    put.settled.clone().map(|client_id| LiveEvent::Sent {
        account: message.account.clone(),
        client_id,
        place: Some(message.place.clone()),
    })
}

/// Make the kept message `id` in `place` a tombstone deleted at
/// `deleted_at` (§7.1 `message.deleted`, or a delete the provider took).
pub(crate) async fn deleted(
    k: &Keeper<'_>,
    place: String,
    id: String,
    deleted_at: Timestamp,
    cursor: Option<Resume>,
) -> SocialResult<()> {
    let actor = k.actor.clone();
    let host = k.host.clone();
    let told = k
        .social
        .blocking(move |store| {
            let account = actor.to_string();
            let Some(kept_place) = store.social_place(&account, &place)? else {
                return Ok(Vec::new());
            };
            let Some(existing) = store.social_message(&account, &place, &id)? else {
                advance(store, &kept_place, cursor, None)?;
                return Ok(Vec::new());
            };
            let Some(mut message) = wire(&existing) else {
                return Ok(Vec::new());
            };
            if message.deleted_at.is_none() {
                tombstone(&mut message, deleted_at);
            }
            let reader = Reader {
                account: &actor,
                place: &place,
                host: &host,
            };
            let held = Held {
                holder: holder_in(&kept_place, &message.author, &actor)
                    .unwrap_or_else(|| host.clone()),
                message,
                reactions: Vec::new(),
            };
            let mut kept = keep(&reader, &held, existing.unread);
            kept.message.reactions = existing.reactions.clone();
            let told = write(store, Some(&existing), &kept)?;
            advance(store, &kept_place, cursor, None)?;
            Ok(told)
        })
        .await?;
    for event in told {
        k.tell(event);
    }
    Ok(())
}

/// Sum `reactions`, all of a kept message's as its provider now gives
/// them, onto it. Reading them all, rather than adding or taking away the
/// one an event names, makes a repeated event change nothing.
pub(crate) async fn reactions(
    k: &Keeper<'_>,
    place: String,
    id: String,
    reactions: Vec<Reaction>,
) -> SocialResult<()> {
    let actor = k.actor.clone();
    let host = k.host.clone();
    let (place_told, id_told) = (place.clone(), id.clone());
    let told = k
        .social
        .blocking(move |store| {
            let account = actor.to_string();
            let Some(kept_place) = store.social_place(&account, &place)? else {
                return Ok(None);
            };
            let Some(existing) = store.social_message(&account, &place, &id)? else {
                return Ok(None);
            };
            let Some(message) = wire(&existing) else {
                return Ok(None);
            };
            let reader = Reader {
                account: &actor,
                place: &place,
                host: &host,
            };
            let held = Held {
                holder: holder_in(&kept_place, &message.author, &actor)
                    .unwrap_or_else(|| host.clone()),
                message,
                reactions,
            };
            let kept = keep(&reader, &held, existing.unread);
            if kept.message.reactions == existing.reactions {
                return Ok(None);
            }
            let put = store.put_social_message(&kept.message, &kept.search_text)?;
            Ok(Some(sent(&kept.message, &put)))
        })
        .await?;
    if let Some(sent) = told {
        if let Some(event) = sent {
            k.tell(event);
        }
        k.tell(LiveEvent::Reactions {
            account: k.actor.to_string(),
            place: place_told,
            id: id_told,
        });
    }
    Ok(())
}

/// Write `kept` unless it is what `existing` already holds; the events to
/// tell when it was written.
fn write(
    store: &Store,
    existing: Option<&SocialMessage>,
    kept: &Kept,
) -> StoreResult<Vec<LiveEvent>> {
    let change = match existing {
        // Its first write settled whatever post it is the copy of.
        Some(e) if e.message == kept.message.message && e.reactions == kept.message.reactions => {
            return Ok(Vec::new());
        }
        None => MessageChange::New,
        Some(e) if !e.deleted && kept.message.deleted => MessageChange::Deleted,
        Some(_) => MessageChange::Edited,
    };
    let put = store.put_social_message(&kept.message, &kept.search_text)?;
    let unread = existing.map_or(kept.message.unread, |e| e.unread);
    let mut told = vec![LiveEvent::Message {
        account: kept.message.account.clone(),
        place: kept.message.place.clone(),
        id: kept.message.id.clone(),
        change,
        unread,
        mentions_me: kept.message.mentions_me,
    }];
    told.extend(sent(&kept.message, &put));
    Ok(told)
}

/// Move the live cursor of `place` that `cursor` is from to it, and its
/// latest activity to `created`, where given.
fn advance(
    store: &Store,
    place: &SocialPlace,
    cursor: Option<Resume>,
    created: Option<i64>,
) -> StoreResult<()> {
    if cursor.is_none() && created <= place.last_activity {
        return Ok(());
    }
    store.change_social_place(&place.account, &place.id, |p| {
        match cursor {
            Some(Resume::Host(at)) => p.cursors.live = Some(at),
            Some(Resume::Sent(at)) => p.cursors.sent = Some(at),
            None => {}
        }
        p.last_activity = p.last_activity.max(created);
    })?;
    Ok(())
}

/// Whether a message new here is unread: not the person's own, and after
/// the place's read marker (the provider's at home, the place's own
/// elsewhere, §7.7).
fn is_new_unread(
    store: &Store,
    place: &SocialPlace,
    message: &Message,
    me: &postrinch_ofscp::ofscp_core::Actor,
) -> StoreResult<bool> {
    if social_account_key(&message.author.to_string()) == social_account_key(&me.to_string()) {
        return Ok(false);
    }
    let Some(marker) = &place.read_marker else {
        return Ok(true);
    };
    if marker == message.id.as_str() {
        return Ok(false);
    }
    let read_up_to = store
        .social_message(&place.account, &place.id, marker)?
        .map(|m| m.created);
    Ok(read_up_to.is_none_or(|read| message.created_at.as_millisecond() > read))
}

/// Who holds a message `author` wrote in `place`, where that is not the
/// provider it arrived from: in a conversation, the recipient's home
/// (§7.4), whichever provider sent the copy. One written to `me` is held
/// at my home; one I wrote, at the other participant's, read from the
/// conversation kept. `None` for a channel, whose host holds it, and for a
/// conversation kept without its participants.
fn holder_in(place: &SocialPlace, author: &Actor, me: &Actor) -> Option<Authority> {
    if place.kind != PlaceKind::Conversation {
        return None;
    }
    if social_account_key(&author.to_string()) != social_account_key(&me.to_string()) {
        return Some(me.domain().clone());
    }
    let conversation = serde_json::from_value(place.object.clone()).ok()?;
    dm_holder(&conversation, author)
}

/// The wire message a kept one holds.
fn wire(kept: &SocialMessage) -> Option<Message> {
    match serde_json::from_value(kept.message.clone()) {
        Ok(message) => Some(message),
        Err(error) => {
            tracing::warn!(id = %kept.id, %error, "a kept message does not read back");
            None
        }
    }
}

/// Clear what a tombstone does not keep (§7.1 *Deleting*).
fn tombstone(message: &mut Message, at: Timestamp) {
    message.content = None;
    message.attachments.clear();
    message.title = None;
    message.summary = None;
    message.cover = None;
    message.tags = None;
    message.mentions = None;
    message.deleted_at = Some(at);
}

/// Add a page of reactions to `all`; the request for the next page, if
/// there is one (§7.2: continue in the same direction).
pub(crate) fn collect(all: &mut Vec<Reaction>, page: Paged<Reaction>) -> Option<PageRequest> {
    all.extend(page.items);
    page.page
        .next_cursor
        .map(|next| PageRequest::after(next, Direction::Backward))
}
