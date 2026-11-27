//! The home provider's read markers (§7.7), as they move on the person's
//! other devices (`read.updated`) or are read again after a reconnect.
//!
//! A marker only moves forward: one behind where the place's marker is
//! here (moved by the person on this computer, and on its way to the
//! provider through the outbox) changes nothing. Messages up to it become
//! read; none becomes unread.
//!
//! "Up to it" is by when messages were made, as the store orders a place
//! (and as `mark_social_read` in postrinch-commands reads). Times are
//! whole seconds on the wire, so another message made in the marker's
//! own second is read with it whether it came before or after.

use postrinch_ofscp::ofscp_core::wire::ReadMarker;
use postrinch_store::{Store, StoreResult};

use super::LiveEvent;
use super::session::Context;
use crate::SocialResult;
use crate::uris::channel_uri;

/// Move `place`'s read marker to the message `id`, if that is further on
/// than where it is, marking read every message up to it. Whether the
/// marker moved.
pub(crate) fn move_to(store: &Store, account: &str, place: &str, id: &str) -> StoreResult<bool> {
    let Some(kept) = store.social_place(account, place)? else {
        return Ok(false);
    };
    if kept.read_marker.as_deref() == Some(id) {
        return Ok(false);
    }
    let read = store.social_message(account, place, id)?.map(|m| m.created);
    let current = match &kept.read_marker {
        Some(current) => store
            .social_message(account, place, current)?
            .map(|m| m.created),
        None => None,
    };
    let forward = match (read, current) {
        (Some(read), Some(current)) => read > current,
        // Where the place's own marker is, is unknown: the provider's
        // stands.
        (_, None) => true,
        // The provider's names a message not kept here, and the place's a
        // message that is. Kept history runs from the newest message back,
        // so a message not in it is older than all of it: the provider's
        // marker is the one behind. (A message too new to be kept yet is
        // read by the marker the next refresh reads.)
        (None, Some(_)) => false,
    };
    if !forward {
        return Ok(false);
    }
    if let Some(read) = read {
        store.set_social_messages_read(account, place, read)?;
    }
    store.change_social_place(account, place, |p| p.read_marker = Some(id.to_owned()))?;
    Ok(true)
}

/// Apply markers the home provider sent (`read.updated`).
pub(crate) async fn read_updated(cx: &Context, markers: Vec<ReadMarker>) -> SocialResult<()> {
    let home = cx.identity.actor().domain().clone();
    let account = cx.actor();
    let moved = cx
        .social
        .blocking(move |store| {
            let mut moved = Vec::new();
            for marker in markers {
                let Some(id) = marker.last_read_message_id else {
                    continue;
                };
                // A channel's scope is its local id at home; a
                // conversation's is its dmId (§7.7).
                let place = match &marker.group_id {
                    Some(group) => channel_uri(&home, group, &marker.scope_id),
                    None => marker.scope_id.to_string(),
                };
                if move_to(store, &account, &place, id.as_str())? {
                    moved.push((place, id.to_string()));
                }
            }
            Ok(moved)
        })
        .await?;
    for (place, marker) in moved {
        cx.tell(LiveEvent::Read {
            account: cx.actor(),
            place,
            marker,
        });
    }
    Ok(())
}
