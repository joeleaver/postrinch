//! Channel history, edits, deletes and reactions over REST (§7.1, §7.2).
//! Posting a channel message is WebSocket-only (`message.create`): the
//! spec defines no REST create.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Extension, Router};
use ofscp_core::LocalId;
use ofscp_core::wire::{MessageUpdate, ReferenceKind, TimelineItem};
use serde::Deserialize;

use super::RequestBody;
use super::groups::id;
use super::respond::{Caller, Reply, body, json, no_content, ok, optional_body, page_query};
use crate::inner::Inner;
use crate::ops::messages::{self, check_readable};
use crate::ops::{Made, paging};
use crate::problem::Problem;
use crate::state::timeline::{PageQuery, Stored};

const MESSAGE: &str = "/api/groups/{group}/channels/{channel}/messages/{message}";

pub(super) fn routes() -> Router<Arc<Inner>> {
    Router::new()
        .route(
            "/api/groups/{group}/channels/{channel}/messages",
            get(history),
        )
        .route(MESSAGE, axum::routing::patch(edit).delete(remove))
        .route(&format!("{MESSAGE}/replies"), get(replies))
        .route(&format!("{MESSAGE}/reactions"), get(reactions))
        .route(
            &format!("{MESSAGE}/reactions/{{key}}"),
            put(react).delete(unreact),
        )
}

/// The body of `PUT .../reactions/{key}` (§7.1): what the key looks like.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ReactionBody {
    pub(crate) unicode: Option<String>,
    pub(crate) image: Option<String>,
}

type Ids3 = (String, String, String);
type Ids4 = (String, String, String, String);

/// A page of a channel's messages that `keep` selects, as `TimelineItem`s.
fn timeline_page(
    inner: &Inner,
    caller: &Caller,
    group: &str,
    channel: &str,
    query: &PageQuery,
    keep: impl Fn(&Stored) -> bool,
) -> Reply {
    let (gid, cid) = (id(group)?, id(channel)?);
    let mut state = inner.state.lock().unwrap();
    check_readable(&mut state, &gid, &cid, caller.viewer())?;
    let c = state.channels.get(&cid).expect("checked readable");
    let page = c.timeline.page(query, keep)?;
    ok(&ofscp_core::wire::Paged {
        items: page
            .items
            .into_iter()
            .map(|m| TimelineItem::Message(Box::new(m)))
            .collect(),
        page: page.page,
    })
}

async fn history(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path((g, c)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Reply {
    timeline_page(&inner, &caller, &g, &c, &page_query(&params, 50), |_| true)
}

async fn replies(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path((g, c, m)): Path<Ids3>,
    Query(params): Query<HashMap<String, String>>,
) -> Reply {
    let parent = id(&m)?;
    {
        let (gid, cid) = (id(&g)?, id(&c)?);
        let mut state = inner.state.lock().unwrap();
        check_readable(&mut state, &gid, &cid, caller.viewer())?;
        if state
            .channels
            .get(&cid)
            .and_then(|c| c.timeline.get(&parent))
            .is_none()
        {
            return Err(Problem::not_found("no such message"));
        }
    }
    let is_reply = move |s: &Stored| {
        s.message
            .reference
            .as_ref()
            .is_some_and(|r| r.kind == ReferenceKind::Reply && r.id == parent)
    };
    timeline_page(&inner, &caller, &g, &c, &page_query(&params, 50), is_reply)
}

async fn edit(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Extension(RequestBody(bytes)): Extension<RequestBody>,
    Path((g, c, m)): Path<Ids3>,
) -> Reply {
    let actor = caller.signed()?.clone();
    let update: MessageUpdate = body(&bytes)?;
    ok(&messages::update(
        &inner,
        &actor,
        &id(&g)?,
        &id(&c)?,
        &id(&m)?,
        update,
        None,
    )?)
}

async fn remove(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path((g, c, m)): Path<Ids3>,
) -> Reply {
    let actor = caller.signed()?.clone();
    messages::delete(&inner, &actor, &id(&g)?, &id(&c)?, &id(&m)?, None)?;
    no_content()
}

async fn reactions(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path((g, c, m)): Path<Ids3>,
    Query(params): Query<HashMap<String, String>>,
) -> Reply {
    let (gid, cid, mid) = (id(&g)?, id(&c)?, id(&m)?);
    let mut state = inner.state.lock().unwrap();
    check_readable(&mut state, &gid, &cid, caller.viewer())?;
    let stored = state
        .channels
        .get(&cid)
        .and_then(|c| c.timeline.get(&mid))
        .ok_or_else(|| Problem::not_found("no such message"))?;
    let tag = format!("rct-{}", mid.as_str());
    ok(&paging::page(
        &stored.reactions,
        &page_query(&params, 50),
        &tag,
    )?)
}

async fn react(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Extension(RequestBody(bytes)): Extension<RequestBody>,
    Path((g, c, m, key)): Path<Ids4>,
) -> Reply {
    let actor = caller.signed()?.clone();
    let looks: ReactionBody = optional_body(&bytes)?;
    let (gid, cid, mid): (LocalId, LocalId, LocalId) = (id(&g)?, id(&c)?, id(&m)?);
    match messages::react(
        &inner,
        &actor,
        &gid,
        &cid,
        &mid,
        &key,
        looks.unicode,
        looks.image,
        None,
    )? {
        Made::New(r) => Ok(json(StatusCode::CREATED, &r)),
        Made::Existing(r) => ok(&r),
    }
}

async fn unreact(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path((g, c, m, key)): Path<Ids4>,
) -> Reply {
    let actor = caller.signed()?.clone();
    messages::unreact(&inner, &actor, &id(&g)?, &id(&c)?, &id(&m)?, &key, None)?;
    no_content()
}
