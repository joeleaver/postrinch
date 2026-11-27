//! The task that owns a signed-in socket: it sends commands, matches
//! replies, keeps the heartbeat, and forwards events.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ofscp_core::ws::{Command, DecodeError, Empty, Envelope, Event};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, MissedTickBehavior};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use super::connect::Socket;
use super::connection::{Connection, Events, Op};
use super::{CloseReason, Incoming, RequestError, WsOptions};
use crate::ProviderClock;
use crate::http::Client;

/// How often timeouts and the heartbeat are looked at.
const TICK: Duration = Duration::from_millis(250);

/// Frame ids, unique within the connection (§7.1).
#[derive(Debug, Default)]
pub(super) struct Ids(u64);

impl Ids {
    pub fn next(&mut self) -> String {
        self.0 += 1;
        format!("c{}", self.0)
    }
}

type Reply = oneshot::Sender<Result<Envelope<Event>, RequestError>>;

struct Pending {
    reply: Reply,
    deadline: Instant,
}

struct Driver {
    socket: Socket,
    clock: ProviderClock,
    authority: String,
    options: WsOptions,
    ids: Ids,
    pending: HashMap<String, Pending>,
    /// The id of the last ping this side sent unasked, and when.
    own_ping: Option<(String, Instant)>,
    last_heard: Instant,
    ops: mpsc::Receiver<Op>,
    events: mpsc::Sender<Incoming>,
    closed: watch::Sender<Option<CloseReason>>,
}

/// Start the driver for a signed-in `socket`.
pub(super) fn spawn(
    socket: Socket,
    client: &Client,
    options: WsOptions,
    ids: Ids,
) -> (Connection, Events) {
    let (ops_tx, ops) = mpsc::channel(64);
    let (events, rx) = mpsc::channel(options.event_buffer.max(1));
    let (closed, closed_rx) = watch::channel(None);
    let authority = client.provider().endpoint().authority().clone();
    let driver = Driver {
        socket,
        clock: client.provider().clock().clone(),
        authority: authority.to_string(),
        options,
        ids,
        pending: HashMap::new(),
        own_ping: None,
        last_heard: Instant::now(),
        ops,
        events,
        closed,
    };
    tokio::spawn(driver.run());
    let connection = Connection {
        ops: ops_tx,
        authority,
        actor: client.identity().actor().clone(),
        closed: closed_rx,
    };
    (connection, Events { rx })
}

/// Whether an event only answers a command and is not news in itself.
fn is_ack(event: &Event) -> bool {
    matches!(
        event,
        Event::Authenticated(_)
            | Event::Subscribed(_)
            | Event::Unsubscribed(_)
            | Event::PresenceSubscribed(_)
            | Event::PresenceUnsubscribed(_)
            | Event::Pong(_)
            | Event::Error(_)
    )
}

impl Driver {
    async fn run(mut self) {
        let reason = self.serve().await;
        tracing::debug!(authority = %self.authority, %reason, "socket closed");
        for (_, pending) in self.pending.drain() {
            let _ = pending
                .reply
                .send(Err(RequestError::Closed(reason.clone())));
        }
        self.closed.send_replace(Some(reason.clone()));
        let _ = self.events.send(Incoming::Closed(reason)).await;
    }

    async fn serve(&mut self) -> CloseReason {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                message = self.socket.next() => match message {
                    None => return CloseReason::Dropped("the provider closed the connection".into()),
                    Some(Err(e)) => return CloseReason::Dropped(e.to_string()),
                    Some(Ok(message)) => {
                        self.last_heard = Instant::now();
                        if let Some(reason) = self.on_message(message).await {
                            return reason;
                        }
                    }
                },
                op = self.ops.recv() => match op {
                    None | Some(Op::Close) => {
                        let frame = CloseFrame { code: CloseCode::Normal, reason: "".into() };
                        // Best effort: the socket may already be gone.
                        let _ = self.socket.close(Some(frame)).await;
                        return CloseReason::Client;
                    }
                    Some(Op::Send { command, reply }) => {
                        if let Err(reason) = self.send(*command, None, reply).await {
                            return reason;
                        }
                    }
                },
                _ = tick.tick() => {
                    if let Err(reason) = self.housekeep().await {
                        return reason;
                    }
                }
            }
        }
    }

    /// Send a frame. A failed write ends the connection.
    async fn send(
        &mut self,
        command: Command,
        correlation_id: Option<String>,
        reply: Option<Reply>,
    ) -> Result<String, CloseReason> {
        let id = self.ids.next();
        let frame = Envelope {
            id: id.clone(),
            ts: Some(self.clock.now()),
            correlation_id,
            body: command,
        };
        let text = match frame.encode() {
            Ok(text) => text,
            Err(e) => {
                if let Some(reply) = reply {
                    let _ = reply.send(Err(RequestError::Encode(e.to_string())));
                }
                return Ok(id);
            }
        };
        if let Some(reply) = reply {
            let deadline = Instant::now() + self.options.reply_timeout;
            self.pending.insert(id.clone(), Pending { reply, deadline });
        }
        self.socket
            .send(Message::Text(text.into()))
            .await
            .map_err(|e| CloseReason::Dropped(e.to_string()))?;
        Ok(id)
    }

    async fn on_message(&mut self, message: Message) -> Option<CloseReason> {
        match message {
            Message::Text(text) => self.on_text(text.as_str()).await.err(),
            Message::Close(frame) => Some(CloseReason::from_frame(frame.as_ref())),
            // OFSCP is JSON text; control frames are tungstenite's.
            Message::Binary(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => None,
        }
    }

    async fn on_text(&mut self, text: &str) -> Result<(), CloseReason> {
        let frame = match Envelope::<Event>::decode(text) {
            Ok(frame) => frame,
            // A malformed frame is a local error, never answered (spec #27).
            Err(DecodeError::Data {
                id,
                kind,
                correlation_id,
                source,
            }) => {
                tracing::warn!(authority = %self.authority, %id, %kind, %source, "dropping a malformed event");
                if let Some(pending) = correlation_id.and_then(|c| self.pending.remove(&c)) {
                    let _ = pending
                        .reply
                        .send(Err(RequestError::Malformed(format!("{kind}: {source}"))));
                }
                return Ok(());
            }
            Err(e @ DecodeError::Envelope(_)) => {
                tracing::warn!(authority = %self.authority, error = %e, "dropping a frame that is not OFSCP");
                return Ok(());
            }
        };
        if let Event::Ping(_) = frame.body {
            self.send(Command::Pong(Empty {}), Some(frame.id), None)
                .await?;
            return Ok(());
        }
        let waiter = frame
            .correlation_id
            .as_ref()
            .and_then(|c| self.pending.remove(c));
        let Some(pending) = waiter else {
            // A pong no one waits for answers this side's own ping.
            if !matches!(frame.body, Event::Pong(_)) {
                self.forward(frame).await;
            }
            return Ok(());
        };
        let forward = !is_ack(&frame.body);
        let answer = match &frame.body {
            Event::Error(e) => Err(RequestError::Rejected(e.clone())),
            _ => Ok(frame.clone()),
        };
        let _ = pending.reply.send(answer);
        if forward {
            self.forward(frame).await;
        }
        Ok(())
    }

    async fn forward(&mut self, frame: Envelope<Event>) {
        // A reader that has gone away just misses events.
        let _ = self.events.send(Incoming::Event(Box::new(frame))).await;
    }

    /// Time out replies, ping a quiet provider, and give up on a silent one.
    async fn housekeep(&mut self) -> Result<(), CloseReason> {
        let now = Instant::now();
        let expired: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| p.deadline <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            if let Some(pending) = self.pending.remove(&id) {
                let _ = pending.reply.send(Err(RequestError::TimedOut));
            }
        }
        let quiet = now.duration_since(self.last_heard);
        if quiet >= self.options.idle_timeout {
            return Err(CloseReason::TimedOut);
        }
        let pinged_since_heard = self
            .own_ping
            .as_ref()
            .is_some_and(|(_, at)| *at >= self.last_heard);
        if quiet >= self.options.ping_interval && !pinged_since_heard {
            let id = self.send(Command::Ping(Empty {}), None, None).await?;
            self.own_ping = Some((id, now));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let mut ids = Ids::default();
        assert_eq!((ids.next(), ids.next()), ("c1".to_owned(), "c2".to_owned()));
    }

    #[test]
    fn acks_are_not_news() {
        assert!(is_ack(&Event::Pong(Empty {})));
        assert!(!is_ack(&Event::Ping(Empty {})));
    }
}
