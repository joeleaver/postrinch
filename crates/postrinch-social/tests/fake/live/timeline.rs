//! Messages, edits, deletes, reactions, mentions and private messages as
//! they arrive live; duplicates told once.

use std::time::Duration;

use postrinch_ofscp::ofscp_core::dm_id;
use postrinch_ofscp::ofscp_core::wire::{DmMessageCreate, MessageUpdate};
use postrinch_ofscp::ofscp_core::ws::{Command, ReactionAdd, Subscribe};
use postrinch_ofscp::ws::{self, WsOptions};
use postrinch_social::{LiveEvent, MessageChange};
use postrinch_store::{PlaceKind, ReactionSum};
use serde_json::json;

use super::{is_new, live, user};
use crate::common::{channel_uri, home};

#[tokio::test]
async fn a_message_posted_while_live_is_kept_unread_and_told() {
    let h = home().await;
    let (bob, group, general) = h.bob_with_group();
    let (_social, mut told) = live(&h, 1).await;

    let posted = h.provider.post_as(&bob, &group, &general, "hello");
    let id = posted.id.to_string();
    let event = told.until("the message", |e| is_new(e, &id)).await;
    let place = channel_uri(&h.provider, &group, &general);
    assert_eq!(
        event,
        LiveEvent::Message {
            account: h.account(),
            place: place.clone(),
            id: id.clone(),
            change: MessageChange::New,
            unread: true,
            mentions_me: false,
        }
    );
    let kept = h.messages(&place);
    assert_eq!(kept.len(), 1);
    assert!(kept[0].unread);
    let stored = h
        .social
        .store()
        .social_place(&h.account(), &place)
        .unwrap()
        .unwrap();
    assert!(stored.cursors.live.is_some(), "the cursor moved on");
    assert_eq!(stored.last_activity, Some(kept[0].created));
}

#[tokio::test]
async fn a_mention_is_told_with_its_notification() {
    let h = home().await;
    let (bob, group, general) = h.bob_with_group();
    let (social, mut told) = live(&h, 1).await;

    let text = format!("over to you @{}", h.ada);
    let posted = h.provider.post_as(&bob, &group, &general, &text);
    let id = posted.id.to_string();
    let event = told.until("the message", |e| is_new(e, &id)).await;
    assert!(matches!(
        event,
        LiveEvent::Message {
            mentions_me: true,
            ..
        }
    ));
    let event = told
        .until("the notification", |e| {
            matches!(e, LiveEvent::Notification { .. })
        })
        .await;
    let LiveEvent::Notification { id: note, .. } = event else {
        unreachable!()
    };
    let kept = social
        .store()
        .social_notifications(&h.account(), 10)
        .unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].id, note);
    assert!(!kept[0].read);
    assert_eq!(kept[0].notification["sourceMessageId"], id);
}

#[tokio::test]
async fn duplicate_events_are_kept_and_told_once() {
    let h = home().await;
    let (bob, group, general) = h.bob_with_group();
    let (_social, mut told) = live(&h, 1).await;
    h.provider.faults().duplicate_events(true);

    let first = h
        .provider
        .post_as(&bob, &group, &general, "one")
        .id
        .to_string();
    let second = h
        .provider
        .post_as(&bob, &group, &general, "two")
        .id
        .to_string();
    told.until("the second", |e| is_new(e, &second)).await;
    told.settle(Duration::from_millis(300)).await;
    assert_eq!(told.messages_told(&first), 1);
    assert_eq!(told.messages_told(&second), 1);
    assert_eq!(
        h.messages(&channel_uri(&h.provider, &group, &general))
            .len(),
        2
    );
}

#[tokio::test]
async fn edits_deletes_and_reactions_change_what_is_kept() {
    let h = home().await;
    let (bob, bob_client) = user(&h, "bob").await;
    let group = h.provider.create_group(&bob, "Bob's group");
    let general = h.provider.create_channel(&group, "general");
    h.provider.add_member(&group, &h.ada, "member");
    let place = channel_uri(&h.provider, &group, &general);
    let (_social, mut told) = live(&h, 1).await;
    h.provider.faults().duplicate_events(true);

    let posted = h.provider.post_as(&bob, &group, &general, "frist");
    let id = posted.id.to_string();
    told.until("the message", |e| is_new(e, &id)).await;

    let update: MessageUpdate =
        serde_json::from_value(json!({ "content": { "text": "first", "mime": "text/plain" } }))
            .unwrap();
    bob_client
        .edit_message(&group, &general, &posted.id, &update)
        .await
        .expect("the edit");
    told.until("the edit", |e| {
        matches!(e, LiveEvent::Message { change: MessageChange::Edited, id: told, .. } if *told == id)
    })
    .await;
    assert_eq!(h.messages(&place)[0].message["content"]["text"], "first");

    let (conn, _events) = ws::connect(&bob_client, WsOptions::default())
        .await
        .expect("Bob's socket");
    // The fake answers a reaction to the channel's subscribers.
    let subscribe: Subscribe = serde_json::from_value(json!({ "channels": [general] })).unwrap();
    conn.subscribe(subscribe).await.expect("Bob subscribes");
    let heart = ReactionAdd {
        group_id: group.clone(),
        channel_id: general.clone(),
        message_id: posted.id.clone(),
        key: "heart".to_owned(),
        unicode: Some("❤".to_owned()),
        image: None,
    };
    conn.request(Command::ReactionAdd(heart))
        .await
        .expect("the reaction");
    told.until("the reaction", |e| matches!(e, LiveEvent::Reactions { .. }))
        .await;
    let reactions = &h.messages(&place)[0].reactions;
    assert_eq!(
        reactions,
        &[ReactionSum {
            key: "heart".to_owned(),
            count: 1,
            mine: false,
        }]
    );
    conn.close().await;

    bob_client
        .delete_message(&group, &general, &posted.id)
        .await
        .expect("the delete");
    told.until("the delete", |e| {
        matches!(
            e,
            LiveEvent::Message {
                change: MessageChange::Deleted,
                ..
            }
        )
    })
    .await;
    let kept = &h.messages(&place)[0];
    assert!(kept.deleted);
    assert!(
        kept.message["content"].is_null(),
        "a tombstone keeps no text"
    );
    told.settle(Duration::from_millis(300)).await;
    let deletes = told
        .seen
        .iter()
        .filter(|e| {
            matches!(
                e,
                LiveEvent::Message {
                    change: MessageChange::Deleted,
                    ..
                }
            )
        })
        .count();
    assert_eq!(deletes, 1, "the repeated delete changes nothing");
}

#[tokio::test]
async fn a_private_message_from_someone_new_makes_the_conversation() {
    let h = home().await;
    let (_social, mut told) = live(&h, 1).await;
    let (carol, carol_client) = user(&h, "carol").await;
    let dm = dm_id(&carol, &h.ada);
    let message: DmMessageCreate = serde_json::from_value(json!({
        "clientMessageId": "c1",
        "to": h.ada.to_string(),
        "content": { "text": "hi Ada", "mime": "text/plain" },
    }))
    .unwrap();
    carol_client.send_dm(&dm, &message).await.expect("the DM");

    told.until(
        "the conversation",
        |e| matches!(e, LiveEvent::Place { place, .. } if place == dm.as_str()),
    )
    .await;
    told.until("the message", |e| {
        matches!(e, LiveEvent::Message { place, change: MessageChange::New, unread: true, .. } if place == dm.as_str())
    })
    .await;
    let place = h
        .social
        .store()
        .social_place(&h.account(), dm.as_str())
        .unwrap()
        .expect("the conversation is kept");
    assert_eq!(place.kind, PlaceKind::Conversation);
    assert_eq!(h.messages(dm.as_str()).len(), 1);

    // It is subscribed now: the next one arrives too.
    let again: DmMessageCreate = serde_json::from_value(json!({
        "clientMessageId": "c2",
        "to": h.ada.to_string(),
        "content": { "text": "still there?", "mime": "text/plain" },
    }))
    .unwrap();
    carol_client.send_dm(&dm, &again).await.expect("the DM");
    told.until("the second", |e| {
        matches!(e, LiveEvent::Message { place, change: MessageChange::New, .. } if place == dm.as_str())
    })
    .await;
    assert_eq!(h.messages(dm.as_str()).len(), 2);
}
