//! The task that owns a signed-in socket: it sends commands, matches
//! replies, keeps the heartbeat, and forwards events.
//!
//! Every await in its loop is bounded, so it always comes back to read the
//! socket, answer pings and time out replies:
//!
//! - events go to the reader through a queue it never waits on; a reader
//!   more than [`WsOptions::event_buffer`] events behind ends the
//!   connection with [`CloseReason::Overrun`];
//! - a write, and the close, give up after the idle timeout.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ofscp_core::ws::{Command, DecodeError, Empty, Envelope, Event};
use ofscp_core::{Actor, Authority};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, MissedTickBehavior};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use super::connection::{Connection, Events, Op};
use super::{CloseReason, Incoming, RequestError, WsOptions};
use crate::ProviderClock;

/// How often timeouts and the heartbeat are looked at.
const TICK: Duration = Duration::from_millis(250);

/// Events in the channel itself; more wait in the driver's queue.
const CHANNEL: usize = 64;

/// Frame ids, unique within the connection (§7.1).
#[derive(Debug, Default)]
pub(super) struct Ids(u64);

impl Ids {
    pub fn next(&mut self) -> String {
        self.0 += 1;
        format!("c{}", self.0)
    }
}

/// Who the connection is for.
pub(super) struct Parts {
    pub authority: Authority,
    pub actor: Actor,
    pub clock: ProviderClock,
}

type Reply = oneshot::Sender<Result<Envelope<Event>, RequestError>>;

struct Pending {
    reply: Reply,
    deadline: Instant,
}

struct Driver<S> {
    socket: WebSocketStream<S>,
    clock: ProviderClock,
    authority: String,
    options: WsOptions,
    ids: Ids,
    pending: HashMap<String, Pending>,
    /// When this side last pinged unasked.
    pinged_at: Option<Instant>,
    /// When a frame last came in from the socket.
    last_heard: Instant,
    ops: mpsc::Receiver<Op>,
    events: mpsc::Sender<Incoming>,
    /// Events the channel had no room for, oldest first.
    queue: VecDeque<Incoming>,
    /// The reader dropped [`Events`]; events are no longer kept.
    reader_gone: bool,
    closed: watch::Sender<Option<CloseReason>>,
}

/// Start the driver for a signed-in `socket`.
pub(super) fn spawn<S>(
    socket: WebSocketStream<S>,
    parts: Parts,
    options: WsOptions,
    ids: Ids,
) -> (Connection, Events)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (ops_tx, ops) = mpsc::channel(CHANNEL);
    let (events, rx) = mpsc::channel(CHANNEL);
    let (closed, closed_rx) = watch::channel(None);
    let driver = Driver {
        socket,
        clock: parts.clock,
        authority: parts.authority.to_string(),
        options,
        ids,
        pending: HashMap::new(),
        pinged_at: None,
        last_heard: Instant::now(),
        ops,
        events,
        queue: VecDeque::new(),
        reader_gone: false,
        closed,
    };
    tokio::spawn(driver.run());
    let connection = Connection {
        ops: ops_tx,
        authority: parts.authority,
        actor: parts.actor,
        closed: closed_rx,
    };
    (connection, Events { rx })
}

/// Whether an event only answers a command and is not news in itself.
pub(super) fn is_ack(event: &Event) -> bool {
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

impl<S> Driver<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    async fn run(mut self) {
        let reason = self.serve().await;
        tracing::debug!(authority = %self.authority, %reason, "socket closed");
        for (_, pending) in self.pending.drain() {
            let _ = pending
                .reply
                .send(Err(RequestError::Closed(reason.clone())));
        }
        self.closed.send_replace(Some(reason.clone()));
        // The socket is done with; what is left is the reader's, in order.
        if self.reader_gone {
            return;
        }
        self.queue.push_back(Incoming::Closed(reason));
        while let Some(incoming) = self.queue.pop_front() {
            if self.events.send(incoming).await.is_err() {
                return;
            }
        }
    }

    async fn serve(&mut self) -> CloseReason {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            let backlog = !self.queue.is_empty();
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
                        let _ = self.bounded(|s| s.socket.close(Some(frame))).await;
                        return CloseReason::Client;
                    }
                    Some(Op::Send { command, reply }) => {
                        if let Err(reason) = self.send(*command, None, reply).await {
                            return reason;
                        }
                    }
                },
                // An owned permit, so the branch holds no borrow of `self`.
                permit = self.events.clone().reserve_owned(), if backlog => match permit {
                    Ok(permit) => {
                        if let Some(incoming) = self.queue.pop_front() {
                            permit.send(incoming);
                        }
                    }
                    Err(_) => self.lose_reader(),
                },
                _ = tick.tick() => {
                    if let Err(reason) = self.housekeep().await {
                        return reason;
                    }
                }
            }
        }
    }

    /// Run a socket operation, giving up after the idle timeout: a
    /// provider that stops reading must not stall the loop.
    async fn bounded<'a, F, Fut, T>(&'a mut self, op: F) -> Result<T, CloseReason>
    where
        F: FnOnce(&'a mut Self) -> Fut,
        Fut: Future<Output = Result<T, tokio_tungstenite::tungstenite::Error>> + 'a,
    {
        let limit = self.options.idle_timeout;
        match tokio::time::timeout(limit, op(self)).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => Err(CloseReason::Dropped(e.to_string())),
            Err(_) => Err(CloseReason::TimedOut),
        }
    }

    /// Send a frame. A failed or stuck write ends the connection.
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
        self.bounded(|s| s.socket.send(Message::Text(text.into())))
            .await?;
        Ok(id)
    }

    async fn on_message(&mut self, message: Message) -> Option<CloseReason> {
        match message {
            Message::Text(text) => self.on_text(text.as_str()).await.err(),
            Message::Close(frame) => {
                // tungstenite has queued the close's answer; send it.
                let _ = self.bounded(|s| s.socket.flush()).await;
                Some(CloseReason::from_frame(frame.as_ref()))
            }
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
                self.forward(frame)?;
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
            self.forward(frame)?;
        }
        Ok(())
    }

    /// Hand an event to the reader, without waiting for it.
    fn forward(&mut self, frame: Envelope<Event>) -> Result<(), CloseReason> {
        if self.reader_gone {
            return Ok(());
        }
        let incoming = Incoming::Event(Box::new(frame));
        if !self.queue.is_empty() {
            return self.enqueue(incoming);
        }
        match self.events.try_send(incoming) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(incoming)) => self.enqueue(incoming),
            Err(TrySendError::Closed(_)) => {
                self.lose_reader();
                Ok(())
            }
        }
    }

    fn enqueue(&mut self, incoming: Incoming) -> Result<(), CloseReason> {
        if self.queue.len() >= self.options.event_buffer {
            tracing::warn!(authority = %self.authority, "the event reader fell behind");
            return Err(CloseReason::Overrun);
        }
        self.queue.push_back(incoming);
        Ok(())
    }

    fn lose_reader(&mut self) {
        self.reader_gone = true;
        self.queue.clear();
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
        let pinged_since_heard = self.pinged_at.is_some_and(|at| at >= self.last_heard);
        if quiet >= self.options.ping_interval && !pinged_since_heard {
            self.send(Command::Ping(Empty {}), None, None).await?;
            self.pinged_at = Some(now);
        }
        Ok(())
    }
}
