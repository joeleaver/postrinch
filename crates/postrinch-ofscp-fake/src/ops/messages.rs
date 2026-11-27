//! Channel messages and reactions: create, edit, delete, react (§7.1),
//! over REST or the WebSocket.

use ofscp_core::wire::{
    Message, MessageCreate, MessagePermissions, MessageReference, MessageType, MessageUpdate,
    Patch, Reaction, ReferenceKind,
};
use ofscp_core::ws::{ChannelMessage, Event, MessageDeleted, ReactionAdded, ReactionRemoved};
use ofscp_core::{Actor, LocalId};

use super::{Made, attach, notify};
use crate::inner::Inner;
use crate::problem::{Outcome, Problem};
use crate::state::State;
use crate::state::groups::{ChannelRec, GroupRec, reply_only_to};
use crate::ws::hub::{ConnId, To};

/// The WebSocket command an operation answers, if any: its connection
/// gets the event with the command's id as `correlationId`.
pub(crate) type ReplyTo<'a> = Option<(ConnId, &'a str)>;

/// The group and channel, if `actor` can read the channel; else `404`, as
/// for one that does not exist (§11.3).
fn readable<'s>(
    state: &'s mut State,
    group: &LocalId,
    channel: &LocalId,
    actor: Option<&Actor>,
) -> Outcome<(&'s GroupRec, &'s mut ChannelRec)> {
    let State {
        groups, channels, ..
    } = state;
    let g = groups.get(group);
    let c = channels.get_mut(channel);
    match (g, c) {
        (Some(g), Some(c)) if c.channel.group_id == *group && g.can_read(&c.channel, actor) => {
            Ok((g, c))
        }
        _ => Err(Problem::not_found("no such channel")),
    }
}

/// Answer `404` unless `actor` can read the channel (for reads).
pub(crate) fn check_readable(
    state: &mut State,
    group: &LocalId,
    channel: &LocalId,
    actor: Option<&Actor>,
) -> Outcome<()> {
    readable(state, group, channel, actor).map(|_| ())
}

fn creatable(kind: &MessageType) -> bool {
    matches!(
        kind,
        MessageType::Message | MessageType::Memo | MessageType::Article
    )
}

/// Post a message, in the order of checks §7.1 gives.
pub(crate) fn create(
    inner: &Inner,
    author: &Actor,
    group: &LocalId,
    channel: &LocalId,
    body: MessageCreate,
    reply_to: ReplyTo<'_>,
) -> Outcome<Made<Message>> {
    let kind = body.kind.clone().unwrap_or(MessageType::Message);
    if !creatable(&kind) {
        return Err(Problem::bad_request(
            "type must be message, memo or article",
        ));
    }
    let now = inner.now();
    let settings = inner.settings();
    let mut state = inner.state.lock().unwrap();
    let (g, c) = readable(&mut state, group, channel, Some(author))?;
    if !g.can_post(&c.channel, author, &kind) {
        return Err(Problem::forbidden(format!(
            "not permitted to post a {} here",
            kind.as_str()
        )));
    }
    let restricted = g.reply_only(&c.channel, author);
    if restricted && body.reference.is_none() {
        return Err(Problem::forbidden("only replies are permitted here"));
    }
    if let Some(cid) = &body.client_message_id
        && let Some(existing) = c.timeline.by_client_id(author, cid)
    {
        return Ok(Made::Existing(existing.message.clone()));
    }
    if let Some(r) = &body.reference {
        if r.kind != ReferenceKind::Reply {
            return Err(Problem::bad_request("reference.type must be reply"));
        }
        // A channel reply's parent is in the same channel, so held here
        // (§7.1: a `provider` naming another provider is a 400).
        if r.provider.as_ref().is_some_and(|p| *p != inner.authority) {
            return Err(Problem::bad_request(
                "reference.provider must be this channel's provider",
            ));
        }
        let parent = c
            .timeline
            .get(&r.id)
            .ok_or_else(|| Problem::bad_request("the parent is not a message in this channel"))?;
        if restricted
            && let Some(allowed) = reply_only_to(&c.channel)
            && !allowed.contains(&parent.message.kind)
        {
            return Err(Problem::forbidden(
                "replies to that type are not permitted here",
            ));
        }
    }
    let attachments = attach::check(
        &state,
        &inner.authority,
        author,
        body.attachments.unwrap_or_default(),
    )?;
    let article = kind == MessageType::Article;
    if article
        && let Some(cover) = &body.cover
        && !attachments.iter().any(|a| &a.id == cover)
    {
        return Err(Problem::bad_request("cover names none of the attachments"));
    }
    let message = Message {
        id: state.ids.next("msg_"),
        client_message_id: body.client_message_id,
        author: author.clone(),
        kind: kind.clone(),
        content: Some(body.content),
        attachments,
        reference: body.reference,
        tags: body.tags,
        mentions: body.mentions,
        title: body.title.filter(|_| article),
        summary: body.summary.filter(|_| article),
        cover: body.cover.filter(|_| article),
        reply_count: None,
        created_at: now,
        edited_at: None,
        deleted_at: None,
        permissions: Some(MessagePermissions {
            edit_until: (!article).then(|| now + settings.edit_window),
        }),
        metadata: body.metadata.unwrap_or_default(),
    };
    let (_, c) = readable(&mut state, group, channel, Some(author))?;
    let cursor = c.timeline.push(message.clone());
    let notes = notify::on_create(&mut state, &inner.authority, group, channel, &message);
    // Published with the state lock still held, so events go out in
    // cursor order (§7.1 *Ordering*).

    let event = Event::MessageCreated(ChannelMessage {
        group_id: group.clone(),
        channel_id: channel.clone(),
        cursor: Some(cursor),
        message: Box::new(message.clone()),
    });
    inner.hub.publish(
        To::Scope(channel.as_str()),
        &event,
        reply_to,
        inner.event_copies(),
    );
    for (user, notification) in notes {
        let event =
            Event::NotificationCreated(ofscp_core::ws::NotificationCreated { notification });
        inner
            .hub
            .publish(To::Actors(&[user], None), &event, None, 1);
    }
    Ok(Made::New(message))
}

/// Edit a message (§7.1 *Editing*).
pub(crate) fn update(
    inner: &Inner,
    actor: &Actor,
    group: &LocalId,
    channel: &LocalId,
    id: &LocalId,
    update: MessageUpdate,
    reply_to: ReplyTo<'_>,
) -> Outcome<Message> {
    let now = inner.now();
    let mut state = inner.state.lock().unwrap();
    let current = {
        let (_, c) = readable(&mut state, group, channel, Some(actor))?;
        c.timeline
            .get(id)
            .ok_or_else(|| Problem::not_found("no such message"))?
            .message
            .clone()
    };
    let attachments = match update.attachments.clone() {
        Some(a) => Some(attach::check(&state, &inner.authority, actor, a)?),
        None => None,
    };
    let updated = apply_update(current, actor, update, attachments, now)?;
    let (_, c) = readable(&mut state, group, channel, Some(actor))?;
    c.timeline.get_mut(id).expect("checked above").message = updated.clone();
    let cursor = c
        .timeline
        .log_change(crate::state::timeline::TimelineEvent::Updated(
            updated.clone(),
        ));
    // Published with the state lock still held, so events go out in
    // cursor order (§7.1 *Ordering*).
    let event = Event::MessageUpdated(ChannelMessage {
        group_id: group.clone(),
        channel_id: channel.clone(),
        cursor: Some(cursor),
        message: Box::new(updated.clone()),
    });
    inner.hub.publish(
        To::Scope(channel.as_str()),
        &event,
        reply_to,
        inner.event_copies(),
    );
    Ok(updated)
}

/// The edit rules shared by channel messages and DMs: author only, within
/// `editUntil`, at least one field, attachments already checked.
pub(crate) fn apply_update(
    mut m: Message,
    actor: &Actor,
    update: MessageUpdate,
    attachments: Option<Vec<ofscp_core::wire::Attachment>>,
    now: jiff::Timestamp,
) -> Outcome<Message> {
    if m.author != *actor {
        return Err(Problem::forbidden("only the author may edit a message"));
    }
    if m.deleted_at.is_some() {
        return Err(Problem::forbidden("the message was deleted"));
    }
    if let Some(until) = m.permissions.as_ref().and_then(|p| p.edit_until)
        && until <= now
    {
        return Err(Problem::forbidden("the edit window has closed"));
    }
    let article = m.kind == MessageType::Article;
    if update.is_empty() || (!article && update.has_only_article_fields()) {
        return Err(Problem::bad_request("the edit changes nothing"));
    }
    if let Some(content) = update.content {
        m.content = Some(content);
    }
    if let Some(a) = attachments {
        m.attachments = a;
    }
    if let Some(tags) = update.tags {
        m.tags = Some(tags);
    }
    if let Some(mentions) = update.mentions {
        m.mentions = Some(mentions);
    }
    if article {
        patch(&mut m.title, update.title);
        patch(&mut m.summary, update.summary);
        patch(&mut m.cover, update.cover);
        if let Some(cover) = &m.cover
            && !m.attachments.iter().any(|a| &a.id == cover)
        {
            return Err(Problem::bad_request("cover names none of the attachments"));
        }
    }
    m.edited_at = Some(now);
    Ok(m)
}

fn patch<T>(field: &mut Option<T>, p: Patch<T>) {
    match p {
        Patch::Keep => {}
        Patch::Clear => *field = None,
        Patch::Set(v) => *field = Some(v),
    }
}

/// Tombstone a message (§7.1 *Deleting*): the author or a moderator.
/// Returns when it was deleted; deleting a tombstone again changes and
/// fans out nothing.
pub(crate) fn delete(
    inner: &Inner,
    actor: &Actor,
    group: &LocalId,
    channel: &LocalId,
    id: &LocalId,
    reply_to: ReplyTo<'_>,
) -> Outcome<Made<jiff::Timestamp>> {
    let now = inner.now();
    let mut state = inner.state.lock().unwrap();
    let (g, c) = readable(&mut state, group, channel, Some(actor))?;
    let moderator = g.can_moderate(actor);
    let stored = c
        .timeline
        .get_mut(id)
        .ok_or_else(|| Problem::not_found("no such message"))?;
    if stored.message.author != *actor && !moderator {
        return Err(Problem::forbidden(
            "only the author or a moderator may delete",
        ));
    }
    if let Some(at) = stored.message.deleted_at {
        return Ok(Made::Existing(at));
    }
    tombstone(&mut stored.message, now);
    let cursor = c
        .timeline
        .log_change(crate::state::timeline::TimelineEvent::Deleted {
            id: id.clone(),
            at: now,
        });
    // Published with the state lock still held, so events go out in
    // cursor order (§7.1 *Ordering*).
    let event = Event::MessageDeleted(MessageDeleted {
        group_id: group.clone(),
        channel_id: channel.clone(),
        cursor: Some(cursor),
        message_id: id.clone(),
        deleted_at: Some(now),
    });
    inner.hub.publish(
        To::Scope(channel.as_str()),
        &event,
        reply_to,
        inner.event_copies(),
    );
    Ok(Made::New(now))
}

/// Clear what a tombstone does not keep (§7.1), ending its attachment
/// references (§5.8).
pub(crate) fn tombstone(m: &mut Message, now: jiff::Timestamp) {
    m.content = None;
    m.attachments.clear();
    m.title = None;
    m.summary = None;
    m.cover = None;
    m.tags = None;
    m.mentions = None;
    m.deleted_at = Some(now);
}

/// A new reaction object.
pub(crate) fn new_reaction(
    state: &mut State,
    author: &Actor,
    message: &LocalId,
    key: &str,
    unicode: Option<String>,
    image: Option<String>,
    now: jiff::Timestamp,
) -> Outcome<Reaction> {
    if key.is_empty() {
        return Err(Problem::bad_request("a reaction needs a key"));
    }
    if image.as_ref().is_some_and(|i| !i.starts_with("https://")) {
        return Err(Problem::bad_request("a reaction image must be https"));
    }
    Ok(Reaction {
        id: state.ids.next("rct_"),
        author: author.clone(),
        key: key.to_owned(),
        unicode,
        image,
        reference: MessageReference {
            kind: ReferenceKind::Message,
            id: message.clone(),
            provider: None,
        },
        created_at: now,
        metadata: Vec::new(),
    })
}

/// Which reaction to add: its key, and what it looks like.
pub(crate) struct NewReaction<'a> {
    pub(crate) key: &'a str,
    pub(crate) unicode: Option<String>,
    pub(crate) image: Option<String>,
}

/// Add a reaction; at most one per author, key and message.
pub(crate) fn react(
    inner: &Inner,
    actor: &Actor,
    group: &LocalId,
    channel: &LocalId,
    id: &LocalId,
    reaction: NewReaction<'_>,
    reply_to: ReplyTo<'_>,
) -> Outcome<Made<Reaction>> {
    let NewReaction {
        key,
        unicode,
        image,
    } = reaction;
    let now = inner.now();
    let mut state = inner.state.lock().unwrap();
    let (g, c) = readable(&mut state, group, channel, Some(actor))?;
    if !g.can_react(&c.channel, actor) {
        return Err(Problem::forbidden("not permitted to react here"));
    }
    let stored = c
        .timeline
        .get(id)
        .ok_or_else(|| Problem::not_found("no such message"))?;
    if let Some(r) = stored
        .reactions
        .iter()
        .find(|r| r.author == *actor && r.key == key)
    {
        return Ok(Made::Existing(r.clone()));
    }
    let reaction = new_reaction(&mut state, actor, id, key, unicode, image, now)?;
    let (_, c) = readable(&mut state, group, channel, Some(actor))?;
    c.timeline
        .get_mut(id)
        .expect("checked above")
        .reactions
        .push(reaction.clone());
    // Published with the state lock still held, so events go out in
    // cursor order (§7.1 *Ordering*).
    let event = Event::ReactionAdded(ReactionAdded {
        group_id: group.clone(),
        channel_id: channel.clone(),
        reaction: reaction.clone(),
    });
    inner.hub.publish(
        To::Scope(channel.as_str()),
        &event,
        reply_to,
        inner.event_copies(),
    );
    Ok(Made::New(reaction))
}

/// Remove the caller's reaction; true when there was one.
pub(crate) fn unreact(
    inner: &Inner,
    actor: &Actor,
    group: &LocalId,
    channel: &LocalId,
    id: &LocalId,
    key: &str,
    reply_to: ReplyTo<'_>,
) -> Outcome<bool> {
    let mut state = inner.state.lock().unwrap();
    let (_, c) = readable(&mut state, group, channel, Some(actor))?;
    let stored = c
        .timeline
        .get_mut(id)
        .ok_or_else(|| Problem::not_found("no such message"))?;
    let before = stored.reactions.len();
    stored
        .reactions
        .retain(|r| !(r.author == *actor && r.key == key));
    let removed = stored.reactions.len() != before;
    // Published with the state lock still held, so events go out in
    // cursor order (§7.1 *Ordering*).
    if removed {
        let event = Event::ReactionRemoved(ReactionRemoved {
            group_id: group.clone(),
            channel_id: channel.clone(),
            message_id: id.clone(),
            key: key.to_owned(),
            author: actor.clone(),
        });
        inner.hub.publish(
            To::Scope(channel.as_str()),
            &event,
            reply_to,
            inner.event_copies(),
        );
    }
    Ok(removed)
}
