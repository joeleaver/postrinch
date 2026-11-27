//! The two halves a caller holds: [`Connection`] to send on, [`Events`]
//! to read from.

use ofscp_core::ws::{
    Command, Empty, Envelope, Event, Subscribe, Subscribed, Unsubscribe, Unsubscribed,
};
use ofscp_core::{Actor, Authority, LocalId};
use tokio::sync::{mpsc, oneshot, watch};

use super::{CloseReason, Incoming, RequestError};

/// What the driver is asked to do.
pub(super) enum Op {
    /// Send a command; with `reply`, wait for the frame that answers it.
    Send {
        command: Box<Command>,
        reply: Option<oneshot::Sender<Result<Envelope<Event>, RequestError>>>,
    },
    /// Close the connection.
    Close,
}

/// Sends on a signed-in connection. Cheap to clone; the connection closes
/// when the last clone is dropped, or on [`Connection::close`].
#[derive(Debug, Clone)]
pub struct Connection {
    pub(super) ops: mpsc::Sender<Op>,
    pub(super) authority: Authority,
    pub(super) actor: Actor,
    pub(super) closed: watch::Receiver<Option<CloseReason>>,
}

/// Reads a connection's events, in the order the provider sent them, up
/// to [`Incoming::Closed`].
#[derive(Debug)]
pub struct Events {
    pub(super) rx: mpsc::Receiver<Incoming>,
}

impl Events {
    /// The next event; `None` after [`Incoming::Closed`].
    pub async fn recv(&mut self) -> Option<Incoming> {
        self.rx.recv().await
    }
}

impl Connection {
    /// The provider.
    pub fn authority(&self) -> &Authority {
        &self.authority
    }

    /// Who is signed in.
    pub fn actor(&self) -> &Actor {
        &self.actor
    }

    /// How the connection ended, once it has.
    pub fn close_reason(&self) -> Option<CloseReason> {
        self.closed.borrow().clone()
    }

    fn gone(&self) -> RequestError {
        RequestError::Closed(
            self.close_reason()
                .unwrap_or_else(|| CloseReason::Dropped("the connection's task ended".into())),
        )
    }

    /// Send `command` and wait for the frame whose `correlationId` is its
    /// id: an acknowledgement, the event it caused (such as the
    /// `message.created` for a `message.create`), or an `error`, which
    /// comes back as [`RequestError::Rejected`]. An event that is also a
    /// timeline change is delivered to [`Events`] too.
    pub async fn request(&self, command: Command) -> Result<Envelope<Event>, RequestError> {
        let (tx, rx) = oneshot::channel();
        self.ops
            .send(Op::Send {
                command: Box::new(command),
                reply: Some(tx),
            })
            .await
            .map_err(|_| self.gone())?;
        rx.await.map_err(|_| self.gone())?
    }

    /// Send `command` without waiting for anything, for commands no reply
    /// answers (`typing.start`, `typing.stop`, `presence.set`).
    pub async fn send(&self, command: Command) -> Result<(), RequestError> {
        self.ops
            .send(Op::Send {
                command: Box::new(command),
                reply: None,
            })
            .await
            .map_err(|_| self.gone())
    }

    /// Subscribe to channels, from their last cursors when resuming (§7.1).
    /// The answer's `truncated` lists the channels whose gap the provider
    /// would not replay: backfill those over REST (§7.2).
    pub async fn subscribe(&self, subscribe: Subscribe) -> Result<Subscribed, RequestError> {
        match self.request(Command::Subscribe(subscribe)).await?.body {
            Event::Subscribed(ack) => Ok(ack),
            other => Err(RequestError::Unexpected(other.kind().to_owned())),
        }
    }

    /// Stop receiving channels' events.
    pub async fn unsubscribe(&self, channels: Vec<LocalId>) -> Result<Unsubscribed, RequestError> {
        match self
            .request(Command::Unsubscribe(Unsubscribe { channels }))
            .await?
            .body
        {
            Event::Unsubscribed(ack) => Ok(ack),
            other => Err(RequestError::Unexpected(other.kind().to_owned())),
        }
    }

    /// Ping the provider and wait for its `pong` (§7.1).
    pub async fn ping(&self) -> Result<(), RequestError> {
        match self.request(Command::Ping(Empty {})).await?.body {
            Event::Pong(_) => Ok(()),
            other => Err(RequestError::Unexpected(other.kind().to_owned())),
        }
    }

    /// Close the connection. [`Events`] ends with
    /// [`CloseReason::Client`].
    pub async fn close(&self) {
        // The driver may have ended already; then it is closed anyway.
        let _ = self.ops.send(Op::Close).await;
    }
}
