//! Typing indicators (§7.1, §7.4): soft state that expires after 6 s
//! without a refresh, and stops when the typing connection goes.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use ofscp_core::ws::{ChannelTyping, DmTyping, Event, TypingState};
use ofscp_core::{Actor, DmId, LocalId};

use super::hub::{ConnId, To};
use crate::inner::Inner;

/// RECOMMENDED expiry of a `start` without a refresh.
const EXPIRY: Duration = Duration::from_secs(6);

/// Where someone is typing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Place {
    Channel(LocalId),
    /// A conversation, and the participants this provider delivers to.
    Dm(DmId, Vec<Actor>),
}

impl Place {
    fn key(&self) -> String {
        match self {
            Self::Channel(c) => c.as_str().to_owned(),
            Self::Dm(d, _) => d.as_str().to_owned(),
        }
    }

    fn publish(&self, inner: &Inner, user: &Actor, state: TypingState) {
        match self {
            Self::Channel(c) => {
                let event = Event::ChannelTyping(ChannelTyping {
                    channel_id: c.clone(),
                    user: user.clone(),
                    state,
                });
                inner.hub.publish(To::Scope(c.as_str()), &event, None, 1);
            }
            Self::Dm(d, to) => {
                let event = Event::DmTyping(DmTyping {
                    dm_id: d.clone(),
                    user: user.clone(),
                    state,
                });
                inner
                    .hub
                    .publish(To::Actors(to, Some(d.as_str())), &event, None, 1);
            }
        }
    }
}

struct Active {
    place: Place,
    user: Actor,
    generation: u64,
}

#[derive(Default)]
pub(crate) struct Typing {
    active: Mutex<HashMap<(ConnId, String), Active>>,
    generation: Mutex<u64>,
}

impl Typing {
    pub(crate) fn start(&self, inner: &Arc<Inner>, conn: ConnId, user: &Actor, place: Place) {
        let generation = {
            let mut g = self.generation.lock().unwrap();
            *g += 1;
            *g
        };
        let key = (conn, place.key());
        self.active.lock().unwrap().insert(
            key.clone(),
            Active {
                place: place.clone(),
                user: user.clone(),
                generation,
            },
        );
        place.publish(inner, user, TypingState::Start);
        let inner = inner.clone();
        tokio::spawn(async move {
            tokio::time::sleep(EXPIRY).await;
            let expired = {
                let mut active = inner.typing.active.lock().unwrap();
                match active.get(&key) {
                    Some(a) if a.generation == generation => active.remove(&key),
                    _ => None,
                }
            };
            if let Some(a) = expired {
                a.place.publish(&inner, &a.user, TypingState::Stop);
            }
        });
    }

    pub(crate) fn stop(&self, inner: &Inner, conn: ConnId, user: &Actor, place: Place) {
        self.active.lock().unwrap().remove(&(conn, place.key()));
        place.publish(inner, user, TypingState::Stop);
    }

    /// The connection went: stop everything it was typing.
    pub(crate) fn disconnected(&self, inner: &Inner, conn: ConnId) {
        let gone: Vec<Active> = {
            let mut active = self.active.lock().unwrap();
            let keys: Vec<_> = active.keys().filter(|(c, _)| *c == conn).cloned().collect();
            keys.into_iter().filter_map(|k| active.remove(&k)).collect()
        };
        for a in gone {
            a.place.publish(inner, &a.user, TypingState::Stop);
        }
    }
}
