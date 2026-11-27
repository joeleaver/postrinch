//! The WebSocket of §7.1: challenge authentication, then commands, with
//! fan-out through the [`hub`].

mod commands;
pub(crate) mod hub;
mod subscribe;
pub(crate) mod typing;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message as Frame, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt as _, StreamExt as _};
use jiff::{SignedDuration, Timestamp};
use ofscp_core::Actor;
use ofscp_core::wire::{Availability, Presence};
use ofscp_core::ws::{
    AuthChallenge, CLOSE_AUTH_FAILED, CLOSE_REVOKED, Envelope, Event, PresenceChanged, WsError,
};
use tokio::time::{Instant, interval, sleep_until};

use crate::inner::Inner;
use crate::problem::Problem;
use crate::verify::keys::Lookup;
use hub::{ConnId, Outgoing, To};

pub(crate) use commands::set_presence;

/// How long a client has to authenticate (§7.1, RECOMMENDED 10 s).
const AUTH_WINDOW: Duration = Duration::from_secs(10);
/// How long a challenge nonce stays valid.
const CHALLENGE_TTL: SignedDuration = SignedDuration::from_secs(30);
/// The provider's heartbeat (§7.1: ping every 30 s, close after 60 s idle).
const PING_EVERY: Duration = Duration::from_secs(30);
const IDLE_LIMIT: Duration = Duration::from_secs(60);
/// A remote actor's keys are re-fetched this long before their cache
/// entry expires (§7.1 *Re-resolve before expiry*).
const REFRESH_AHEAD: SignedDuration = SignedDuration::from_secs(10);

pub(crate) async fn upgrade(State(inner): State<Arc<Inner>>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| run(inner, socket))
}

/// What the connection loop does next.
pub(crate) enum Flow {
    Continue,
    Close(u16),
}

/// One connection's state.
pub(crate) struct Conn {
    pub(crate) inner: Arc<Inner>,
    pub(crate) id: ConnId,
    sink: SplitSink<WebSocket, Frame>,
    next_event: u64,
    /// The challenge nonce, and when it expires on the real clock (a
    /// skewed provider clock does not shorten the client's 30 s).
    pub(crate) challenge: Option<(String, Instant)>,
    pub(crate) actor: Option<Actor>,
    pub(crate) key_id: Option<String>,
    /// For a remote actor: until when its key is confirmed.
    pub(crate) key_until: Option<Timestamp>,
}

impl Conn {
    /// Queue an event behind everything already queued for this
    /// connection (acks, replays, fan-out), so replies keep their order.
    pub(crate) fn emit(&self, body: Event, correlation_id: Option<String>) {
        self.inner.hub.send(
            self.id,
            Outgoing::Event {
                body: Box::new(body),
                correlation_id,
            },
        );
    }

    /// Write one event to the socket with a fresh id and the provider's
    /// time.
    async fn write(&mut self, body: Event, correlation_id: Option<String>) -> bool {
        self.next_event += 1;
        let envelope = Envelope {
            id: format!("evt_{}", self.next_event),
            ts: Some(self.inner.now()),
            correlation_id,
            body,
        };
        let text = envelope.encode().expect("events encode");
        self.sink.send(Frame::Text(text.into())).await.is_ok()
    }

    pub(crate) fn error(&self, problem: Problem, correlation_id: Option<String>) {
        let mut details = problem.details.clone();
        if let Some(secs) = problem.retry_after {
            let d = details.get_or_insert_with(|| serde_json::json!({}));
            d["retryAfter"] = secs.into();
        }
        let body = Event::Error(WsError {
            code: problem.code.clone(),
            message: problem.detail.clone(),
            status: problem.status(),
            details,
        });
        self.emit(body, correlation_id);
    }

    async fn close(&mut self, code: u16) {
        let frame = CloseFrame {
            code,
            reason: "".into(),
        };
        let _ = self.sink.send(Frame::Close(Some(frame))).await;
        let _ = self.sink.close().await;
    }
}

async fn run(inner: Arc<Inner>, socket: WebSocket) {
    let (id, mut outgoing) = inner.hub.register();
    let (sink, mut stream) = socket.split();
    let nonce = crate::random::token(16);
    let expires_at = inner.now() + CHALLENGE_TTL;
    let deadline = Instant::now() + Duration::from_secs(CHALLENGE_TTL.as_secs().unsigned_abs());
    let mut conn = Conn {
        inner: inner.clone(),
        id,
        sink,
        next_event: 0,
        challenge: Some((nonce.clone(), deadline)),
        actor: None,
        key_id: None,
        key_until: None,
    };
    let challenge = Event::AuthChallenge(AuthChallenge { nonce, expires_at });
    let mut open = conn.write(challenge, None).await;
    // Set once a close is queued: no further commands are read.
    let mut closing = false;
    let auth_deadline = Instant::now() + AUTH_WINDOW;
    let mut last_traffic = Instant::now();
    let mut ping = interval(PING_EVERY);
    ping.tick().await;
    let mut checks = interval(Duration::from_secs(1));

    while open {
        tokio::select! {
            frame = stream.next(), if !closing => {
                last_traffic = Instant::now();
                match frame {
                    Some(Ok(Frame::Text(text))) => match commands::handle(&mut conn, text.as_str()).await {
                        Flow::Continue => {}
                        Flow::Close(code) => {
                            // Behind the replies already queued, such as the error.
                            inner.hub.send(id, Outgoing::Close(code));
                            closing = true;
                        }
                    },
                    Some(Ok(Frame::Close(_))) | None | Some(Err(_)) => open = false,
                    Some(Ok(_)) => {}
                }
            }
            out = outgoing.recv() => match out {
                Some(Outgoing::Event { body, correlation_id }) => open = conn.write(*body, correlation_id).await,
                Some(Outgoing::Close(code)) => { conn.close(code).await; open = false; }
                // No close frame: the socket just goes, as when the network fails.
                Some(Outgoing::Drop) | None => open = false,
            },
            () = sleep_until(auth_deadline), if conn.actor.is_none() => {
                conn.close(CLOSE_AUTH_FAILED).await;
                open = false;
            }
            _ = ping.tick() => {
                if last_traffic.elapsed() >= IDLE_LIMIT {
                    conn.close(1000).await;
                    open = false;
                } else {
                    open = conn.write(Event::Ping(Default::default()), None).await;
                }
            }
            _ = checks.tick() => {
                if let Some(code) = recheck_key(&mut conn).await {
                    conn.close(code).await;
                    open = false;
                }
            }
        }
    }
    finish(&inner, id).await;
}

/// §7.1 *Close codes*: keep a remote actor's key confirmed, or say why
/// the connection must close.
async fn recheck_key(conn: &mut Conn) -> Option<u16> {
    let (Some(actor), Some(key_id), Some(until)) = (&conn.actor, &conn.key_id, conn.key_until)
    else {
        return None;
    };
    let now = conn.inner.clock.now();
    if now < until - REFRESH_AHEAD {
        return None;
    }
    match conn.inner.resolver.key(actor, key_id, now, true).await {
        Lookup::Found { until, .. } if until > now => {
            conn.key_until = Some(until);
            None
        }
        Lookup::Gone => Some(CLOSE_REVOKED),
        _ if now >= until => Some(CLOSE_AUTH_FAILED),
        _ => None,
    }
}

/// After the socket ends: stop its typing, and mark its actor offline when
/// this was their last connection (§7.5).
async fn finish(inner: &Arc<Inner>, id: ConnId) {
    inner.typing.disconnected(inner, id);
    let Some(actor) = inner.hub.unregister(id) else {
        return;
    };
    if inner.hub.online(&actor) {
        return;
    }
    let now = inner.now();
    let local = {
        let mut state = inner.state.lock().unwrap();
        state
            .local_user_mut(&inner.authority, &actor)
            .map(|u| u.last_seen = Some(now))
    };
    if local.is_some() {
        publish_presence(inner, &actor);
    }
}

/// A local user's presence as everyone may see it (the fake treats every
/// user's presence visibility as `public`).
pub(crate) fn presence_of(inner: &Inner, actor: &Actor) -> Option<Presence> {
    let state = inner.state.lock().unwrap();
    let user = state.local_user(&inner.authority, actor)?;
    Some(if inner.hub.online(actor) {
        Presence {
            availability: user.availability.clone(),
            status: user.status.clone(),
            last_seen: None,
            metadata: Vec::new(),
        }
    } else {
        Presence {
            availability: Availability::Offline,
            status: None,
            last_seen: user.last_seen,
            metadata: Vec::new(),
        }
    })
}

/// Fan a local user's presence out to the connections following it.
pub(crate) fn publish_presence(inner: &Inner, actor: &Actor) {
    if let Some(presence) = presence_of(inner, actor) {
        let event = Event::PresenceUpdate(PresenceChanged {
            user: actor.clone(),
            presence,
        });
        inner.hub.publish(To::PresenceOf(actor), &event, None, 1);
    }
}
