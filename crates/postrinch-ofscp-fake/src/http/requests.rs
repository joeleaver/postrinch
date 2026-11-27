//! Join requests (§5.7, `request` policy): listed, approved and denied by
//! the group's managers or moderators. Approving makes a `member`,
//! whoever approves, and the subset rule does not apply.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Extension, Router};
use ofscp_core::wire::{JoinPolicy, JoinRequest, JoinRequestState};
use ofscp_core::{Actor, LocalId};

use super::groups::{admit, announce_member, id};
use super::respond::{Caller, Reply, no_content, ok, page_query};
use crate::inner::Inner;
use crate::ops::paging;
use crate::problem::{Outcome, Problem};
use crate::state::State as Store;
use crate::state::groups::GroupRec;

pub(super) fn routes() -> Router<Arc<Inner>> {
    Router::new()
        .route("/api/groups/{group}/requests", get(list))
        .route(
            "/api/groups/{group}/requests/{request}/approve",
            post(approve),
        )
        .route("/api/groups/{group}/requests/{request}/deny", post(deny))
}

/// The group, for a caller who may manage or moderate it.
fn overseen<'s>(state: &'s Store, group: &LocalId, actor: &Actor) -> Outcome<&'s GroupRec> {
    let g = state
        .groups
        .get(group)
        .filter(|g| g.can_view(Some(actor)))
        .ok_or_else(|| Problem::not_found("no such group"))?;
    if !g.can_manage(actor) && !g.can_moderate(actor) {
        return Err(Problem::forbidden(
            "join requests need the manage or moderate role",
        ));
    }
    Ok(g)
}

/// The pending request `request` of `g`.
fn pending<'g>(g: &'g GroupRec, request: &LocalId) -> Outcome<&'g JoinRequest> {
    g.requests
        .iter()
        .find(|r| r.id == *request && r.state == JoinRequestState::Pending)
        .ok_or_else(|| Problem::not_found("no such join request"))
}

async fn list(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Reply {
    let actor = caller.signed()?.clone();
    let gid = id(&raw)?;
    let state = inner.state.lock().unwrap();
    let g = overseen(&state, &gid, &actor)?;
    let list: Vec<JoinRequest> = g
        .requests
        .iter()
        .filter(|r| r.state == JoinRequestState::Pending)
        .cloned()
        .collect();
    ok(&paging::page(
        &list,
        &page_query(&params, 50),
        &format!("jrq-{}", gid.as_str()),
    )?)
}

async fn approve(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path((g, r)): Path<(String, String)>,
) -> Reply {
    let actor = caller.signed()?.clone();
    let (gid, rid) = (id(&g)?, id(&r)?);
    let mut state = inner.state.lock().unwrap();
    let group = overseen(&state, &gid, &actor)?;
    let user = pending(group, &rid)?.user.clone();
    // The policy first, before the already-a-member case (§5.7).
    if group.effective_policy() == JoinPolicy::Invite {
        return Err(Problem::conflict("the group no longer takes join requests"));
    }
    let existing = group.members.get(&user).cloned();
    let joined_now = existing.is_none();
    let member = match existing {
        Some(member) => member,
        None => admit(&inner, &mut state, &gid, &user, "member"),
    };
    let group = state.groups.get_mut(&gid).expect("the group exists");
    if let Some(request) = group.requests.iter_mut().find(|q| q.id == rid) {
        request.state = JoinRequestState::Approved;
    }
    drop(state);
    if joined_now {
        announce_member(&inner, &gid, &member);
    }
    ok(&member)
}

async fn deny(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path((g, r)): Path<(String, String)>,
) -> Reply {
    let actor = caller.signed()?.clone();
    let (gid, rid) = (id(&g)?, id(&r)?);
    let mut state = inner.state.lock().unwrap();
    pending(overseen(&state, &gid, &actor)?, &rid)?;
    let group = state.groups.get_mut(&gid).expect("the group exists");
    if let Some(request) = group.requests.iter_mut().find(|q| q.id == rid) {
        request.state = JoinRequestState::Denied;
    }
    no_content()
}
