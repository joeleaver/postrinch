//! The driver against a scripted provider over an in-memory socket.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use jiff::Timestamp;
use ofscp_core::wire::ErrorCode;
use ofscp_core::ws::{Command, Empty, Envelope, Event, Subscribe, Subscribed, WsError};
use ofscp_core::{Actor, Authority};
use tokio::io::DuplexStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role};

use super::driver::{Ids, Parts, is_ack, spawn};
use super::{CloseReason, Connection, Events, Incoming, RequestError, WsOptions};
use crate::ProviderClock;

type Peer = WebSocketStream<DuplexStream>;

async fn open(options: WsOptions) -> (Connection, Events, Peer) {
    let (ours, theirs) = tokio::io::duplex(1 << 20);
    let ours = WebSocketStream::from_raw_socket(ours, Role::Client, None).await;
    let peer = WebSocketStream::from_raw_socket(theirs, Role::Server, None).await;
    let authority = Authority::canonicalize("a.example").unwrap();
    let parts = Parts {
        actor: Actor::new("ann", authority.clone()).unwrap(),
        authority,
        clock: ProviderClock::new(),
    };
    let (connection, events) = spawn(ours, parts, options, Ids::default());
    (connection, events, peer)
}

/// The next command the client sent.
async fn command(peer: &mut Peer) -> Envelope<Command> {
    loop {
        if let Message::Text(text) = peer.next().await.unwrap().unwrap() {
            return Envelope::decode(text.as_str()).unwrap();
        }
    }
}

async fn say(peer: &mut Peer, id: &str, correlation_id: Option<&str>, body: Event) {
    let frame = Envelope {
        id: id.to_owned(),
        ts: Some(Timestamp::now()),
        correlation_id: correlation_id.map(str::to_owned),
        body,
    };
    peer.send(Message::Text(frame.encode().unwrap().into()))
        .await
        .unwrap();
}

/// Send `n` custom events, stopping when the client has gone.
async fn flood(peer: &mut Peer, n: usize) {
    for n in 0..n {
        let frame = Envelope {
            id: format!("r{n}"),
            ts: Some(Timestamp::now()),
            correlation_id: None,
            body: custom(),
        };
        if peer
            .send(Message::Text(frame.encode().unwrap().into()))
            .await
            .is_err()
        {
            return;
        }
    }
}

fn custom() -> Event {
    Event::Other {
        kind: "x.custom".into(),
        data: serde_json::json!({}),
    }
}

fn subscribe() -> Command {
    Command::Subscribe(Subscribe {
        channels: vec![],
        include: None,
        since: None,
    })
}

fn subscribed() -> Event {
    Event::Subscribed(Subscribed {
        channels: vec![],
        truncated: None,
    })
}

async fn next_event(events: &mut Events) -> Event {
    match events.recv().await.unwrap() {
        Incoming::Event(frame) => frame.body,
        Incoming::Closed(reason) => panic!("closed: {reason}"),
    }
}

#[test]
fn acks_are_not_news() {
    assert!(is_ack(&Event::Pong(Empty {})));
    assert!(!is_ack(&custom()));
}

#[tokio::test]
async fn a_reply_is_matched_and_a_timeline_reply_is_forwarded_too() {
    let (connection, mut events, mut peer) = open(WsOptions::default()).await;
    let ask = tokio::spawn({
        let connection = connection.clone();
        async move { connection.request(Command::Ping(Empty {})).await }
    });
    let sent = command(&mut peer).await;
    assert_eq!(sent.id, "c1");
    // An unrelated event first, then the answer.
    say(&mut peer, "s1", None, custom()).await;
    say(&mut peer, "s2", Some(&sent.id), custom()).await;
    let reply = ask.await.unwrap().unwrap();
    assert_eq!(reply.id, "s2");
    assert_eq!(next_event(&mut events).await.kind(), "x.custom");
    assert_eq!(next_event(&mut events).await.kind(), "x.custom");

    // Dropping the last `Connection` closes the socket; keep one.
    let held = connection.clone();
    let ask = tokio::spawn(async move {
        held.subscribe(Subscribe {
            channels: vec![],
            include: None,
            since: None,
        })
        .await
    });
    let sent = command(&mut peer).await;
    say(&mut peer, "s3", Some(&sent.id), subscribed()).await;
    ask.await.unwrap().unwrap();
    // An ack goes to the waiter only.
    say(&mut peer, "s4", None, custom()).await;
    assert_eq!(next_event(&mut events).await.kind(), "x.custom");
}

#[tokio::test]
async fn an_error_rejects_the_request() {
    let (connection, _events, mut peer) = open(WsOptions::default()).await;
    let ask = tokio::spawn(async move { connection.request(subscribe()).await });
    let sent = command(&mut peer).await;
    let error = WsError {
        code: ErrorCode::Forbidden,
        message: "no".into(),
        status: 403,
        details: None,
    };
    say(&mut peer, "s1", Some(&sent.id), Event::Error(error.clone())).await;
    assert!(matches!(ask.await.unwrap(), Err(RequestError::Rejected(e)) if *e == error));
}

#[tokio::test]
async fn a_malformed_reply_fails_the_request_and_is_not_answered() {
    let (connection, _events, mut peer) = open(WsOptions::default()).await;
    let ask = tokio::spawn(async move { connection.request(subscribe()).await });
    let sent = command(&mut peer).await;
    let bad = serde_json::json!({
        "type": "subscribed", "id": "s1", "correlationId": sent.id,
        "data": { "channels": 5 },
    });
    peer.send(Message::Text(bad.to_string().into()))
        .await
        .unwrap();
    assert!(matches!(
        ask.await.unwrap(),
        Err(RequestError::Malformed(_))
    ));
}

#[tokio::test]
async fn the_providers_ping_is_answered_and_not_forwarded() {
    let (_connection, mut events, mut peer) = open(WsOptions::default()).await;
    say(&mut peer, "p1", None, Event::Ping(Empty {})).await;
    let pong = command(&mut peer).await;
    assert!(matches!(pong.body, Command::Pong(_)));
    assert_eq!(pong.correlation_id.as_deref(), Some("p1"));
    say(&mut peer, "s1", None, custom()).await;
    assert_eq!(next_event(&mut events).await.kind(), "x.custom");
}

#[tokio::test(start_paused = true)]
async fn an_unanswered_request_times_out() {
    let options = WsOptions {
        reply_timeout: Duration::from_secs(5),
        ..WsOptions::default()
    };
    let (connection, _events, _peer) = open(options).await;
    let result = connection.request(subscribe()).await;
    assert!(matches!(result, Err(RequestError::TimedOut)));
}

#[tokio::test(start_paused = true)]
async fn a_quiet_provider_is_pinged_and_a_silent_one_given_up() {
    let (connection, mut events, mut peer) = open(WsOptions::default()).await;
    let ping = command(&mut peer).await;
    assert!(matches!(ping.body, Command::Ping(_)));
    assert_eq!(
        events.recv().await,
        Some(Incoming::Closed(CloseReason::TimedOut))
    );
    assert_eq!(connection.close_reason(), Some(CloseReason::TimedOut));
}

#[tokio::test]
async fn a_slow_reader_does_not_hold_up_replies_or_pings() {
    let (connection, mut events, mut peer) = open(WsOptions::default()).await;
    let ask = tokio::spawn(async move { connection.request(subscribe()).await });
    let sent = command(&mut peer).await;
    // A long replay, more than the channel holds, before the ack (§7.1).
    for n in 0..500 {
        say(&mut peer, &format!("r{n}"), None, custom()).await;
    }
    say(&mut peer, "p1", None, Event::Ping(Empty {})).await;
    say(&mut peer, "s1", Some(&sent.id), subscribed()).await;
    assert!(matches!(command(&mut peer).await.body, Command::Pong(_)));
    ask.await.unwrap().unwrap();
    for n in 0..500 {
        match events.recv().await.unwrap() {
            Incoming::Event(frame) => assert_eq!(frame.id, format!("r{n}")),
            Incoming::Closed(reason) => panic!("closed: {reason}"),
        }
    }
}

#[tokio::test]
async fn a_reader_too_far_behind_ends_the_connection() {
    let options = WsOptions {
        event_buffer: 10,
        ..WsOptions::default()
    };
    let (_connection, mut events, mut peer) = open(options).await;
    flood(&mut peer, 200).await;
    let mut seen = 0;
    loop {
        match events.recv().await.unwrap() {
            Incoming::Event(frame) => {
                assert_eq!(frame.id, format!("r{seen}"));
                seen += 1;
            }
            Incoming::Closed(reason) => {
                assert_eq!(reason, CloseReason::Overrun);
                break;
            }
        }
    }
    assert!(seen < 200, "only what was held is delivered");
    assert_eq!(events.recv().await, None);
}

#[tokio::test]
async fn a_dropped_reader_does_not_hold_up_replies() {
    let (connection, events, mut peer) = open(WsOptions::default()).await;
    drop(events);
    let ask = tokio::spawn(async move { connection.request(subscribe()).await });
    let sent = command(&mut peer).await;
    for n in 0..500 {
        say(&mut peer, &format!("r{n}"), None, custom()).await;
    }
    say(&mut peer, "s1", Some(&sent.id), subscribed()).await;
    ask.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_revoked_key_fails_what_waits_and_ends_the_events() {
    let (connection, mut events, mut peer) = open(WsOptions::default()).await;
    let ask = tokio::spawn({
        let connection = connection.clone();
        async move { connection.request(subscribe()).await }
    });
    command(&mut peer).await;
    peer.close(Some(CloseFrame {
        code: CloseCode::from(4003),
        reason: "revoked".into(),
    }))
    .await
    .unwrap();
    assert!(matches!(
        ask.await.unwrap(),
        Err(RequestError::Closed(CloseReason::Revoked))
    ));
    assert_eq!(
        events.recv().await,
        Some(Incoming::Closed(CloseReason::Revoked))
    );
    assert_eq!(connection.close_reason(), Some(CloseReason::Revoked));
    assert!(matches!(
        connection.ping().await,
        Err(RequestError::Closed(CloseReason::Revoked))
    ));
}

#[tokio::test]
async fn closing_sends_a_normal_close() {
    let (connection, mut events, mut peer) = open(WsOptions::default()).await;
    connection.close().await;
    loop {
        match peer.next().await {
            Some(Ok(Message::Close(frame))) => {
                assert_eq!(frame.unwrap().code, CloseCode::Normal);
                break;
            }
            Some(Ok(_)) => {}
            other => panic!("no close frame: {other:?}"),
        }
    }
    assert_eq!(
        events.recv().await,
        Some(Incoming::Closed(CloseReason::Client))
    );
}

mod end;
