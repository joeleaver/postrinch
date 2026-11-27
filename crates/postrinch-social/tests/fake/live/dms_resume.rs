//! Following sent messages at the other person's provider across a lost
//! socket and a restart: the `dmId` subscription there resumes from its
//! own cursor (§7.4), so what changed meanwhile is replayed, not read
//! again page by page.

use std::time::Duration;

use postrinch_ofscp::ofscp_core::wire::MessageUpdate;
use postrinch_social::{LiveEvent, MessageChange};
use serde_json::json;

use super::dms::{away, caught_up_at, held_id, to};
use super::{Told, live, options};
use crate::common::{Home, home, requests_to};
use crate::outbox::{chat, kept_sent, queue, until_sent};

fn edit(text: &str) -> MessageUpdate {
    serde_json::from_value(json!({ "content": { "text": text, "mime": "text/plain" } })).unwrap()
}

/// The first event `wanted` matches among those read since `from`, or,
/// if none, the next one: events from two providers come in no set order.
async fn since_or_next(
    told: &mut Told,
    from: usize,
    what: &str,
    wanted: impl Fn(&LiveEvent) -> bool,
) -> LiveEvent {
    match told.seen[from..].iter().find(|e| wanted(e)) {
        Some(event) => event.clone(),
        None => told.until(what, wanted).await,
    }
}

fn is_edit(event: &LiveEvent, id: &str) -> bool {
    matches!(event, LiveEvent::Message { id: told, change: MessageChange::Edited, .. } if told == id)
}

/// Ada live with a conversation with Erin in which each wrote once, so
/// both providers hold something and each side has a cursor. Ada's
/// message, as kept.
async fn talking(h: &Home, away: &super::dms::Away) -> (postrinch_social::Social, Told, String) {
    away.erin_at_ada
        .send_dm(&away.dm, &to(&h.ada, "e1", "hi Ada", None))
        .await
        .unwrap();
    // Erin wrote first, so the first sync keeps the conversation, and a
    // session is opened at each provider.
    let (social, mut told) = live(h, 2).await;
    let place = away.dm.to_string();
    queue(h, "a1", &place, &chat("hi Erin", None));
    social.outbox_changed(&h.account());
    until_sent(h).await;
    let mine = kept_sent(h, &place, "a1").id;
    // Wait for the copy from Erin's provider, so its cursor is past it.
    told.settle(Duration::from_millis(200)).await;
    (social, told, mine)
}

#[tokio::test]
async fn a_lost_socket_at_the_other_provider_resumes_from_its_cursor() {
    let h = home().await;
    let away = away(&h).await;
    let (_social, mut told, mine) = talking(&h, &away).await;
    let pages = format!("/api/dms/{}/messages", away.dm);
    let read_before = requests_to(&away.provider, &pages).len();

    away.provider.faults().refuse_ws_auth(true);
    away.provider.faults().drop_sockets();
    told.until("the session at Erin's ending", |e| {
        let authority = away.provider.authority().to_string();
        matches!(e, LiveEvent::Disconnected { provider, retry_in: Some(_), .. } if *provider == authority)
    })
    .await;
    away.phone_away
        .edit_dm(&away.dm, &held_id(&mine), &edit("hello Erin"))
        .await
        .unwrap();
    away.provider.faults().refuse_ws_auth(false);
    let from = told.seen.len();
    told.until("the edit, replayed", |e| is_edit(e, &mine))
        .await;
    let caught = since_or_next(&mut told, from, "catching up at Erin's", |e| {
        caught_up_at(e, &away.provider)
    })
    .await;
    match caught {
        LiveEvent::CaughtUp { backfilled, .. } => assert!(backfilled.is_empty(), "{backfilled:?}"),
        _ => unreachable!(),
    }
    assert_eq!(
        requests_to(&away.provider, &pages).len(),
        read_before,
        "resumed, not read again"
    );
    let place = away.dm.to_string();
    assert_eq!(
        kept_sent(&h, &place, "a1").message["content"]["text"],
        "hello Erin"
    );
}

#[tokio::test]
async fn a_restart_resumes_at_both_providers_from_the_kept_cursors() {
    let h = home().await;
    let away = away(&h).await;
    let (social, _told, mine) = talking(&h, &away).await;
    social.go_offline(&h.account()).await.unwrap();
    let place = away.dm.to_string();
    let kept = h
        .social
        .store()
        .social_place(&h.account(), &place)
        .unwrap()
        .unwrap();
    assert!(
        kept.cursors.live.is_some() && kept.cursors.sent.is_some(),
        "{:?}",
        kept.cursors
    );

    // Meanwhile Ada edits hers from her phone, and Erin writes again.
    away.phone_away
        .edit_dm(&away.dm, &held_id(&mine), &edit("hello Erin"))
        .await
        .unwrap();
    let again = away
        .erin_at_ada
        .send_dm(&away.dm, &to(&h.ada, "e2", "still there?", None))
        .await
        .unwrap();
    let pages = format!("/api/dms/{}/messages", away.dm);
    let read_before = requests_to(&away.provider, &pages).len();

    // A new engine over the same store, as after a restart.
    let restarted = h
        .social_with_keys(h.keys.clone())
        .with_live_options(options());
    let mut told = Told::new(&restarted);
    restarted.go_live(&h.account()).await.unwrap();
    let theirs = again.message().id.to_string();
    told.until("Erin's new message", |e| super::is_new(e, &theirs))
        .await;
    since_or_next(&mut told, 0, "the edit, replayed", |e| is_edit(e, &mine)).await;
    assert_eq!(
        requests_to(&away.provider, &pages).len(),
        read_before,
        "resumed, not read again"
    );
    assert_eq!(h.messages(&place).len(), 3);
}
