//! Groups, channels and membership (§5.5, §5.7). Groups and channels are
//! seeded by the test (`FakeProvider::create_group`…), not created over
//! REST.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Router};
use ofscp_core::wire::{
    Channel, GroupPointer, JoinPolicy, JoinRequest, JoinRequestState, Member, Membership, Page,
    Paged, Tier,
};
use ofscp_core::ws::{Event, Unsubscribed};
use ofscp_core::{Actor, LocalId};

use super::me::{group_uri, groups_updated};
use super::respond::{Caller, Reply, json, no_content, ok, page_query};
use crate::inner::Inner;
use crate::ops::paging;
use crate::problem::{Outcome, Problem};
use crate::state::State as Store;
use crate::state::groups::{GroupRec, OWNER};

pub(super) fn routes() -> Router<Arc<Inner>> {
    Router::new()
        .route("/api/groups/{group}", get(group))
        .route("/api/groups/{group}/channels", get(channels))
        .route("/api/groups/{group}/channels/{channel}", get(channel))
        .route("/api/groups/{group}/members", get(members))
        .route("/api/groups/{group}/join", post(join))
        .route("/api/groups/{group}/leave", post(leave))
}

pub(crate) fn id(raw: &str) -> Outcome<LocalId> {
    LocalId::new(raw).map_err(|_| Problem::not_found("no such object"))
}

fn viewable<'s>(
    state: &'s Store,
    group: &LocalId,
    viewer: Option<&Actor>,
) -> Outcome<&'s GroupRec> {
    state
        .groups
        .get(group)
        .filter(|g| g.can_view(viewer))
        .ok_or_else(|| Problem::not_found("no such group"))
}

async fn group(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
) -> Reply {
    let gid = id(&raw)?;
    let state = inner.state.lock().unwrap();
    let g = viewable(&state, &gid, caller.viewer())?;
    let mut group = g.group.clone();
    group.membership = caller
        .viewer()
        .and_then(|a| g.members.get(a))
        .map(|m| Membership {
            role: m.role.clone(),
            joined_at: m.joined_at,
            display_name_override: m.display_name_override.clone(),
        });
    ok(&group)
}

/// The channels of `group` that `viewer` can read.
pub(crate) fn readable_channels(
    state: &Store,
    group: &LocalId,
    viewer: Option<&Actor>,
) -> Vec<Channel> {
    let Some(g) = state.groups.get(group) else {
        return Vec::new();
    };
    state
        .channels
        .values()
        .filter(|c| c.channel.group_id == *group && g.can_read(&c.channel, viewer))
        .map(|c| c.channel.clone())
        .collect()
}

async fn channels(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
) -> Reply {
    let gid = id(&raw)?;
    let state = inner.state.lock().unwrap();
    viewable(&state, &gid, caller.viewer())?;
    ok(&Paged {
        items: readable_channels(&state, &gid, caller.viewer()),
        page: Page::default(),
    })
}

async fn channel(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path((g, c)): Path<(String, String)>,
) -> Reply {
    let (gid, cid) = (id(&g)?, id(&c)?);
    let state = inner.state.lock().unwrap();
    let found = readable_channels(&state, &gid, caller.viewer())
        .into_iter()
        .find(|ch| ch.id == cid);
    found.map_or_else(|| Err(Problem::not_found("no such channel")), |ch| ok(&ch))
}

async fn members(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Reply {
    let gid = id(&raw)?;
    let state = inner.state.lock().unwrap();
    let g = viewable(&state, &gid, caller.viewer())?;
    let mut list: Vec<Member> = g.members.values().cloned().collect();
    list.sort_by_key(|m| m.joined_at);
    ok(&paging::page(
        &list,
        &page_query(&params, 50),
        &format!("mem-{}", gid.as_str()),
    )?)
}

async fn join(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
) -> Reply {
    let actor = caller.signed()?.clone();
    let gid = id(&raw)?;
    let now = inner.now();
    let mut state = inner.state.lock().unwrap();
    let ids_next = state.ids.next("jrq_");
    let g = state
        .groups
        .get_mut(&gid)
        .filter(|g| g.can_view(Some(&actor)))
        .ok_or_else(|| Problem::not_found("no such group"))?;
    if let Some(m) = g.members.get(&actor) {
        return ok(m);
    }
    let policy = if g.group.tier == Tier::Private {
        JoinPolicy::Invite
    } else {
        g.group.join_policy.clone()
    };
    match policy {
        JoinPolicy::Open => {}
        JoinPolicy::Request => {
            let request = JoinRequest {
                id: ids_next,
                group_id: gid.clone(),
                user: actor.clone(),
                state: JoinRequestState::Pending,
                message: None,
                requested_at: now,
            };
            g.requests.push(request.clone());
            return Ok(json(StatusCode::ACCEPTED, &request));
        }
        _ => return Err(Problem::forbidden("joining this group needs an invite")),
    }
    let member = Member {
        user: actor.clone(),
        role: "member".to_owned(),
        display_name_override: None,
        joined_at: now,
    };
    g.members.insert(actor.clone(), member.clone());
    drop(state);
    if actor.is_local_to(&inner.authority) {
        let entry = GroupPointer {
            group: group_uri(&inner, &gid),
            role: Some(member.role.clone()),
            updated_at: now,
        };
        groups_updated(&inner, &actor, vec![entry], Vec::new());
    }
    Ok(json(StatusCode::CREATED, &member))
}

async fn leave(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
) -> Reply {
    let actor = caller.signed()?.clone();
    let gid = id(&raw)?;
    let mut state = inner.state.lock().unwrap();
    let before = readable_channels(&state, &gid, Some(&actor));
    let g = state
        .groups
        .get_mut(&gid)
        .filter(|g| g.is_member(Some(&actor)))
        .ok_or_else(|| Problem::not_found("not a member of that group"))?;
    if g.role_of(Some(&actor)).is_some_and(|r| r == OWNER) {
        return Err(Problem::conflict(
            "the owner must transfer ownership before leaving",
        ));
    }
    g.members.remove(&actor);
    let after = readable_channels(&state, &gid, Some(&actor));
    drop(state);
    lost_access(&inner, &actor, &before, &after);
    if actor.is_local_to(&inner.authority) {
        groups_updated(&inner, &actor, Vec::new(), vec![group_uri(&inner, &gid)]);
    }
    no_content()
}

/// End the subscriptions to channels `actor` could read before and no
/// longer can (§7.1 *Losing access*).
pub(crate) fn lost_access(inner: &Inner, actor: &Actor, before: &[Channel], after: &[Channel]) {
    for c in before
        .iter()
        .filter(|c| !after.iter().any(|a| a.id == c.id))
    {
        let notice = Event::Unsubscribed(Unsubscribed {
            channels: vec![c.id.clone()],
            reason: Some("access_revoked".to_owned()),
        });
        inner.hub.revoke_access(actor, c.id.as_str(), &notice);
    }
}
