//! `subscribe` (§7.1 *Subscriptions*, *Resuming*; §7.4 dmId targets).

use std::collections::BTreeSet;

use ofscp_core::LocalId;
use ofscp_core::wire::ErrorCode;
use ofscp_core::ws::{ChannelMessage, DmMessage, Event, MessageDeleted, Subscribe, Subscribed};

use super::Conn;
use super::hub::Outgoing;
use crate::ops::dms::is_participant;
use crate::problem::Problem;
use crate::state::State;
use crate::state::timeline::{Logged, Replay, TimelineEvent};

/// What one target of a `subscribe` is.
enum Target {
    Channel { group: LocalId },
    Dm,
}

pub(super) fn subscribe(conn: &Conn, corr: String, cmd: Subscribe) {
    let inner = conn.inner.clone();
    let actor = conn.actor.clone().expect("authenticated");
    let cap = inner.settings().subscription_cap;
    let distinct: BTreeSet<&LocalId> = cmd.channels.iter().collect();
    if distinct.len() > cap {
        let p = Problem::new(
            ErrorCode::SubscriptionLimit,
            "the list exceeds the subscription cap",
        );
        conn.error(p, Some(corr));
        return;
    }
    let replay_limit = inner.faults.lock().unwrap().replay_limit;
    // Everything under the state lock, which is not held across an await.
    let checked = {
        let state = inner.state.lock().unwrap();
        let mut ok: Vec<(LocalId, Target)> = Vec::new();
        let (mut missing, mut forbidden) = (Vec::new(), Vec::new());
        for id in distinct {
            if id.as_str().starts_with("dm_") {
                match ofscp_core::DmId::parse(id.as_str()) {
                    Ok(dm) if is_participant(&inner, &state, &dm, &actor) => {
                        ok.push((id.clone(), Target::Dm))
                    }
                    _ => forbidden.push(id.clone()),
                }
                continue;
            }
            match state.channels.get(id) {
                Some(c)
                    if state
                        .groups
                        .get(&c.channel.group_id)
                        .is_some_and(|g| g.can_read(&c.channel, Some(&actor))) =>
                {
                    let group = c.channel.group_id.clone();
                    ok.push((id.clone(), Target::Channel { group }));
                }
                _ => missing.push(id.clone()),
            }
        }
        let new = ok
            .iter()
            .filter(|(id, _)| !inner.hub.is_subscribed(conn.id, id.as_str()))
            .count();
        if inner.hub.subscription_count(conn.id) + new > cap {
            Err(Problem::new(
                ErrorCode::SubscriptionLimit,
                "the subscription cap would be exceeded",
            ))
        } else {
            let mut out = Vec::new();
            let mut truncated = Vec::new();
            for (id, target) in &ok {
                let Some(since) = cmd.since.as_ref().and_then(|s| s.get(id)) else {
                    continue;
                };
                match replay(&state, id, target, since, replay_limit) {
                    Some(events) => out.extend(events),
                    None => truncated.push(id.clone()),
                }
            }
            if !ok.is_empty() {
                let ack = Event::Subscribed(Subscribed {
                    channels: ok.iter().map(|(id, _)| id.clone()).collect(),
                    truncated: (!truncated.is_empty()).then_some(truncated),
                });
                let mut frames = vec![Outgoing::event(ack, Some(corr.clone()))];
                frames.extend(out.into_iter().map(|body| Outgoing::event(body, None)));
                let scopes: Vec<String> = ok.iter().map(|(id, _)| id.as_str().to_owned()).collect();
                // Under the state lock, so no timeline event slips between the
                // replay and live delivery (§7.1 *Ordering*).
                inner.hub.subscribe_with(conn.id, &scopes, frames);
            }
            Ok((missing, forbidden))
        }
    };
    let (missing, forbidden) = match checked {
        Ok(lists) => lists,
        Err(p) => {
            conn.error(p, Some(corr));
            return;
        }
    };
    if !missing.is_empty() {
        let p = Problem::not_found("No such channel")
            .with_details(serde_json::json!({ "channels": missing }));
        conn.error(p, Some(corr.clone()));
    }
    if !forbidden.is_empty() {
        let p = Problem::forbidden("not a participant of the conversation")
            .with_details(serde_json::json!({ "channels": forbidden }));
        conn.error(p, Some(corr));
    }
}

/// The events after `since` as frames, or `None` for `truncated`.
fn replay(
    state: &State,
    id: &LocalId,
    target: &Target,
    since: &str,
    limit: Option<usize>,
) -> Option<Vec<Event>> {
    match target {
        Target::Channel { group } => {
            let timeline = &state.channels.get(id)?.timeline;
            let Replay::Events(events) = timeline.replay(since, limit) else {
                return None;
            };
            Some(
                events
                    .into_iter()
                    .map(|l| channel_event(group, id, l))
                    .collect(),
            )
        }
        Target::Dm => {
            let dm = ofscp_core::DmId::parse(id.as_str()).ok()?;
            // No conversation yet: no cursor can be from it.
            let rec = state.dms.get(&dm)?;
            let Replay::Events(events) = rec.timeline.replay(since, limit) else {
                return None;
            };
            let frames = events.into_iter().filter_map(|Logged { cursor, event }| {
                let message = match event {
                    TimelineEvent::Created(m) | TimelineEvent::Updated(m) => m,
                    TimelineEvent::Deleted { id, .. } => rec.timeline.get(&id)?.message.clone(),
                };
                Some(Event::DmMessage(DmMessage {
                    dm_id: dm.clone(),
                    cursor: Some(cursor),
                    message: Box::new(message),
                }))
            });
            Some(frames.collect())
        }
    }
}

fn channel_event(group: &LocalId, channel: &LocalId, logged: Logged) -> Event {
    let cursor = Some(logged.cursor);
    match logged.event {
        TimelineEvent::Created(m) => Event::MessageCreated(ChannelMessage {
            group_id: group.clone(),
            channel_id: channel.clone(),
            cursor,
            message: Box::new(m),
        }),
        TimelineEvent::Updated(m) => Event::MessageUpdated(ChannelMessage {
            group_id: group.clone(),
            channel_id: channel.clone(),
            cursor,
            message: Box::new(m),
        }),
        TimelineEvent::Deleted { id, at } => Event::MessageDeleted(MessageDeleted {
            group_id: group.clone(),
            channel_id: channel.clone(),
            cursor,
            message_id: id,
            deleted_at: Some(at),
        }),
    }
}
