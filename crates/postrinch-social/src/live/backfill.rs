//! A place's backfill over REST (§7.2): what a resume could not replay
//! (`truncated`), or everything since the newest page for a place that
//! had no cursor.
//!
//! It reads pages newest first until it reaches a message already kept,
//! so the place has no gap: everything from the newest message back to
//! what was here is kept, and history older than that is left to
//! scrolling. A place with nothing kept gets its newest page, as on the
//! first sync. The place resumes from the newest message afterwards.

use std::collections::HashSet;

use postrinch_ofscp::http::{Client, Direction, PageRequest};
use postrinch_ofscp::ofscp_core::wire::MessagesPage;
use postrinch_store::social_account_key;

use super::apply;
use super::session::Context;
use super::subscribe::Scope;
use super::{LiveEvent, MessageChange};
use crate::SocialResult;
use crate::refused;
use crate::sync::message::{Held, Kept, Reader, held_by, keep};

/// Backfill `scope`; how many messages were new.
pub(crate) async fn backfill(cx: &Context, scope: &Scope) -> SocialResult<usize> {
    let (account, place) = (cx.actor(), scope.place().to_owned());
    let found = cx
        .social
        .blocking(move |store| {
            let Some(kept) = store.social_place(&account, &place)? else {
                return Ok(None);
            };
            let ids: HashSet<String> = store
                .social_place_messages(&account, &place)?
                .into_iter()
                .map(|m| m.id)
                .collect();
            Ok(Some((kept, ids)))
        })
        .await?;
    let Some((kept, had)) = found else {
        return Ok(0);
    };
    let client = cx.client();
    let me = social_account_key(&cx.actor());
    let mut walk = Walk {
        marker: kept.read_marker.clone(),
        passed_marker: false,
    };
    let mut request = PageRequest::newest();
    let (mut live, mut first_older, mut reached_start) = (None, None, false);
    let mut first = true;
    let mut new = 0;
    loop {
        let Some(page) = fetch(cx, &client, scope, &request).await? else {
            break;
        };
        let next = page.page.next_cursor.clone();
        if first {
            live = page.page.prev_cursor.clone();
            first_older = next.clone();
        }
        let held = held_by(&cx.authority, page.items);
        let unread = walk.unread(&held, &me);
        let reader = Reader {
            account: cx.identity.actor(),
            place: scope.place(),
            host: &cx.authority,
        };
        let kept_page: Vec<Kept> = held
            .iter()
            .zip(unread)
            .map(|(h, unread)| keep(&reader, h, unread))
            .collect();
        // Compared by the key each is kept under, which names the
        // provider holding it: a private conversation keeps the other
        // side's messages under their URL there, and this provider's
        // pages are never matched against them.
        let known = kept_page.iter().any(|k| had.contains(&k.message.id));
        new += keep_page(cx, &had, kept_page).await?;
        first = false;
        let Some(next) = next else {
            reached_start = !known;
            break;
        };
        if known || had.is_empty() {
            break;
        }
        request = PageRequest::after(next, Direction::Backward);
    }
    let (account, place, first_seen) = (cx.actor(), scope.place().to_owned(), had.is_empty());
    cx.social
        .blocking(move |store| {
            store.change_social_place(&account, &place, |p| {
                if live.is_some() {
                    p.cursors.live = live;
                }
                if first_seen && p.cursors.older.is_none() && !p.cursors.complete {
                    p.cursors.complete = first_older.is_none();
                    p.cursors.older = first_older;
                } else if reached_start {
                    p.cursors.complete = true;
                    p.cursors.older = None;
                }
            })
        })
        .await?;
    tracing::info!(provider = %cx.authority, place = %scope.place(), new, "backfilled");
    Ok(new)
}

/// One page of `scope`, `None` when it is out of reach (`403`, `404`):
/// what is kept stays, and the next refresh decides whether it goes.
async fn fetch(
    cx: &Context,
    client: &Client,
    scope: &Scope,
    request: &PageRequest,
) -> SocialResult<Option<MessagesPage>> {
    let result = match scope {
        Scope::Channel { group, channel, .. } => client.messages(group, channel, request).await,
        Scope::Conversation { dm, .. } => client.dm_messages(dm, request).await,
    };
    match result {
        Err(error) if refused::is_out_of_reach(&error) => {
            tracing::info!(provider = %cx.authority, place = %scope.place(), %error, "cannot backfill");
            Ok(None)
        }
        result => cx.checked(client, result).await.map(Some),
    }
}

/// Keep a page; how many of its messages were new. Each new one is told.
async fn keep_page(cx: &Context, had: &HashSet<String>, page: Vec<Kept>) -> SocialResult<usize> {
    let mut told: Vec<LiveEvent> = page
        .iter()
        .filter(|k| !had.contains(&k.message.id))
        .map(|k| LiveEvent::Message {
            account: k.message.account.clone(),
            place: k.message.place.clone(),
            id: k.message.id.clone(),
            change: MessageChange::New,
            unread: k.message.unread,
            mentions_me: k.message.mentions_me,
        })
        .collect();
    let (account, place) = (cx.actor(), page.first().map(|k| k.message.place.clone()));
    let sent = cx
        .social
        .blocking(move |store| {
            let (mut newest, mut sent) = (None, Vec::new());
            for k in &page {
                // Reactions come with the page (§7.2), so they are fresh.
                let put = store.put_social_message(&k.message, &k.search_text)?;
                sent.extend(apply::sent(&k.message, &put));
                newest = newest.max(Some(k.message.created));
            }
            if let Some(place) = place {
                store.change_social_place(&account, &place, |p| {
                    p.last_activity = p.last_activity.max(newest);
                })?;
            }
            Ok(sent)
        })
        .await?;
    let count = told.len();
    told.extend(sent);
    for event in told {
        cx.tell(event);
    }
    Ok(count)
}

/// Which messages are unread, walking a place from its newest message
/// back: those after the read marker, unless the person wrote them.
struct Walk {
    marker: Option<String>,
    /// Whether the marker's message was passed: everything older is read.
    passed_marker: bool,
}

impl Walk {
    /// Whether each of a page's messages (oldest first) is unread.
    fn unread(&mut self, held: &[Held], me: &str) -> Vec<bool> {
        let mut unread = vec![false; held.len()];
        for (i, h) in held.iter().enumerate().rev() {
            if self.marker.as_deref() == Some(h.message.id.as_str()) {
                self.passed_marker = true;
            }
            let mine = social_account_key(&h.message.author.to_string()) == me;
            unread[i] = !self.passed_marker && !mine;
        }
        unread
    }
}

#[cfg(test)]
mod tests {
    use postrinch_ofscp::ofscp_core::Authority;
    use postrinch_ofscp::ofscp_core::wire::Message;
    use serde_json::json;

    use super::*;

    fn held(id: &str, author: &str) -> Held {
        let message: Message = serde_json::from_value(json!({
            "id": id, "type": "message", "author": author,
            "createdAt": "2026-01-01T00:00:00Z",
            "content": { "text": "hi", "mime": "text/plain" },
        }))
        .unwrap();
        Held {
            holder: Authority::canonicalize("a.example").unwrap(),
            message,
            reactions: Vec::new(),
        }
    }

    #[test]
    fn what_is_after_the_marker_is_unread_across_pages() {
        let mut walk = Walk {
            marker: Some("m2".into()),
            passed_marker: false,
        };
        let me = "ada@a.example";
        let newest = [held("m3", "bob@a.example"), held("m4", "ada@a.example")];
        assert_eq!(walk.unread(&newest, me), [true, false]);
        let older = [held("m1", "bob@a.example"), held("m2", "bob@a.example")];
        assert_eq!(walk.unread(&older, me), [false, false]);
    }

    #[test]
    fn without_a_marker_everything_from_others_is_unread() {
        let mut walk = Walk {
            marker: None,
            passed_marker: false,
        };
        let page = [held("m1", "bob@a.example"), held("m2", "ada@a.example")];
        assert_eq!(walk.unread(&page, "ada@a.example"), [true, false]);
    }
}
