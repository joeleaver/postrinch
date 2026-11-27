//! The live WebSocket connections of one provider, what each is signed in
//! as and subscribed to, and fan-out to them.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ofscp_core::Actor;
use ofscp_core::ws::Event;
use tokio::sync::mpsc;

pub(crate) type ConnId = u64;

/// What a connection's writer is told to do.
#[derive(Debug)]
pub(crate) enum Outgoing {
    Event {
        body: Box<Event>,
        correlation_id: Option<String>,
    },
    /// Close with this code (a close frame, then the socket).
    Close(u16),
    /// Drop the socket with no close frame.
    Drop,
}

impl Outgoing {
    pub(crate) fn event(body: Event, correlation_id: Option<String>) -> Self {
        Outgoing::Event {
            body: Box::new(body),
            correlation_id,
        }
    }
}

#[derive(Debug)]
struct Conn {
    tx: mpsc::UnboundedSender<Outgoing>,
    actor: Option<Actor>,
    key_id: Option<String>,
    /// Channel ids and dmIds this connection subscribed to.
    scopes: HashSet<String>,
    /// Users whose presence it follows (§7.5).
    presence: HashSet<Actor>,
}

/// Who an event goes to.
pub(crate) enum To<'a> {
    /// The subscribers of a channel or dmId.
    Scope(&'a str),
    /// Every authenticated connection of these actors, subscribed or not
    /// (implicit delivery, §7.4, §10.1), plus the subscribers of `scope`.
    Actors(&'a [Actor], Option<&'a str>),
    /// The connections following this user's presence.
    PresenceOf(&'a Actor),
}

#[derive(Debug, Default)]
pub(crate) struct Hub {
    conns: Mutex<HashMap<ConnId, Conn>>,
    next: AtomicU64,
    /// Set once the provider is dropped: a socket whose upgrade finishes
    /// after that is dropped as soon as it registers.
    shut: AtomicBool,
}

impl Hub {
    pub(crate) fn register(&self) -> (ConnId, mpsc::UnboundedReceiver<Outgoing>) {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::unbounded_channel();
        let conn = Conn {
            tx,
            actor: None,
            key_id: None,
            scopes: HashSet::new(),
            presence: HashSet::new(),
        };
        let mut conns = self.conns.lock().unwrap();
        // Read under the lock `drop_all` holds, so no socket slips past.
        if self.shut.load(Ordering::SeqCst) {
            let _ = conn.tx.send(Outgoing::Drop);
        }
        conns.insert(id, conn);
        (id, rx)
    }

    /// Forget a connection; returns who it was signed in as.
    pub(crate) fn unregister(&self, id: ConnId) -> Option<Actor> {
        self.conns.lock().unwrap().remove(&id).and_then(|c| c.actor)
    }

    /// Mark a connection signed in and queue its `ack` in the same step,
    /// so no event delivered to the actor can get ahead of the ack.
    pub(crate) fn authenticated(&self, id: ConnId, actor: &Actor, key_id: &str, ack: Outgoing) {
        if let Some(c) = self.conns.lock().unwrap().get_mut(&id) {
            c.actor = Some(actor.clone());
            c.key_id = Some(key_id.to_owned());
            let _ = c.tx.send(ack);
        }
    }

    /// Add subscriptions and queue `frames` (the ack, then any replay) in
    /// one step, so no live event reaches the connection between them.
    pub(crate) fn subscribe_with(&self, id: ConnId, scopes: &[String], frames: Vec<Outgoing>) {
        let mut conns = self.conns.lock().unwrap();
        let Some(c) = conns.get_mut(&id) else { return };
        c.scopes.extend(scopes.iter().cloned());
        for frame in frames {
            let _ = c.tx.send(frame);
        }
    }

    pub(crate) fn subscription_count(&self, id: ConnId) -> usize {
        self.conns
            .lock()
            .unwrap()
            .get(&id)
            .map_or(0, |c| c.scopes.len())
    }

    pub(crate) fn is_subscribed(&self, id: ConnId, scope: &str) -> bool {
        self.conns
            .lock()
            .unwrap()
            .get(&id)
            .is_some_and(|c| c.scopes.contains(scope))
    }

    pub(crate) fn unsubscribe(&self, id: ConnId, scopes: &[String]) {
        if let Some(c) = self.conns.lock().unwrap().get_mut(&id) {
            for s in scopes {
                c.scopes.remove(s);
            }
        }
    }

    /// Follow or stop following `users`' presence, queuing `ack` in the
    /// same step so no `presence.update` gets ahead of it.
    pub(crate) fn follow_presence(&self, id: ConnId, users: &[Actor], on: bool, ack: Outgoing) {
        if let Some(c) = self.conns.lock().unwrap().get_mut(&id) {
            for u in users {
                if on {
                    c.presence.insert(u.clone());
                } else {
                    c.presence.remove(u);
                }
            }
            let _ = c.tx.send(ack);
        }
    }

    /// Does `actor` have a signed-in connection here?
    pub(crate) fn online(&self, actor: &Actor) -> bool {
        self.conns
            .lock()
            .unwrap()
            .values()
            .any(|c| c.actor.as_ref() == Some(actor))
    }

    /// Queue a frame for one connection.
    pub(crate) fn send(&self, id: ConnId, out: Outgoing) {
        if let Some(c) = self.conns.lock().unwrap().get(&id) {
            let _ = c.tx.send(out);
        }
    }

    /// Fan `body` out `times` times (2 under the duplicate-events fault).
    /// The connection `reply_to` names gets its copies with the command's
    /// id as `correlationId` (§7.1).
    pub(crate) fn publish(
        &self,
        to: To<'_>,
        body: &Event,
        reply_to: Option<(ConnId, &str)>,
        times: usize,
    ) {
        let conns = self.conns.lock().unwrap();
        for (id, c) in conns.iter() {
            let Some(actor) = &c.actor else { continue };
            let wanted = match &to {
                To::Scope(scope) => c.scopes.contains(*scope),
                To::Actors(actors, scope) => {
                    actors.contains(actor) || scope.is_some_and(|s| c.scopes.contains(s))
                }
                To::PresenceOf(user) => c.presence.contains(*user),
            };
            if !wanted {
                continue;
            }
            let correlation_id = reply_to
                .filter(|(conn, _)| conn == id)
                .map(|(_, corr)| corr.to_owned());
            for _ in 0..times {
                let _ =
                    c.tx.send(Outgoing::event(body.clone(), correlation_id.clone()));
            }
        }
    }

    /// End `actor`'s subscriptions to `scope` with an unsolicited
    /// `unsubscribed` (`access_revoked`, §7.1 *Losing access*).
    pub(crate) fn revoke_access(&self, actor: &Actor, scope: &str, notice: &Event) {
        let mut conns = self.conns.lock().unwrap();
        for c in conns.values_mut() {
            if c.actor.as_ref() == Some(actor) && c.scopes.remove(scope) {
                let _ = c.tx.send(Outgoing::event(notice.clone(), None));
            }
        }
    }

    /// Close every connection signed in with `key_id`.
    pub(crate) fn close_key(&self, key_id: &str, code: u16) {
        for c in self.conns.lock().unwrap().values() {
            if c.key_id.as_deref() == Some(key_id) {
                let _ = c.tx.send(Outgoing::Close(code));
            }
        }
    }

    pub(crate) fn close_all(&self, code: u16) {
        for c in self.conns.lock().unwrap().values() {
            let _ = c.tx.send(Outgoing::Close(code));
        }
    }

    /// Drop every socket, now and any that register later.
    pub(crate) fn drop_all(&self) {
        let conns = self.conns.lock().unwrap();
        self.shut.store(true, Ordering::SeqCst);
        for c in conns.values() {
            let _ = c.tx.send(Outgoing::Drop);
        }
    }

    /// How many connections are signed in (for tests).
    pub(crate) fn signed_in(&self) -> usize {
        self.conns
            .lock()
            .unwrap()
            .values()
            .filter(|c| c.actor.is_some())
            .count()
    }
}
