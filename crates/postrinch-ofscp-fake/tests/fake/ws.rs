//! The WebSocket (§7.1): authentication, subscriptions, resume, and the
//! socket faults.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::common::{Ended, Ws, user};
use ofscp_core::LocalId;
use ofscp_core::wire::{Content, MessageCreate};
use ofscp_core::ws::{ChannelMessageCreate, Command, Event, Subscribe};
use postrinch_ofscp_fake::FakeProvider;

/// A provider with alice, owner of a group with one channel.
async fn setup() -> (FakeProvider, crate::common::User, LocalId, LocalId) {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    let group = p.create_group(&alice.actor, "Team");
    let channel = p.create_channel(&group, "general");
    (p, alice, group, channel)
}

fn subscribe(channel: &LocalId, since: Option<&str>) -> Command {
    Command::Subscribe(Subscribe {
        channels: vec![channel.clone()],
        include: None,
        since: since.map(|s| BTreeMap::from([(channel.clone(), s.to_owned())])),
    })
}

fn create(group: &LocalId, channel: &LocalId, client_id: &str, text: &str) -> Command {
    Command::MessageCreate(ChannelMessageCreate {
        group_id: group.clone(),
        channel_id: channel.clone(),
        message: MessageCreate {
            client_message_id: Some(client_id.to_owned()),
            kind: None,
            content: Content {
                text: text.to_owned(),
                mime: "text/plain".to_owned(),
            },
            attachments: None,
            reference: None,
            tags: None,
            mentions: None,
            title: None,
            summary: None,
            cover: None,
            metadata: None,
        },
    })
}

fn created(event: Event) -> (Option<String>, String) {
    match event {
        Event::MessageCreated(m) => (m.cursor, m.message.content.unwrap().text),
        other => panic!("expected message.created, got {other:?}"),
    }
}

#[tokio::test]
async fn authenticate_then_subscribe_then_live_messages() {
    let (p, alice, group, channel) = setup().await;
    let mut ws = Ws::signed_in(&p, &alice).await;
    assert_eq!(p.signed_in_sockets(), 1);
    let id = ws.send(subscribe(&channel, None)).await;
    let ack = ws.event().await;
    assert_eq!(ack.correlation_id.as_deref(), Some(id.as_str()));
    assert!(matches!(ack.body, Event::Subscribed(ref s) if s.channels == vec![channel.clone()]));
    p.post_as(&alice.actor, &group, &channel, "hello");
    let (cursor, text) = created(ws.event().await.body);
    assert!(cursor.is_some());
    assert_eq!(text, "hello");
}

#[tokio::test]
async fn a_bad_signature_closes_with_4001() {
    let (p, alice, _, _) = setup().await;
    let mut ws = Ws::connect(&p).await;
    ws.challenge = "not-the-challenge".to_owned();
    ws.authenticate(&p, &alice).await;
    assert!(matches!(ws.event().await.body, Event::Error(_)));
    assert_eq!(ws.ended().await, Ended::Closed(4001));
}

#[tokio::test]
async fn a_command_before_authenticate_closes_with_4001() {
    let (p, _, _, channel) = setup().await;
    let mut ws = Ws::connect(&p).await;
    ws.send(subscribe(&channel, None)).await;
    assert_eq!(ws.ended().await, Ended::Closed(4001));
}

#[tokio::test]
async fn the_refuse_fault_fails_authentication() {
    let (p, alice, _, _) = setup().await;
    p.faults().refuse_ws_auth(true);
    let mut ws = Ws::connect(&p).await;
    ws.authenticate(&p, &alice).await;
    assert!(matches!(ws.event().await.body, Event::Error(_)));
    assert_eq!(ws.ended().await, Ended::Closed(4001));
}

#[tokio::test]
async fn an_unreadable_channel_is_refused_as_not_found() {
    let (p, alice, _, _) = setup().await;
    let mut ws = Ws::signed_in(&p, &alice).await;
    let nowhere = LocalId::new("chn_404").unwrap();
    ws.send(subscribe(&nowhere, None)).await;
    match ws.event().await.body {
        Event::Error(e) => {
            assert_eq!(e.status, 404);
            assert_eq!(e.details.unwrap()["channels"][0], "chn_404");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

#[tokio::test]
async fn resume_replays_after_the_cursor_or_reports_truncated() {
    let (p, alice, group, channel) = setup().await;
    p.post_as(&alice.actor, &group, &channel, "one");
    let mut ws = Ws::signed_in(&p, &alice).await;
    ws.send(subscribe(&channel, None)).await;
    ws.event().await;
    let (cursor, _) = created({
        p.post_as(&alice.actor, &group, &channel, "two");
        ws.event().await.body
    });
    let cursor = cursor.unwrap();
    drop(ws);
    p.post_as(&alice.actor, &group, &channel, "three");
    p.post_as(&alice.actor, &group, &channel, "four");

    let mut ws = Ws::signed_in(&p, &alice).await;
    ws.send(subscribe(&channel, Some(&cursor))).await;
    assert!(matches!(ws.event().await.body, Event::Subscribed(ref s) if s.truncated.is_none()));
    assert_eq!(created(ws.event().await.body).1, "three");
    assert_eq!(created(ws.event().await.body).1, "four");

    p.faults().set_replay_limit(Some(1));
    let mut ws = Ws::signed_in(&p, &alice).await;
    ws.send(subscribe(&channel, Some(&cursor))).await;
    match ws.event().await.body {
        Event::Subscribed(s) => assert_eq!(s.truncated, Some(vec![channel.clone()])),
        other => panic!("expected subscribed, got {other:?}"),
    }
    assert!(ws.quiet(Duration::from_millis(200)).await);
}

#[tokio::test]
async fn a_repeated_client_message_id_is_not_stored_twice() {
    let (p, alice, group, channel) = setup().await;
    let mut ws = Ws::signed_in(&p, &alice).await;
    ws.send(subscribe(&channel, None)).await;
    ws.event().await;
    let id = ws.send(create(&group, &channel, "c-1", "hi")).await;
    let first = ws.event().await;
    assert_eq!(first.correlation_id.as_deref(), Some(id.as_str()));
    assert!(created(first.body).0.is_some());
    ws.send(create(&group, &channel, "c-1", "hi")).await;
    // The duplicate is answered to its sender, without a cursor.
    assert_eq!(created(ws.event().await.body).0, None);
    assert_eq!(p.messages(&group, &channel).len(), 1);
}

#[tokio::test]
async fn the_duplicate_fault_sends_every_event_twice() {
    let (p, alice, group, channel) = setup().await;
    let mut ws = Ws::signed_in(&p, &alice).await;
    ws.send(subscribe(&channel, None)).await;
    ws.event().await;
    p.faults().duplicate_events(true);
    p.post_as(&alice.actor, &group, &channel, "twice");
    let a = created(ws.event().await.body);
    let b = created(ws.event().await.body);
    assert_eq!(a, b);
}

#[tokio::test]
async fn revoking_the_key_closes_its_sockets_with_4003() {
    let (p, alice, _, _) = setup().await;
    let mut ws = Ws::signed_in(&p, &alice).await;
    assert!(p.faults().revoke_key(&alice.key_id));
    assert_eq!(ws.ended().await, Ended::Closed(4003));
}

#[tokio::test]
async fn close_and_drop_faults() {
    let (p, alice, _, _) = setup().await;
    let mut ws = Ws::signed_in(&p, &alice).await;
    p.faults().close_sockets(4001);
    assert_eq!(ws.ended().await, Ended::Closed(4001));
    let mut ws = Ws::signed_in(&p, &alice).await;
    p.faults().drop_sockets();
    assert_eq!(ws.ended().await, Ended::Dropped);
    // The network failed for a moment only: sockets connect again.
    let mut ws = Ws::signed_in(&p, &alice).await;
    p.faults().drop_sockets();
    assert_eq!(ws.ended().await, Ended::Dropped);
}

#[tokio::test]
async fn a_foreign_member_authenticates_with_its_home_key() {
    let a = FakeProvider::start().await.unwrap();
    let b = FakeProvider::start().await.unwrap();
    let alice = user(&a, "alice");
    let bob = user(&b, "bob");
    let group = a.create_group(&alice.actor, "Team");
    let channel = a.create_channel(&group, "general");
    a.add_member(&group, &bob.actor, "member");
    let mut ws = Ws::signed_in(&a, &bob).await;
    ws.send(subscribe(&channel, None)).await;
    assert!(matches!(ws.event().await.body, Event::Subscribed(_)));
    let posted = a.post_as(&alice.actor, &group, &channel, "welcome");
    assert_eq!(posted.author, alice.actor);
    assert_eq!(created(ws.event().await.body).1, "welcome");
}

#[tokio::test]
async fn subscribed_comes_before_the_error_for_a_missing_channel() {
    let (p, alice, _, channel) = setup().await;
    let mut ws = Ws::signed_in(&p, &alice).await;
    let nowhere = LocalId::new("chn_404").unwrap();
    ws.send(Command::Subscribe(Subscribe {
        channels: vec![nowhere, channel.clone()],
        include: None,
        since: None,
    }))
    .await;
    assert!(
        matches!(ws.event().await.body, Event::Subscribed(ref s) if s.channels == vec![channel.clone()])
    );
    match ws.event().await.body {
        Event::Error(e) => assert_eq!(e.details.unwrap()["channels"][0], "chn_404"),
        other => panic!("expected error, got {other:?}"),
    }
}

#[tokio::test]
async fn dropping_the_provider_ends_its_sockets() {
    let (p, alice, _, _) = setup().await;
    let mut ws = Ws::signed_in(&p, &alice).await;
    drop(p);
    assert_eq!(ws.ended().await, Ended::Dropped);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_posts_reach_a_subscriber_in_cursor_order() {
    let (p, alice, group, channel) = setup().await;
    let mut ws = Ws::signed_in(&p, &alice).await;
    ws.send(subscribe(&channel, None)).await;
    assert!(matches!(ws.event().await.body, Event::Subscribed(_)));
    const WRITERS: usize = 8;
    const EACH: usize = 25;
    std::thread::scope(|s| {
        for w in 0..WRITERS {
            let (p, alice, group, channel) = (&p, &alice, &group, &channel);
            s.spawn(move || {
                for i in 0..EACH {
                    p.post_as(&alice.actor, group, channel, &format!("{w}.{i}"));
                }
            });
        }
    });
    let mut last = 0;
    for _ in 0..WRITERS * EACH {
        let (cursor, _) = created(ws.event().await.body);
        let seq: u64 = cursor.unwrap().rsplit_once('.').unwrap().1.parse().unwrap();
        assert!(seq > last, "cursor {seq} came after {last}");
        last = seq;
    }
}

#[tokio::test]
async fn the_challenge_expires_on_real_time_not_the_provider_clock() {
    let (p, alice, _, _) = setup().await;
    let mut ws = Ws::connect(&p).await;
    // Past the 30 s challenge on the provider's clock, inside the ±300 s
    // signing window.
    p.faults().set_clock_offset(120);
    ws.authenticate(&p, &alice).await;
    assert!(matches!(ws.event().await.body, Event::Authenticated(_)));
}
