//! The notification feed (§10.1): read again over REST after every
//! reconnect of the home session (nothing replays it), then kept as
//! `notification.created` arrives. The feed is optional for providers: one
//! that answers `404` has none.

use std::collections::HashSet;

use postrinch_ofscp::ofscp_core::wire::{ErrorCode, Notification};
use postrinch_store::SocialNotification;

use super::LiveEvent;
use super::session::Context;
use crate::SocialResult;

/// Read the feed newest first, down to an entry already kept (or one page,
/// for an account that keeps none yet), keeping each entry; entries
/// already kept are refreshed, as they may have been read elsewhere.
pub(crate) async fn catch_up(cx: &Context) -> SocialResult<()> {
    let account = cx.actor();
    let known: HashSet<String> = cx
        .social
        .blocking(move |store| store.social_notifications(&account, usize::MAX))
        .await?
        .into_iter()
        .map(|n| n.id)
        .collect();
    let home = cx.home();
    let mut cursor: Option<String> = None;
    loop {
        let result = home.notifications(None, None, cursor.as_deref()).await;
        if let Err(error) = &result
            && error.problem().is_some_and(|p| p.is(&ErrorCode::NotFound))
        {
            return Ok(());
        }
        let page = cx.checked(&home, result).await?;
        let reached = page.items.iter().any(|n| known.contains(n.id.as_str()));
        let new: Vec<String> = page
            .items
            .iter()
            .filter(|n| !known.contains(n.id.as_str()))
            .map(|n| n.id.to_string())
            .collect();
        keep(cx, page.items).await?;
        for id in new {
            cx.tell(LiveEvent::Notification {
                account: cx.actor(),
                id,
            });
        }
        match page.next_cursor {
            Some(next) if !reached && !known.is_empty() => cursor = Some(next),
            _ => return Ok(()),
        }
    }
}

/// Keep a notification that arrived live; one kept already (delivery is
/// at least once, §7.1) is not told again.
pub(crate) async fn created(cx: &Context, notification: Notification) -> SocialResult<()> {
    let (account, id) = (cx.actor(), notification.id.to_string());
    let lookup = id.clone();
    let known = cx
        .social
        .blocking(move |store| {
            let kept = store.social_notifications(&account, usize::MAX)?;
            Ok(kept.iter().any(|n| n.id == lookup))
        })
        .await?;
    keep(cx, vec![notification]).await?;
    if !known {
        cx.tell(LiveEvent::Notification {
            account: cx.actor(),
            id,
        });
    }
    Ok(())
}

async fn keep(cx: &Context, notifications: Vec<Notification>) -> SocialResult<()> {
    let account = cx.actor();
    cx.social
        .blocking(move |store| {
            for n in notifications {
                store.put_social_notification(&SocialNotification {
                    account: account.clone(),
                    id: n.id.to_string(),
                    created: n.created_at.as_millisecond(),
                    read: n.read_at.is_some(),
                    notification: serde_json::to_value(&n)
                        .expect("a wire notification always serializes"),
                })?;
            }
            Ok(())
        })
        .await
}
