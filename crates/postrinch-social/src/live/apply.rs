//! Live timeline events kept: messages, edits, deletes and reactions.
//!
//! Delivery is at least once (§7.1), and a resume may replay what already
//! arrived, so every write is checked against what is kept: an event that
//! changes nothing is a duplicate, and is neither written nor told.

use postrinch_ofscp::ofscp_core::wire::{Message, Reaction, Timestamp};
use postrinch_store::{
    PlaceKind, SocialMessage, SocialPlace, Store, StoreResult, social_account_key,
};

use super::session::Context;
use super::{LiveEvent, MessageChange};
use crate::SocialResult;
use crate::sync::message::{Held, Kept, Reader, dm_holder, keep};

/// What a message event is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arrival {
    /// `message.created`, or a private message (`dm.message` is sent for
    /// new, edited and deleted messages alike, §7.4): kept whether or not
    /// it is here already.
    Any,
    /// `message.updated`: an edit of a message that is not kept (older
    /// than what is here) waits for scrolling back to it.
    IfKept,
}

/// Keep `message`, which arrived live in `place` with `cursor`.
pub(crate) async fn message(
    cx: &Context,
    place: String,
    message: Message,
    cursor: Option<String>,
    arrival: Arrival,
) -> SocialResult<()> {
    let actor = cx.identity.actor().clone();
    let host = cx.authority.clone();
    let told = cx
        .social
        .blocking(move |store| {
            let account = actor.to_string();
            let Some(kept_place) = store.social_place(&account, &place)? else {
                return Ok(None);
            };
            let reader = Reader {
                account: &actor,
                place: &place,
                host: &host,
            };
            let held = Held {
                holder: holder_in(&kept_place, &message.author).unwrap_or_else(|| host.clone()),
                message,
                reactions: Vec::new(),
            };
            // Looked up by the key it is kept under, which names the
            // provider holding it (docs/ofscp.md, "Private messages").
            let mut kept = keep(&reader, &held, false);
            let existing = store.social_message(&account, &place, &kept.message.id)?;
            if existing.is_none() && arrival == Arrival::IfKept {
                advance(store, &kept_place, cursor, None)?;
                return Ok(None);
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
    if let Some(event) = told {
        cx.tell(event);
    }
    Ok(())
}

/// Make the kept message `id` in `place` a tombstone (§7.1 `message.deleted`).
pub(crate) async fn deleted(
    cx: &Context,
    place: String,
    id: String,
    deleted_at: Option<Timestamp>,
    cursor: Option<String>,
) -> SocialResult<()> {
    let actor = cx.identity.actor().clone();
    let host = cx.authority.clone();
    let now = cx.client().provider().clock().now();
    let told = cx
        .social
        .blocking(move |store| {
            let account = actor.to_string();
            let Some(kept_place) = store.social_place(&account, &place)? else {
                return Ok(None);
            };
            let Some(existing) = store.social_message(&account, &place, &id)? else {
                advance(store, &kept_place, cursor, None)?;
                return Ok(None);
            };
            let Some(mut message) = wire(&existing) else {
                return Ok(None);
            };
            if message.deleted_at.is_none() {
                tombstone(&mut message, deleted_at.unwrap_or(now));
            }
            let reader = Reader {
                account: &actor,
                place: &place,
                host: &host,
            };
            let held = Held {
                holder: host.clone(),
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
    if let Some(event) = told {
        cx.tell(event);
    }
    Ok(())
}

/// Sum `reactions`, all of a kept message's as its provider now gives
/// them, onto it. Reading them all, rather than adding or taking away the
/// one an event names, makes a repeated event change nothing.
pub(crate) async fn reactions(
    cx: &Context,
    place: String,
    id: String,
    reactions: Vec<Reaction>,
) -> SocialResult<()> {
    let actor = cx.identity.actor().clone();
    let host = cx.authority.clone();
    let (place_told, id_told) = (place.clone(), id.clone());
    let told = cx
        .social
        .blocking(move |store| {
            let account = actor.to_string();
            let Some(existing) = store.social_message(&account, &place, &id)? else {
                return Ok(false);
            };
            let Some(message) = wire(&existing) else {
                return Ok(false);
            };
            let reader = Reader {
                account: &actor,
                place: &place,
                host: &host,
            };
            let held = Held {
                holder: host.clone(),
                message,
                reactions,
            };
            let kept = keep(&reader, &held, existing.unread);
            if kept.message.reactions == existing.reactions {
                return Ok(false);
            }
            store.put_social_message(&kept.message, &kept.search_text)?;
            Ok(true)
        })
        .await?;
    if told {
        cx.tell(LiveEvent::Reactions {
            account: cx.actor(),
            place: place_told,
            id: id_told,
        });
    }
    Ok(())
}

/// Write `kept` unless it is what `existing` already holds; the event to
/// tell when it was written.
fn write(
    store: &Store,
    existing: Option<&SocialMessage>,
    kept: &Kept,
) -> StoreResult<Option<LiveEvent>> {
    let change = match existing {
        Some(e) if e.message == kept.message.message && e.reactions == kept.message.reactions => {
            return Ok(None);
        }
        None => MessageChange::New,
        Some(e) if !e.deleted && kept.message.deleted => MessageChange::Deleted,
        Some(_) => MessageChange::Edited,
    };
    store.put_social_message(&kept.message, &kept.search_text)?;
    let unread = existing.map_or(kept.message.unread, |e| e.unread);
    Ok(Some(LiveEvent::Message {
        account: kept.message.account.clone(),
        place: kept.message.place.clone(),
        id: kept.message.id.clone(),
        change,
        unread,
        mentions_me: kept.message.mentions_me,
    }))
}

/// Move `place`'s live cursor to `cursor` and its latest activity to
/// `created`, where given.
fn advance(
    store: &Store,
    place: &SocialPlace,
    cursor: Option<String>,
    created: Option<i64>,
) -> StoreResult<()> {
    if cursor.is_none() && created <= place.last_activity {
        return Ok(());
    }
    store.change_social_place(&place.account, &place.id, |p| {
        if cursor.is_some() {
            p.cursors.live = cursor;
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
/// (§7.4), whichever provider sent the copy. `None` for a channel, whose
/// host holds it, and for a conversation kept without its participants.
fn holder_in(
    place: &SocialPlace,
    author: &postrinch_ofscp::ofscp_core::Actor,
) -> Option<postrinch_ofscp::ofscp_core::Authority> {
    if place.kind != PlaceKind::Conversation {
        return None;
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
