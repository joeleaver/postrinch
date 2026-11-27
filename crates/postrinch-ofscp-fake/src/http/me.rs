//! The caller's account and profiles (§5.1, §6.2, §6.3), presence (§6.4)
//! and the My groups list (§5.7 *My groups*).
//!
//! The fake keeps no privacy settings (§6.6): every user's profile
//! extras and presence are visible to every viewer, as `public` allows.

use std::sync::Arc;

use axum::Extension;
use axum::Router;
use axum::extract::{Path, State};
use axum::routing::{get, patch, put};
use ofscp_core::wire::{
    GroupPointer, GroupPointerRequest, GroupPointersResponse, Patch, PresenceUpdate, ProfileUpdate,
    UserAccount, UserProfile,
};
use ofscp_core::ws::{Event, GroupsUpdated};
use ofscp_core::{Actor, LocalId, ObjectUri};
use percent_encoding::percent_decode_str;

use super::RequestBody;
use super::respond::{Caller, Reply, body, no_content, ok, optional_body};
use crate::inner::Inner;
use crate::problem::{Outcome, Problem};
use crate::state::{State as Store, User};
use crate::ws::hub::To;

pub(super) fn routes() -> Router<Arc<Inner>> {
    Router::new()
        .route("/api/me", get(account))
        .route("/api/me/profile", patch(update_profile))
        .route("/api/me/presence", put(set_presence))
        .route("/api/users/{user}/profile", get(profile))
        .route("/api/users/{user}/presence", get(presence))
        .route("/api/me/groups", get(my_groups))
        .route(
            "/api/me/groups/{group}",
            put(put_group).delete(delete_group),
        )
}

pub(crate) fn profile_of(inner: &Inner, user: &User) -> UserProfile {
    UserProfile {
        id: format!("{}/api/users/{}", inner.base_url(), user.actor.handle()),
        handle: user.actor.handle().to_owned(),
        domain: inner.authority.clone(),
        display_name: user.display_name.clone(),
        avatar: user.avatar.clone(),
        bio: user.bio.clone(),
        guest: None,
        expires_at: None,
        updated_at: user.updated_at,
        metadata: Vec::new(),
    }
}

/// A `{userRef}` (§2.2): a handle, an actor or a user URI, naming one of
/// this provider's users; anything else is `404`.
fn user_ref(inner: &Inner, raw: &str) -> Outcome<Actor> {
    let decoded = percent_decode_str(raw).decode_utf8_lossy();
    let missing = || Problem::not_found("no such user");
    let actor =
        if let Some(path) = decoded.strip_prefix(&format!("{}/api/users/", inner.base_url())) {
            Actor::new(path, inner.authority.clone()).map_err(|_| missing())?
        } else if decoded.contains('@') {
            Actor::canonicalize(&decoded).map_err(|_| missing())?
        } else {
            Actor::new(&decoded, inner.authority.clone()).map_err(|_| missing())?
        };
    let state = inner.state.lock().unwrap();
    state
        .local_user(&inner.authority, &actor)
        .ok_or_else(missing)?;
    Ok(actor)
}

async fn account(State(inner): State<Arc<Inner>>, Extension(caller): Extension<Caller>) -> Reply {
    let actor = caller.local(&inner)?;
    let state = inner.state.lock().unwrap();
    let user = state
        .local_user(&inner.authority, &actor)
        .ok_or_else(|| Problem::not_found("no such user"))?;
    ok(&UserAccount {
        profile: profile_of(&inner, user),
        settings: Default::default(),
    })
}

async fn update_profile(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Extension(RequestBody(bytes)): Extension<RequestBody>,
) -> Reply {
    let actor = caller.local(&inner)?;
    let update: ProfileUpdate = body(&bytes)?;
    if let Patch::Set(url) = &update.avatar
        && !url.starts_with("https://")
    {
        return Err(Problem::bad_request("avatar must be an https url"));
    }
    let now = inner.now();
    let mut state = inner.state.lock().unwrap();
    let user = state
        .local_user_mut(&inner.authority, &actor)
        .ok_or_else(|| Problem::not_found("no such user"))?;
    if let Some(name) = update.display_name {
        user.display_name = Some(name);
    }
    match update.avatar {
        Patch::Keep => {}
        Patch::Clear => user.avatar = None,
        Patch::Set(url) => user.avatar = Some(url),
    }
    if let Some(bio) = update.bio {
        user.bio = Some(bio);
    }
    user.updated_at = now;
    ok(&profile_of(&inner, user))
}

async fn profile(State(inner): State<Arc<Inner>>, Path(raw): Path<String>) -> Reply {
    let actor = user_ref(&inner, &raw)?;
    let state = inner.state.lock().unwrap();
    let user = state
        .local_user(&inner.authority, &actor)
        .ok_or_else(|| Problem::not_found("no such user"))?;
    ok(&profile_of(&inner, user))
}

async fn presence(State(inner): State<Arc<Inner>>, Path(raw): Path<String>) -> Reply {
    let actor = user_ref(&inner, &raw)?;
    let presence =
        crate::ws::presence_of(&inner, &actor).ok_or_else(|| Problem::not_found("no such user"))?;
    ok(&presence)
}

async fn set_presence(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Extension(RequestBody(bytes)): Extension<RequestBody>,
) -> Reply {
    let actor = caller.local(&inner)?;
    let update: PresenceUpdate = body(&bytes)?;
    crate::ws::set_presence(&inner, &actor, update.availability, update.status)?;
    let presence =
        crate::ws::presence_of(&inner, &actor).ok_or_else(|| Problem::not_found("no such user"))?;
    ok(&presence)
}

/// The URI of a group this provider hosts.
pub(crate) fn group_uri(inner: &Inner, group: &LocalId) -> ObjectUri {
    ObjectUri::parse(&format!(
        "{}/api/groups/{}",
        inner.base_url(),
        group.as_str()
    ))
    .expect("a group URI parses")
}

/// The caller's list: the groups hosted here they belong to, then their
/// pointers to groups elsewhere.
fn group_list(inner: &Inner, state: &Store, actor: &Actor) -> Vec<GroupPointer> {
    let mut list: Vec<GroupPointer> = state
        .groups
        .iter()
        .filter_map(|(id, g)| {
            let m = g.members.get(actor)?;
            Some(GroupPointer {
                group: group_uri(inner, id),
                role: Some(m.role.clone()),
                updated_at: m.joined_at,
            })
        })
        .collect();
    if let Some(user) = state.local_user(&inner.authority, actor) {
        list.extend(user.group_pointers.values().cloned());
    }
    list
}

/// `groups.updated` to all of `user`'s connections (§5.7).
pub(crate) fn groups_updated(
    inner: &Inner,
    user: &Actor,
    groups: Vec<GroupPointer>,
    removed: Vec<ObjectUri>,
) {
    if groups.is_empty() && removed.is_empty() {
        return;
    }
    let event = Event::GroupsUpdated(GroupsUpdated { groups, removed });
    inner.hub.publish(
        To::Actors(std::slice::from_ref(user), None),
        &event,
        None,
        1,
    );
}

async fn my_groups(State(inner): State<Arc<Inner>>, Extension(caller): Extension<Caller>) -> Reply {
    let actor = caller.local(&inner)?;
    let state = inner.state.lock().unwrap();
    ok(&GroupPointersResponse {
        groups: group_list(&inner, &state, &actor),
        metadata: Vec::new(),
    })
}

/// A `{groupRef}`: a group object URI, authority made canonical.
fn group_ref(raw: &str) -> Outcome<(ObjectUri, LocalId)> {
    let decoded = percent_decode_str(raw).decode_utf8_lossy();
    let bad = || Problem::bad_request("not a group object URI");
    let uri = ObjectUri::canonicalize(&decoded).map_err(|_| bad())?;
    let id = uri
        .path()
        .strip_prefix("/api/groups/")
        .filter(|id| !id.contains('/'))
        .ok_or_else(bad)?;
    let id = LocalId::new(id).map_err(|_| bad())?;
    Ok((uri, id))
}

async fn put_group(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Extension(RequestBody(bytes)): Extension<RequestBody>,
    Path(raw): Path<String>,
) -> Reply {
    let (uri, _) = group_ref(&raw)?;
    let req: GroupPointerRequest = optional_body(&bytes)?;
    let actor = caller.local(&inner)?;
    let now = inner.now();
    let mut state = inner.state.lock().unwrap();
    if *uri.authority() == inner.authority {
        let entry = group_list(&inner, &state, &actor)
            .into_iter()
            .find(|p| p.group == uri);
        return entry.map_or_else(
            || Err(Problem::not_found("not a member of that group")),
            |e| ok(&e),
        );
    }
    let user = state
        .local_user_mut(&inner.authority, &actor)
        .ok_or_else(|| Problem::not_found("no such user"))?;
    let key = uri.to_string();
    let changed = user
        .group_pointers
        .get(&key)
        .is_none_or(|p| p.role != req.role);
    if changed {
        user.group_pointers.insert(
            key.clone(),
            GroupPointer {
                group: uri.clone(),
                role: req.role,
                updated_at: now,
            },
        );
    }
    let pointer = user.group_pointers[&key].clone();
    drop(state);
    if changed {
        groups_updated(&inner, &actor, vec![pointer.clone()], Vec::new());
    }
    ok(&pointer)
}

async fn delete_group(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
) -> Reply {
    let (uri, id) = group_ref(&raw)?;
    let actor = caller.local(&inner)?;
    let mut state = inner.state.lock().unwrap();
    if *uri.authority() == inner.authority {
        let member = state
            .groups
            .get(&id)
            .is_some_and(|g| g.is_member(Some(&actor)));
        return if member {
            Err(Problem::conflict(
                "leave a group hosted here with POST .../leave",
            ))
        } else {
            no_content()
        };
    }
    let removed = state
        .local_user_mut(&inner.authority, &actor)
        .and_then(|u| u.group_pointers.remove(&uri.to_string()))
        .is_some();
    drop(state);
    if removed {
        groups_updated(&inner, &actor, Vec::new(), vec![uri]);
    }
    no_content()
}
