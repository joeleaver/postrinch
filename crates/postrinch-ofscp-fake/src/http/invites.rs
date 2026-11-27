//! Invites and join links (§5.6): made, listed and revoked by a group's
//! managers, and redeemed by anyone holding the token, local or remote.
//! The token is in the answer that makes the invite and nowhere else;
//! the fake keeps only its SHA-256.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{delete, post};
use axum::{Extension, Router};
use bytes::Bytes;
use ofscp_core::wire::{Invite, InviteCreateRequest, InviteCreateResponse, InviteRedeemResponse};
use ofscp_core::{Actor, LocalId};

use super::groups::{admit, announce_member, id};
use super::respond::{Caller, Reply, created, no_content, ok, optional_body, page_query};
use crate::inner::Inner;
use crate::ops::paging;
use crate::problem::{Outcome, Problem};
use crate::random;
use crate::state::State as Store;
use crate::state::groups::GroupRec;
use crate::state::invites::{InviteRec, digest, grantable, holds_all_of};

/// The role an invite grants a full account when it names none.
const MEMBER: &str = "member";

pub(super) fn routes() -> Router<Arc<Inner>> {
    Router::new()
        .route(
            "/api/groups/{group}/invites",
            post(create_invite).get(list_invites),
        )
        .route("/api/groups/{group}/invites/{invite}", delete(revoke))
        .route("/api/invites/{token}/redeem", post(redeem))
}

/// The group, for a caller who may manage it; `404` when there is no such
/// group for them, `403` when they may see it but not manage it.
fn managed<'s>(state: &'s Store, group: &LocalId, actor: &Actor) -> Outcome<&'s GroupRec> {
    let g = state
        .groups
        .get(group)
        .filter(|g| g.can_view(Some(actor)))
        .ok_or_else(|| Problem::not_found("no such group"))?;
    if !g.can_manage(actor) {
        return Err(Problem::forbidden("managing invites needs the manage role"));
    }
    Ok(g)
}

async fn create_invite(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
    bytes: Bytes,
) -> Reply {
    let actor = caller.signed()?.clone();
    let gid = id(&raw)?;
    let request: InviteCreateRequest = optional_body(&bytes)?;
    let mut state = inner.state.lock().unwrap();
    let g = managed(&state, &gid, &actor)?;
    let role = request.role.clone().unwrap_or_else(|| MEMBER.to_owned());
    if !grantable(&g.group, &role) {
        return Err(Problem::bad_request(format!(
            "an invite cannot grant {role:?}"
        )));
    }
    if let Some(channel) = &request.channel_id
        && state.channel(&gid, channel).is_none()
    {
        return Err(Problem::bad_request(
            "that channel is not one of the group's",
        ));
    }
    let creator_role = g.role_of(Some(&actor)).cloned().unwrap_or_default();
    if !holds_all_of(&g.group, &creator_role, &role) {
        return Err(Problem::forbidden(
            "an invite cannot grant more than its creator holds",
        ));
    }
    let token = random::token(16);
    let invite = Invite {
        id: state.ids.next("inv_"),
        group_id: gid,
        channel_id: request.channel_id,
        token: None,
        role: request.role,
        grants_guest: request.grants_guest,
        max_uses: request.max_uses,
        uses: 0,
        expires_at: request.expires_at,
        created_by: actor,
        created_at: inner.now(),
    };
    state.invites.insert(
        invite.id.clone(),
        InviteRec {
            invite: invite.clone(),
            digest: digest(&token),
        },
    );
    created(&with_token(invite, token))
}

/// The answer that makes an invite: the invite, and its token.
fn with_token(invite: Invite, token: String) -> InviteCreateResponse {
    InviteCreateResponse {
        id: invite.id,
        group_id: invite.group_id,
        channel_id: invite.channel_id,
        token,
        role: invite.role,
        grants_guest: invite.grants_guest,
        max_uses: invite.max_uses,
        uses: invite.uses,
        expires_at: invite.expires_at,
        created_by: invite.created_by,
        created_at: invite.created_at,
    }
}

async fn list_invites(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Reply {
    let actor = caller.signed()?.clone();
    let gid = id(&raw)?;
    let state = inner.state.lock().unwrap();
    managed(&state, &gid, &actor)?;
    // Oldest first, by `createdAt` and then id (re-ofscp#43), so paging
    // backward gives the newest first.
    let mut list: Vec<Invite> = state
        .invites
        .values()
        .filter(|r| r.invite.group_id == gid)
        .map(|r| r.invite.clone())
        .collect();
    list.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    ok(&paging::page(
        &list,
        &page_query(&params, 50),
        &format!("inv-{}", gid.as_str()),
    )?)
}

async fn revoke(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path((g, i)): Path<(String, String)>,
) -> Reply {
    let actor = caller.signed()?.clone();
    let (gid, iid) = (id(&g)?, id(&i)?);
    let mut state = inner.state.lock().unwrap();
    managed(&state, &gid, &actor)?;
    let ours = state
        .invites
        .get(&iid)
        .is_some_and(|r| r.invite.group_id == gid);
    if !ours {
        return Err(Problem::not_found("no such invite"));
    }
    state.invites.remove(&iid);
    no_content()
}

async fn redeem(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path(token): Path<String>,
) -> Reply {
    let actor = caller.signed()?.clone();
    let now = inner.now();
    let mut state = inner.state.lock().unwrap();
    // The invite first, whoever the caller is (§5.6).
    let wanted = digest(&token);
    let rec = state
        .invites
        .values()
        .find(|r| r.digest == wanted)
        .filter(|r| !r.expired(now))
        .cloned()
        .ok_or_else(|| Problem::not_found("no such invite"))?;
    if rec.exhausted() {
        return Err(Problem::conflict("this invite has been used up"));
    }
    let invite = rec.invite;
    let gid = invite.group_id.clone();
    if !state.groups.contains_key(&gid) {
        return Err(Problem::not_found("no such invite"));
    }
    // A channel deleted since is left out of the answer (§5.6).
    let channel_id = invite
        .channel_id
        .clone()
        .filter(|c| state.channel(&gid, c).is_some());
    if let Some(role) = state.groups[&gid].role_of(Some(&actor)).cloned() {
        return ok(&InviteRedeemResponse {
            group_id: gid,
            channel_id,
            role,
        });
    }
    let role = invite.role.clone().unwrap_or_else(|| MEMBER.to_owned());
    let member = admit(&inner, &mut state, &gid, &actor, &role);
    if let Some(rec) = state.invites.get_mut(&invite.id) {
        rec.invite.uses += 1;
    }
    drop(state);
    announce_member(&inner, &gid, &member);
    ok(&InviteRedeemResponse {
        group_id: gid,
        channel_id,
        role: member.role,
    })
}
