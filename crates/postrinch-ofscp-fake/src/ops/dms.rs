//! Direct messages (§7.4): stored in the recipient's inbox, with
//! conversation pointers at the sender's home.

use jiff::Timestamp;
use ofscp_core::wire::{
    ConversationPointer, DmConversation, DmMessageCreate, Message, MessagePermissions, MessageType,
    MessageUpdate, Reaction, ReferenceKind,
};
use ofscp_core::ws::{DmConversationEvent, DmMessage, DmReaction, Event, ReactionState};
use ofscp_core::{Actor, DmId, LocalId};

use super::messages::{apply_update, new_reaction, tombstone};
use super::{Made, attach};
use crate::inner::Inner;
use crate::problem::{Outcome, Problem};
use crate::state::timeline::{Timeline, TimelineEvent};
use crate::state::{DmPointer, DmRec, State};
use crate::ws::hub::To;

pub(crate) fn parse_dm(s: &str) -> Outcome<DmId> {
    DmId::parse(s).map_err(|_| Problem::not_found("no such conversation"))
}

/// Is `caller` a participant of `dm` as this provider can tell: the id
/// derives from the caller and one of its users, or it is the caller's own
/// conversation pointer?
pub(crate) fn is_participant(inner: &Inner, state: &State, dm: &DmId, caller: &Actor) -> bool {
    state
        .local_participant(&inner.authority, dm, caller)
        .is_some()
        || state
            .local_user(&inner.authority, caller)
            .is_some_and(|u| u.dm_pointers.contains_key(dm))
}

/// Deliver a DM to this provider's user (§7.4 *Sending*).
pub(crate) fn send(
    inner: &Inner,
    author: &Actor,
    dm: &DmId,
    body: DmMessageCreate,
) -> Outcome<Made<Message>> {
    let now = inner.now();
    let window = inner.settings().edit_window;
    let mut state = inner.state.lock().unwrap();
    let recipient = match &body.to {
        Some(to) => {
            let ok = to != author
                && state.local_user(&inner.authority, to).is_some()
                && DmId::derive(author, to) == *dm;
            ok.then(|| to.clone())
        }
        None => state.local_participant(&inner.authority, dm, author),
    }
    .ok_or_else(|| {
        Problem::bad_request("the conversation is not between the author and a user here")
    })?;
    if let Some(existing) = state
        .dms
        .get(dm)
        .and_then(|r| r.timeline.by_client_id(author, &body.client_message_id))
    {
        return Ok(Made::Existing(existing.message.clone()));
    }
    if let Some(r) = &body.reference {
        let parent_here = state
            .dms
            .get(dm)
            .is_some_and(|d| d.timeline.get(&r.id).is_some());
        if r.kind != ReferenceKind::Reply || !parent_here {
            return Err(Problem::bad_request(
                "the parent is not a message in this conversation",
            ));
        }
    }
    let attachments = attach::check(
        &state,
        &inner.authority,
        author,
        body.attachments.unwrap_or_default(),
    )?;
    let message = Message {
        id: state.ids.next("msg_"),
        client_message_id: Some(body.client_message_id),
        author: author.clone(),
        kind: MessageType::Message,
        content: Some(body.content),
        attachments,
        reference: body.reference,
        tags: None,
        mentions: body.mentions,
        title: None,
        summary: None,
        cover: None,
        reply_count: None,
        created_at: now,
        edited_at: None,
        deleted_at: None,
        permissions: Some(MessagePermissions {
            edit_until: Some(now + window),
        }),
        metadata: Vec::new(),
    };
    let rec = state.dms.entry(dm.clone()).or_insert_with(|| DmRec {
        participants: [author.clone(), recipient.clone()],
        timeline: Timeline::new(dm.as_str()),
    });
    let cursor = rec.timeline.push(message.clone());
    let author_view = state.local_user_mut(&inner.authority, author).map(|u| {
        let pointer = u
            .dm_pointers
            .entry(dm.clone())
            .or_insert_with(|| DmPointer {
                with: recipient.clone(),
                last_message: None,
                stored_at: None,
                updated_at: now,
            });
        pointer.last_message = Some(message.clone());
        pointer.stored_at = Some(now);
        pointer.updated_at = now;
    });
    let conversation = author_view.and_then(|()| conversation(inner, &state, author, dm));
    // Published with the state lock still held, so events go out in
    // cursor order (§7.1 *Ordering*).

    let event = Event::DmMessage(DmMessage {
        dm_id: dm.clone(),
        cursor: Some(cursor),
        message: Box::new(message.clone()),
    });
    inner.hub.publish(
        To::Actors(&[recipient], Some(dm.as_str())),
        &event,
        None,
        inner.event_copies(),
    );
    if let Some(conversation) = conversation {
        let event = Event::DmConversation(DmConversationEvent {
            conversation: Box::new(conversation),
        });
        inner.hub.publish(
            To::Actors(std::slice::from_ref(author), None),
            &event,
            None,
            1,
        );
    }
    Ok(Made::New(message))
}

/// The inbox owner here: the local participant that is not `actor`, or
/// `actor` itself when both are local.
fn inbox_owners(rec: &DmRec, inner: &Inner) -> Vec<Actor> {
    rec.participants
        .iter()
        .filter(|p| p.is_local_to(&inner.authority))
        .cloned()
        .collect()
}

/// Edit (`Some`) or delete (`None`) a message the actor sent (§7.4
/// *Edits & deletes*), re-sent as `dm.message`.
pub(crate) fn change(
    inner: &Inner,
    actor: &Actor,
    dm: &DmId,
    id: &LocalId,
    update: Option<MessageUpdate>,
) -> Outcome<Option<Message>> {
    let now = inner.now();
    let mut state = inner.state.lock().unwrap();
    let current = state
        .dms
        .get(dm)
        .filter(|r| r.has(actor))
        .and_then(|r| r.timeline.get(id))
        .ok_or_else(|| Problem::not_found("no such message"))?
        .message
        .clone();
    let changed = match update {
        Some(update) => {
            let attachments = match update.attachments.clone() {
                Some(a) => Some(attach::check(&state, &inner.authority, actor, a)?),
                None => None,
            };
            let m = apply_update(current, actor, update, attachments, now)?;
            Some((TimelineEvent::Updated(m.clone()), m))
        }
        None if current.author != *actor => {
            return Err(Problem::forbidden("only the author may delete a message"));
        }
        None if current.deleted_at.is_some() => None,
        None => {
            let mut m = current;
            tombstone(&mut m, now);
            Some((
                TimelineEvent::Deleted {
                    id: id.clone(),
                    at: now,
                },
                m,
            ))
        }
    };
    let Some((event, message)) = changed else {
        return Ok(None);
    };
    let rec = state.dms.get_mut(dm).expect("checked above");
    rec.timeline.get_mut(id).expect("checked above").message = message.clone();
    let cursor = rec.timeline.log_change(event);
    let owners = inbox_owners(rec, inner);
    // Published with the state lock still held, so events go out in
    // cursor order (§7.1 *Ordering*).
    let event = Event::DmMessage(DmMessage {
        dm_id: dm.clone(),
        cursor: Some(cursor),
        message: Box::new(message.clone()),
    });
    inner.hub.publish(
        To::Actors(&owners, Some(dm.as_str())),
        &event,
        None,
        inner.event_copies(),
    );
    Ok(Some(message))
}

/// Add (`Some(key data)`) or remove a reaction on a message held here.
pub(crate) fn react(
    inner: &Inner,
    actor: &Actor,
    dm: &DmId,
    id: &LocalId,
    key: &str,
    add: Option<(Option<String>, Option<String>)>,
) -> Outcome<Option<Reaction>> {
    let now = inner.now();
    let mut state = inner.state.lock().unwrap();
    let held = state
        .dms
        .get(dm)
        .filter(|r| r.has(actor))
        .and_then(|r| r.timeline.get(id))
        .ok_or_else(|| Problem::not_found("no such message"))?;
    let existing = held
        .reactions
        .iter()
        .find(|r| r.author == *actor && r.key == key)
        .cloned();
    let (state_kind, reaction) = match (add, existing) {
        (Some(_), Some(r)) => return Ok(Some(r)),
        (None, None) => return Ok(None),
        (Some((unicode, image)), None) => {
            let r = new_reaction(&mut state, actor, id, key, unicode, image, now)?;
            let stored = state
                .dms
                .get_mut(dm)
                .and_then(|d| d.timeline.get_mut(id))
                .expect("checked above");
            stored.reactions.push(r.clone());
            (ReactionState::Added, Some(r))
        }
        (None, Some(_)) => {
            let stored = state
                .dms
                .get_mut(dm)
                .and_then(|d| d.timeline.get_mut(id))
                .expect("checked above");
            stored
                .reactions
                .retain(|r| !(r.author == *actor && r.key == key));
            (ReactionState::Removed, None)
        }
    };
    let owners = inbox_owners(state.dms.get(dm).expect("checked above"), inner);
    // Published with the state lock still held, so events go out in
    // cursor order (§7.1 *Ordering*).
    let removed = reaction.is_none();
    let event = Event::DmReaction(DmReaction {
        dm_id: dm.clone(),
        message_id: id.clone(),
        state: state_kind,
        reaction: reaction.clone(),
        key: removed.then(|| key.to_owned()),
        author: removed.then(|| actor.clone()),
    });
    inner.hub.publish(
        To::Actors(&owners, Some(dm.as_str())),
        &event,
        None,
        inner.event_copies(),
    );
    Ok(reaction)
}

/// `PUT /api/me/dms/{dmId}` (§7.4 *Conversation pointers*).
pub(crate) fn put_pointer(
    inner: &Inner,
    caller: &Actor,
    dm: &DmId,
    body: ConversationPointer,
) -> Outcome<DmConversation> {
    let now = inner.now();
    let canonical = Actor::parse_canonical(&body.with.to_string()).is_ok_and(|a| a == body.with);
    if !canonical || body.with == *caller || DmId::derive(caller, &body.with) != *dm {
        return Err(Problem::bad_request(
            "with is not the canonical counterparty of this conversation",
        ));
    }
    if body
        .last_message
        .as_ref()
        .is_some_and(|m| m.author != *caller)
    {
        return Err(Problem::bad_request("lastMessage must be the caller's"));
    }
    let mut state = inner.state.lock().unwrap();
    let user = state
        .local_user_mut(&inner.authority, caller)
        .ok_or_else(|| Problem::forbidden("a local-account endpoint"))?;
    let pointer = user
        .dm_pointers
        .entry(dm.clone())
        .or_insert_with(|| DmPointer {
            with: body.with.clone(),
            last_message: None,
            stored_at: None,
            updated_at: now,
        });
    if let Some(m) = body.last_message {
        pointer.last_message = Some(m);
        pointer.stored_at = Some(now);
    }
    pointer.updated_at = now;
    let view = conversation(inner, &state, caller, dm).expect("the pointer was just stored");
    // Published with the state lock still held, so events go out in
    // cursor order (§7.1 *Ordering*).
    let event = Event::DmConversation(DmConversationEvent {
        conversation: Box::new(view.clone()),
    });
    inner.hub.publish(
        To::Actors(std::slice::from_ref(caller), None),
        &event,
        None,
        1,
    );
    Ok(view)
}

/// One of `user`'s conversations as `GET /api/me/dms` lists it.
pub(crate) fn conversation(
    inner: &Inner,
    state: &State,
    user: &Actor,
    dm: &DmId,
) -> Option<DmConversation> {
    let received = state
        .dms
        .get(dm)
        .filter(|r| r.has(user))
        .and_then(|r| {
            r.timeline
                .messages()
                .filter(|s| s.message.author != *user)
                .last()
        })
        .map(|s| s.message.clone());
    let pointer = state
        .local_user(&inner.authority, user)
        .and_then(|u| u.dm_pointers.get(dm))
        .filter(|p| DmId::derive(user, &p.with) == *dm);
    let with = match (&received, pointer) {
        (_, Some(p)) => p.with.clone(),
        (Some(m), None) => m.author.clone(),
        (None, None) => return None,
    };
    let sent = pointer.and_then(|p| Some((p.last_message.clone()?, p.stored_at?)));
    let last_message = match (&received, sent) {
        (Some(r), Some((s, at))) if at > r.created_at => Some(s),
        (Some(r), _) => Some(r.clone()),
        (None, s) => s.map(|(m, _)| m),
    };
    let times = [
        received.as_ref().map(|m| m.created_at),
        pointer.map(|p| p.updated_at),
    ];
    let updated_at = times
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(Timestamp::UNIX_EPOCH);
    let mut participants = [user.clone(), with];
    participants.sort();
    Some(DmConversation {
        id: dm.clone(),
        participants,
        last_message,
        updated_at,
        metadata: Vec::new(),
    })
}

/// Every conversation in `user`'s list, oldest activity first.
pub(crate) fn conversations(inner: &Inner, state: &State, user: &Actor) -> Vec<DmConversation> {
    let mut ids: Vec<&DmId> = state
        .dms
        .iter()
        .filter(|(_, r)| r.has(user))
        .map(|(id, _)| id)
        .collect();
    if let Some(u) = state.local_user(&inner.authority, user) {
        ids.extend(u.dm_pointers.keys());
    }
    ids.sort();
    ids.dedup();
    let mut list: Vec<DmConversation> = ids
        .into_iter()
        .filter_map(|dm| conversation(inner, state, user, dm))
        .collect();
    list.sort_by_key(|c| c.updated_at);
    list
}
