//! Direct messages over REST (§7.4, §8.3): delivery to the recipient's
//! home, the caller's conversation pointers and list, and history.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Extension, Router};
use ofscp_core::wire::{
    ConversationPointer, DmMessageCreate, MessageUpdate, Paged, ReferenceKind, TimelineItem,
};
use ofscp_core::{Actor, DmId};

use super::RequestBody;
use super::groups::id;
use super::messages::ReactionBody;
use super::respond::{Caller, Reply, body, json, no_content, ok, optional_body, page_query};
use crate::inner::Inner;
use crate::ops::dms::{self, conversations, is_participant, parse_dm};
use crate::ops::{Made, paging};
use crate::problem::{Outcome, Problem};
use crate::state::State as Store;
use crate::state::timeline::{PageQuery, Stored};

const FED: &str = "/api/federation/dms/{dm}/messages";
const DM: &str = "/api/dms/{dm}/messages";

pub(super) fn routes() -> Router<Arc<Inner>> {
    Router::new()
        .route(FED, post(deliver))
        .route(
            &format!("{FED}/{{message}}"),
            axum::routing::patch(edit).delete(remove),
        )
        .route(
            &format!("{FED}/{{message}}/reactions/{{key}}"),
            put(react).delete(unreact),
        )
        .route("/api/me/dms", get(list))
        .route("/api/me/dms/{dm}", put(pointer))
        .route(DM, get(history))
        .route(&format!("{DM}/{{message}}/replies"), get(replies))
        .route(&format!("{DM}/{{message}}/reactions"), get(reactions))
        .route(
            &format!("{DM}/{{message}}/reactions/{{key}}"),
            put(react).delete(unreact),
        )
}

async fn deliver(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Extension(RequestBody(bytes)): Extension<RequestBody>,
    Path(raw): Path<String>,
) -> Reply {
    let author = caller.signed()?.clone();
    let dm = parse_dm(&raw)?;
    let create: DmMessageCreate = body(&bytes)?;
    match dms::send(&inner, &author, &dm, create)? {
        Made::New(m) => Ok(json(StatusCode::CREATED, &m)),
        Made::Existing(m) => ok(&m),
    }
}

async fn edit(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Extension(RequestBody(bytes)): Extension<RequestBody>,
    Path((d, m)): Path<(String, String)>,
) -> Reply {
    let actor = caller.signed()?.clone();
    let update: MessageUpdate = body(&bytes)?;
    let changed = dms::change(&inner, &actor, &parse_dm(&d)?, &id(&m)?, Some(update))?;
    ok(&changed.expect("an edit returns the message"))
}

async fn remove(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path((d, m)): Path<(String, String)>,
) -> Reply {
    let actor = caller.signed()?.clone();
    dms::change(&inner, &actor, &parse_dm(&d)?, &id(&m)?, None)?;
    no_content()
}

async fn react(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Extension(RequestBody(bytes)): Extension<RequestBody>,
    Path((d, m, key)): Path<(String, String, String)>,
) -> Reply {
    let actor = caller.signed()?.clone();
    let looks: ReactionBody = optional_body(&bytes)?;
    let (dm, mid) = (parse_dm(&d)?, id(&m)?);
    let had = {
        let state = inner.state.lock().unwrap();
        state
            .dms
            .get(&dm)
            .and_then(|r| r.timeline.get(&mid))
            .is_some_and(|s| {
                s.reactions
                    .iter()
                    .any(|r| r.author == actor && r.key == key)
            })
    };
    let reaction = dms::react(
        &inner,
        &actor,
        &dm,
        &mid,
        &key,
        Some((looks.unicode, looks.image)),
    )?
    .expect("adding returns the reaction");
    let status = if had {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok(json(status, &reaction))
}

async fn unreact(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path((d, m, key)): Path<(String, String, String)>,
) -> Reply {
    let actor = caller.signed()?.clone();
    dms::react(&inner, &actor, &parse_dm(&d)?, &id(&m)?, &key, None)?;
    no_content()
}

async fn pointer(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Extension(RequestBody(bytes)): Extension<RequestBody>,
    Path(raw): Path<String>,
) -> Reply {
    let actor = caller.local(&inner)?;
    let dm = parse_dm(&raw)?;
    let body: ConversationPointer = body(&bytes)?;
    ok(&dms::put_pointer(&inner, &actor, &dm, body)?)
}

/// `GET /api/me/dms`: newest activity first (the backward default).
async fn list(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Query(params): Query<HashMap<String, String>>,
) -> Reply {
    let actor = caller.local(&inner)?;
    let state = inner.state.lock().unwrap();
    let all = conversations(&inner, &state, &actor);
    let tag = format!("dms-{}", actor.handle());
    ok(&paging::page(&all, &page_query(&params, 50), &tag)?)
}

/// `403` for a caller outside the conversation, `404` for a conversation
/// this provider knows nothing of.
fn participant(inner: &Inner, state: &Store, dm: &DmId, caller: &Actor) -> Outcome<()> {
    if is_participant(inner, state, dm, caller) || state.dms.get(dm).is_some_and(|r| r.has(caller))
    {
        Ok(())
    } else if state.dms.contains_key(dm) {
        Err(Problem::forbidden("not a participant of this conversation"))
    } else {
        Err(Problem::not_found("no such conversation"))
    }
}

/// A page of a conversation's messages here that `keep` selects. A
/// participant whose provider holds none (the sender's home) gets an
/// empty page.
fn dm_page(
    inner: &Inner,
    caller: &Caller,
    raw: &str,
    query: &PageQuery,
    keep: impl Fn(&Stored) -> bool,
) -> Reply {
    let actor = caller.signed()?;
    let dm = parse_dm(raw)?;
    let state = inner.state.lock().unwrap();
    participant(inner, &state, &dm, actor)?;
    let page = match state.dms.get(&dm) {
        Some(r) => r.timeline.page(query, keep)?,
        None => Paged {
            items: Vec::new(),
            page: Default::default(),
        },
    };
    ok(&Paged {
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
    Path(raw): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Reply {
    dm_page(&inner, &caller, &raw, &page_query(&params, 50), |_| true)
}

async fn replies(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path((d, m)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Reply {
    let parent = id(&m)?;
    let is_reply = move |s: &Stored| {
        s.message
            .reference
            .as_ref()
            .is_some_and(|r| r.kind == ReferenceKind::Reply && r.id == parent)
    };
    dm_page(&inner, &caller, &d, &page_query(&params, 50), is_reply)
}

async fn reactions(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    Path((d, m)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Reply {
    let actor = caller.signed()?;
    let (dm, mid) = (parse_dm(&d)?, id(&m)?);
    let state = inner.state.lock().unwrap();
    participant(&inner, &state, &dm, actor)?;
    let stored = state
        .dms
        .get(&dm)
        .and_then(|r| r.timeline.get(&mid))
        .ok_or_else(|| Problem::not_found("no such message"))?;
    let tag = format!("rct-{}", mid.as_str());
    ok(&paging::page(
        &stored.reactions,
        &page_query(&params, 50),
        &tag,
    )?)
}
