//! Follows (§7.6), read markers (§7.7) and the notification feed
//! (§10.1): signed local-account endpoints.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Extension, Router};
use ofscp_core::wire::{
    Follow, FollowCreateRequest, FollowsResponse, NotificationCounts, NotificationKind,
    NotificationsMarkResponse, NotificationsResponse, ReadMarker, ReadMarkersResponse,
    ReadMarkersUpdateRequest,
};
use ofscp_core::ws::{Event, ReadUpdated};
use ofscp_core::{Actor, DmId, LocalId, ObjectRef};
use percent_encoding::percent_decode_str;
use serde::Deserialize;

use super::RequestBody;
use super::respond::{Caller, Reply, body, created, no_content, ok, optional_body, page_query};
use crate::inner::Inner;
use crate::ops::paging;
use crate::problem::{Outcome, Problem};
use crate::state::State as Store;
use crate::state::timeline::Timeline;
use crate::ws::hub::To;

pub(super) fn routes() -> Router<Arc<Inner>> {
    Router::new()
        .route("/api/me/follows", get(follows).post(follow))
        .route("/api/me/follows/{channel}", delete(unfollow))
        .route(
            "/api/me/read-markers",
            get(read_markers).patch(move_markers),
        )
        .route("/api/me/notifications", get(notifications))
        .route("/api/me/notifications/seen", post(mark_seen))
        .route("/api/me/notifications/read", post(mark_read))
}

async fn follows(State(inner): State<Arc<Inner>>, Extension(caller): Extension<Caller>) -> Reply {
    let actor = caller.local(&inner)?;
    let state = inner.state.lock().unwrap();
    let user = state
        .local_user(&inner.authority, &actor)
        .ok_or_else(|| Problem::not_found("no such user"))?;
    ok(&FollowsResponse {
        follows: user.follows.clone(),
        metadata: Vec::new(),
    })
}

/// The local channel a ref names, if it names one of this provider's.
fn local_channel(inner: &Inner, r: &ObjectRef) -> Option<LocalId> {
    match r {
        ObjectRef::Local(id) => Some(id.clone()),
        ObjectRef::Uri(uri) if *uri.authority() == inner.authority => {
            let id = uri.path().rsplit('/').next()?;
            LocalId::new(id).ok()
        }
        ObjectRef::Uri(_) => None,
    }
}

async fn follow(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Extension(RequestBody(bytes)): Extension<RequestBody>,
) -> Reply {
    let actor = caller.local(&inner)?;
    let req: FollowCreateRequest = body(&bytes)?;
    let now = inner.now();
    let mut state = inner.state.lock().unwrap();
    if let Some(channel) = local_channel(&inner, &req.channel) {
        let readable = state.channels.get(&channel).is_some_and(|c| {
            state
                .groups
                .get(&c.channel.group_id)
                .is_some_and(|g| g.can_read(&c.channel, Some(&actor)))
        });
        if !readable {
            return Err(Problem::not_found("no such channel"));
        }
    }
    let user = state
        .local_user_mut(&inner.authority, &actor)
        .ok_or_else(|| Problem::not_found("no such user"))?;
    if let Some(existing) = user.follows.iter().find(|f| f.channel == req.channel) {
        return ok(existing);
    }
    let made = Follow {
        channel: req.channel,
        group_id: req.group_id,
        created_at: now,
        metadata: Vec::new(),
    };
    user.follows.push(made.clone());
    created(&made)
}

async fn unfollow(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
) -> Reply {
    let actor = caller.local(&inner)?;
    let decoded = percent_decode_str(&raw).decode_utf8_lossy();
    let target =
        ObjectRef::parse(&decoded).map_err(|_| Problem::bad_request("not a channel ref"))?;
    let mut state = inner.state.lock().unwrap();
    if let Some(user) = state.local_user_mut(&inner.authority, &actor) {
        user.follows.retain(|f| f.channel != target);
    }
    no_content()
}

/// Messages in `timeline` after `marker`, not by `user`, not deleted.
fn unread(timeline: &Timeline, marker: Option<&LocalId>, user: &Actor) -> u64 {
    let after = marker.and_then(|m| timeline.get(m)).map_or(0, |s| s.seq);
    timeline
        .messages()
        .filter(|s| s.seq > after && s.message.author != *user && s.message.deleted_at.is_none())
        .count() as u64
}

/// Every scope `GET /api/me/read-markers` lists for `user`, with the
/// timeline it reads.
fn scopes<'s>(
    inner: &Inner,
    state: &'s Store,
    user: &Actor,
) -> Vec<(LocalId, Option<LocalId>, Option<&'s Timeline>)> {
    let mut out = Vec::new();
    for (gid, g) in &state.groups {
        if !g.is_member(Some(user)) {
            continue;
        }
        for (cid, c) in &state.channels {
            if c.channel.group_id == *gid && g.can_read(&c.channel, Some(user)) {
                out.push((cid.clone(), Some(gid.clone()), Some(&c.timeline)));
            }
        }
    }
    for conversation in crate::ops::dms::conversations(inner, state, user) {
        let Ok(scope) = LocalId::new(conversation.id.as_str()) else {
            continue;
        };
        out.push((
            scope,
            None,
            state.dms.get(&conversation.id).map(|r| &r.timeline),
        ));
    }
    out
}

fn marker(
    state: &Store,
    inner: &Inner,
    user: &Actor,
    scope: LocalId,
    group: Option<LocalId>,
    t: Option<&Timeline>,
) -> ReadMarker {
    let last = state
        .local_user(&inner.authority, user)
        .and_then(|u| u.read_markers.get(&scope).cloned());
    ReadMarker {
        unread_count: t.map_or(0, |t| unread(t, last.as_ref(), user)),
        scope_id: scope,
        last_read_message_id: last,
        group_id: group,
    }
}

async fn read_markers(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
) -> Reply {
    let actor = caller.local(&inner)?;
    let state = inner.state.lock().unwrap();
    let list = scopes(&inner, &state, &actor)
        .into_iter()
        .map(|(s, g, t)| marker(&state, &inner, &actor, s, g, t))
        .collect();
    ok(&ReadMarkersResponse { scopes: list })
}

async fn move_markers(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Extension(RequestBody(bytes)): Extension<RequestBody>,
) -> Reply {
    let actor = caller.local(&inner)?;
    let req: ReadMarkersUpdateRequest = body(&bytes)?;
    if req.markers.is_empty() {
        return Err(Problem::bad_request("at least one marker"));
    }
    let mut state = inner.state.lock().unwrap();
    let mut advanced = Vec::new();
    for update in req.markers {
        let known = scopes(&inner, &state, &actor);
        let Some((scope, group, Some(timeline))) =
            known.into_iter().find(|(s, _, _)| *s == update.scope_id)
        else {
            continue;
        };
        let Some(new_seq) = timeline.get(&update.last_read_message_id).map(|s| s.seq) else {
            continue;
        };
        let current = state
            .local_user(&inner.authority, &actor)
            .and_then(|u| u.read_markers.get(&scope).cloned());
        let current_seq = current
            .as_ref()
            .and_then(|m| timeline.get(m))
            .map_or(0, |s| s.seq);
        if new_seq <= current_seq {
            continue;
        }
        if let Some(user) = state.local_user_mut(&inner.authority, &actor) {
            user.read_markers
                .insert(scope.clone(), update.last_read_message_id);
        }
        let t = if group.is_some() {
            state.channels.get(&scope).map(|c| &c.timeline)
        } else {
            DmId::parse(scope.as_str())
                .ok()
                .and_then(|d| state.dms.get(&d))
                .map(|r| &r.timeline)
        };
        advanced.push(marker(&state, &inner, &actor, scope, group, t));
    }
    drop(state);
    if !advanced.is_empty() {
        let event = Event::ReadUpdated(ReadUpdated {
            markers: advanced.clone(),
        });
        inner
            .hub
            .publish(To::Actors(&[actor], None), &event, None, 1);
    }
    ok(&ReadMarkersResponse { scopes: advanced })
}

fn counts(state: &Store, inner: &Inner, user: &Actor) -> Outcome<NotificationCounts> {
    let u = state
        .local_user(&inner.authority, user)
        .ok_or_else(|| Problem::not_found("no such user"))?;
    let unseen = |k: NotificationKind| {
        u.notifications
            .iter()
            .filter(|n| n.kind == k && n.seen_at.is_none())
            .count() as u64
    };
    Ok(NotificationCounts {
        mention: unseen(NotificationKind::Mention),
        reply: unseen(NotificationKind::Reply),
        message: Some(unseen(NotificationKind::Message)),
    })
}

async fn notifications(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Query(params): Query<HashMap<String, String>>,
) -> Reply {
    let actor = caller.local(&inner)?;
    let mut query = page_query(&params, 30);
    query.forward = false;
    let kind = params.get("type").and_then(|t| match t.as_str() {
        "mention" => Some(NotificationKind::Mention),
        "reply" => Some(NotificationKind::Reply),
        "message" => Some(NotificationKind::Message),
        _ => None,
    });
    let state = inner.state.lock().unwrap();
    let user = state
        .local_user(&inner.authority, &actor)
        .ok_or_else(|| Problem::not_found("no such user"))?;
    let list: Vec<_> = user
        .notifications
        .iter()
        .filter(|n| kind.as_ref().is_none_or(|k| n.kind == *k))
        .cloned()
        .collect();
    let page = paging::page(&list, &query, "ntf")?;
    ok(&NotificationsResponse {
        items: page.items,
        counts: counts(&state, &inner, &actor)?,
        next_cursor: page.page.next_cursor,
    })
}

#[derive(Debug, Default, Deserialize)]
struct MarkRequest {
    #[serde(default)]
    ids: Option<Vec<LocalId>>,
}

async fn mark(inner: Arc<Inner>, caller: Caller, bytes: bytes::Bytes, read: bool) -> Reply {
    let actor = caller.local(&inner)?;
    let req: MarkRequest = optional_body(&bytes)?;
    let now = inner.now();
    let mut state = inner.state.lock().unwrap();
    let user = state
        .local_user_mut(&inner.authority, &actor)
        .ok_or_else(|| Problem::not_found("no such user"))?;
    let ids = req.ids.filter(|ids| !ids.is_empty());
    let mut affected = 0;
    for n in user
        .notifications
        .iter_mut()
        .filter(|n| ids.as_ref().is_none_or(|ids| ids.contains(&n.id)))
    {
        let mut changed = false;
        if n.seen_at.is_none() {
            n.seen_at = Some(now);
            changed = true;
        }
        if read && n.read_at.is_none() {
            n.read_at = Some(now);
            changed = true;
        }
        affected += u64::from(changed);
    }
    ok(&NotificationsMarkResponse {
        affected,
        counts: counts(&state, &inner, &actor)?,
    })
}

async fn mark_seen(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Extension(RequestBody(bytes)): Extension<RequestBody>,
) -> Reply {
    mark(inner, caller, bytes, false).await
}

async fn mark_read(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Extension(RequestBody(bytes)): Extension<RequestBody>,
) -> Reply {
    mark(inner, caller, bytes, true).await
}
