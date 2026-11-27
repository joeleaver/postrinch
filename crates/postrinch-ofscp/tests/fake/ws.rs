//! The WebSocket: sign-in, events, resume, and the ways it ends.

use std::collections::BTreeMap;
use std::time::Duration;

use ofscp_core::LocalId;
use ofscp_core::ws::{ChannelMessage, Event, Subscribe};
use postrinch_ofscp::ws::{
    self, CloseReason, ConnectError, Connection, Events, Incoming, WsOptions,
};

use crate::common::{World, world};

async fn subscribed(w: &World, since: Option<&str>) -> (Connection, Events, Vec<LocalId>) {
    let (connection, events) = ws::connect(&w.alice, WsOptions::default())
        .await
        .expect("the WebSocket sign-in");
    let ack = connection
        .subscribe(Subscribe {
            channels: vec![w.channel.clone()],
            include: None,
            since: since.map(|s| BTreeMap::from([(w.channel.clone(), s.to_owned())])),
        })
        .await
        .expect("subscribe");
    (connection, events, ack.truncated.unwrap_or_default())
}

async fn next(events: &mut Events) -> Incoming {
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("something within 5 s")
        .expect("not past the end")
}

async fn created(events: &mut Events) -> ChannelMessage {
    loop {
        match next(events).await {
            Incoming::Event(frame) => {
                if let Event::MessageCreated(created) = frame.body {
                    return created;
                }
            }
            Incoming::Closed(reason) => panic!("closed: {reason}"),
        }
    }
}

async fn closed(events: &mut Events) -> CloseReason {
    loop {
        if let Incoming::Closed(reason) = next(events).await {
            return reason;
        }
    }
}

#[tokio::test]
async fn a_post_reaches_a_subscriber() {
    let w = world().await;
    let (connection, mut events, truncated) = subscribed(&w, None).await;
    assert!(truncated.is_empty());
    assert_eq!(w.provider.signed_in_sockets(), 1);
    connection.ping().await.expect("pong");
    let posted = w
        .provider
        .post_as(w.alice.identity().actor(), &w.group, &w.channel, "hello");
    let event = created(&mut events).await;
    assert_eq!(event.message.id, posted.id);
    assert_eq!(event.channel_id, w.channel);
    connection.close().await;
    assert_eq!(closed(&mut events).await, CloseReason::Client);
}

#[tokio::test]
async fn resuming_replays_the_gap_or_says_it_is_truncated() {
    let w = world().await;
    let actor = w.alice.identity().actor().clone();
    let (connection, mut events, _) = subscribed(&w, None).await;
    w.provider.post_as(&actor, &w.group, &w.channel, "one");
    let cursor = created(&mut events).await.cursor.expect("a cursor");
    connection.close().await;
    let two = w.provider.post_as(&actor, &w.group, &w.channel, "two");
    let three = w.provider.post_as(&actor, &w.group, &w.channel, "three");

    let (connection, mut events, truncated) = subscribed(&w, Some(&cursor)).await;
    assert!(truncated.is_empty());
    assert_eq!(created(&mut events).await.message.id, two.id);
    assert_eq!(created(&mut events).await.message.id, three.id);
    connection.close().await;

    w.provider.faults().set_replay_limit(Some(1));
    let (_connection, _events, truncated) = subscribed(&w, Some(&cursor)).await;
    assert_eq!(
        truncated,
        std::slice::from_ref(&w.channel),
        "backfill over REST"
    );
}

#[tokio::test]
async fn duplicate_events_are_passed_on_as_sent() {
    let w = world().await;
    w.provider.faults().duplicate_events(true);
    let (_connection, mut events, _) = subscribed(&w, None).await;
    let posted = w
        .provider
        .post_as(w.alice.identity().actor(), &w.group, &w.channel, "twice");
    let first = created(&mut events).await;
    let second = created(&mut events).await;
    assert_eq!(first.message.id, posted.id);
    assert_eq!(second.message.id, posted.id);
    assert_eq!(first.cursor, second.cursor, "the store dedupes by cursor");
}

#[tokio::test]
async fn a_revoked_key_ends_the_socket_for_good() {
    let w = world().await;
    let (connection, mut events, _) = subscribed(&w, None).await;
    assert!(w.provider.faults().revoke_key(w.alice.identity().key_id()));
    let reason = closed(&mut events).await;
    assert_eq!(reason, CloseReason::Revoked);
    assert!(!reason.may_reconnect());
    assert_eq!(connection.close_reason(), Some(CloseReason::Revoked));
    assert!(events.recv().await.is_none());
}

#[tokio::test]
async fn a_4001_may_be_reconnected() {
    let w = world().await;
    let (_connection, mut events, _) = subscribed(&w, None).await;
    w.provider.faults().close_sockets_auth_failed();
    let reason = closed(&mut events).await;
    assert_eq!(reason, CloseReason::AuthFailed);
    assert!(reason.may_reconnect());
    subscribed(&w, None).await;
}

#[tokio::test]
async fn a_dropped_socket_is_noticed() {
    let w = world().await;
    let (connection, mut events, _) = subscribed(&w, None).await;
    w.provider.faults().drop_sockets();
    assert!(matches!(closed(&mut events).await, CloseReason::Dropped(_)));
    assert!(connection.ping().await.is_err());
}

#[tokio::test]
async fn a_refused_sign_in_says_how() {
    let w = world().await;
    w.provider.faults().refuse_ws_auth(true);
    let Err(ConnectError::Refused { error, close, .. }) =
        ws::connect(&w.alice, WsOptions::default()).await
    else {
        panic!("the sign-in is refused");
    };
    assert!(error.is_some(), "the error event is kept");
    assert_eq!(close, CloseReason::AuthFailed);
}
