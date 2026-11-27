//! Reading a frame and answering its command (§7.1 *WebSocket message
//! types*).

use ofscp_core::wire::{Availability, SetAvailability};
use ofscp_core::ws::{
    Authenticate, Authenticated, CLOSE_AUTH_FAILED, ChannelMessage, Command, DecodeError, Envelope,
    Event, MessageDeleted, PresenceUsers, PresenceUsersAck, ReactionAdded, ReactionRemoved, Typing,
    Unsubscribed,
};
use ofscp_core::{Actor, DmId};

use super::hub::Outgoing;
use super::typing::Place;
use super::{Conn, Flow, presence_of, publish_presence, subscribe};
use crate::ops::{Made, messages};
use crate::problem::Problem;
use crate::verify::verify_ws;
use tokio::time::Instant;

pub(super) async fn handle(conn: &mut Conn, text: &str) -> Flow {
    let envelope = match Envelope::<Command>::decode(text) {
        Ok(e) => e,
        // Before authentication anything but a good `authenticate` closes
        // the connection (§7.1 *Authentication*).
        Err(_) if conn.actor.is_none() => {
            return fail_auth(conn, None, Problem::unauthorized("malformed authenticate"));
        }
        Err(DecodeError::Data { id, .. }) => {
            conn.error(
                Problem::bad_request("data does not match the command's schema"),
                Some(id),
            );
            return Flow::Continue;
        }
        Err(_) => {
            conn.error(Problem::bad_request("not a command envelope"), None);
            return Flow::Continue;
        }
    };
    let corr = envelope.id;
    let Some(actor) = conn.actor.clone() else {
        return match envelope.body {
            Command::Authenticate(auth) => authenticate(conn, corr, auth).await,
            _ => Flow::Close(CLOSE_AUTH_FAILED),
        };
    };
    let reply = Some((conn.id, corr.as_str()));
    let inner = conn.inner.clone();
    let outcome: Result<Option<Event>, Problem> = match envelope.body {
        Command::Authenticate(_) => Err(Problem::bad_request("already authenticated")),
        Command::Subscribe(cmd) => {
            subscribe::subscribe(conn, corr, cmd);
            return Flow::Continue;
        }
        Command::Unsubscribe(cmd) => {
            let scopes: Vec<String> = cmd.channels.iter().map(|c| c.as_str().to_owned()).collect();
            inner.hub.unsubscribe(conn.id, &scopes);
            Ok(Some(Event::Unsubscribed(Unsubscribed {
                channels: cmd.channels,
                reason: None,
            })))
        }
        Command::MessageCreate(cmd) => {
            messages::create(
                &inner,
                &actor,
                &cmd.group_id,
                &cmd.channel_id,
                cmd.message,
                reply,
            )
            .map(|made| match made {
                // The retry's answer: to this connection only, with no
                // cursor (§7.1 *Idempotency*).
                Made::Existing(m) => Some(Event::MessageCreated(ChannelMessage {
                    group_id: cmd.group_id,
                    channel_id: cmd.channel_id,
                    cursor: None,
                    message: Box::new(m),
                })),
                Made::New(_) => None,
            })
        }
        Command::MessageUpdate(cmd) => messages::update(
            &inner,
            &actor,
            &cmd.group_id,
            &cmd.channel_id,
            &cmd.message_id,
            cmd.update,
            reply,
        )
        .map(|_| None),
        Command::MessageDelete(cmd) => messages::delete(
            &inner,
            &actor,
            &cmd.group_id,
            &cmd.channel_id,
            &cmd.message_id,
            reply,
        )
        .map(|made| match made {
            Made::Existing(at) => Some(Event::MessageDeleted(MessageDeleted {
                group_id: cmd.group_id,
                channel_id: cmd.channel_id,
                cursor: None,
                message_id: cmd.message_id,
                deleted_at: Some(at),
            })),
            Made::New(_) => None,
        }),
        Command::ReactionAdd(cmd) => messages::react(
            &inner,
            &actor,
            &cmd.group_id,
            &cmd.channel_id,
            &cmd.message_id,
            messages::NewReaction {
                key: &cmd.key,
                unicode: cmd.unicode,
                image: cmd.image,
            },
            reply,
        )
        .map(|made| match made {
            Made::Existing(reaction) => Some(Event::ReactionAdded(ReactionAdded {
                group_id: cmd.group_id,
                channel_id: cmd.channel_id,
                reaction,
            })),
            Made::New(_) => None,
        }),
        Command::ReactionRemove(cmd) => messages::unreact(
            &inner,
            &actor,
            &cmd.group_id,
            &cmd.channel_id,
            &cmd.message_id,
            &cmd.key,
            reply,
        )
        .map(|removed| {
            (!removed).then(|| {
                Event::ReactionRemoved(ReactionRemoved {
                    group_id: cmd.group_id,
                    channel_id: cmd.channel_id,
                    message_id: cmd.message_id,
                    key: cmd.key,
                    author: actor.clone(),
                })
            })
        }),
        Command::TypingStart(t) => typing(conn, &actor, t, true),
        Command::TypingStop(t) => typing(conn, &actor, t, false),
        Command::PresenceSubscribe(p) => return presence_subscribe(conn, corr, p),
        Command::PresenceUnsubscribe(p) => {
            let ack = Event::PresenceUnsubscribed(PresenceUsersAck {
                users: p.users.clone(),
            });
            let ack = Outgoing::event(ack, Some(corr));
            inner.hub.follow_presence(conn.id, &p.users, false, ack);
            return Flow::Continue;
        }
        Command::PresenceSet(p) => {
            set_presence(&inner, &actor, p.availability, p.status).map(|()| None)
        }
        Command::Ping(_) => Ok(Some(Event::Pong(Default::default()))),
        Command::Pong(_) => Ok(None),
        Command::Other { .. } => Ok(None),
    };
    match outcome {
        Ok(Some(event)) => {
            conn.emit(event, Some(corr));
        }
        Ok(None) => {}
        Err(problem) => {
            conn.error(problem, Some(corr));
        }
    }
    Flow::Continue
}

fn fail_auth(conn: &Conn, corr: Option<String>, problem: Problem) -> Flow {
    conn.error(problem, corr);
    Flow::Close(CLOSE_AUTH_FAILED)
}

async fn authenticate(conn: &mut Conn, corr: String, auth: Authenticate) -> Flow {
    let inner = conn.inner.clone();
    if inner.faults.lock().unwrap().refuse_ws_auth {
        return fail_auth(
            conn,
            Some(corr),
            Problem::unauthorized("fault: authentication refused"),
        );
    }
    // The challenge is good for one attempt, and only until it expires.
    let Some((nonce, expires_at)) = conn.challenge.take() else {
        return fail_auth(
            conn,
            Some(corr),
            Problem::unauthorized("the challenge was used"),
        );
    };
    if Instant::now() > expires_at {
        return fail_auth(
            conn,
            Some(corr),
            Problem::unauthorized("the challenge expired"),
        );
    }
    match verify_ws(&inner, &auth, &nonce).await {
        Ok(verified) => {
            let first = !inner.hub.online(&verified.actor);
            let ack = Event::Authenticated(Authenticated {
                actor: verified.actor.clone(),
            });
            inner.hub.authenticated(
                conn.id,
                &verified.actor,
                &verified.key_id,
                Outgoing::event(ack, Some(corr)),
            );
            conn.actor = Some(verified.actor.clone());
            conn.key_id = Some(verified.key_id);
            conn.key_until = verified.until;
            if first {
                publish_presence(&inner, &verified.actor);
            }
            Flow::Continue
        }
        Err(problem) => fail_auth(conn, Some(corr), problem),
    }
}

/// `typing.start` / `typing.stop` on a channel or a dmId.
fn typing(conn: &Conn, actor: &Actor, t: Typing, start: bool) -> Result<Option<Event>, Problem> {
    let inner = &conn.inner;
    let place = {
        let mut state = inner.state.lock().unwrap();
        if t.channel_id.as_str().starts_with("dm_") {
            let dm = DmId::parse(t.channel_id.as_str())
                .map_err(|_| Problem::not_found("no such conversation"))?;
            if !crate::ops::dms::is_participant(inner, &state, &dm, actor) {
                // As for a conversation that does not exist (§7.4).
                return Err(Problem::not_found("no such conversation"));
            }
            let other = state.local_participant(&inner.authority, &dm, actor);
            let mut to: Vec<Actor> = other.into_iter().collect();
            if actor.is_local_to(&inner.authority) {
                to.push(actor.clone());
            }
            Place::Dm(dm, to)
        } else {
            let group = state
                .channels
                .get(&t.channel_id)
                .map(|c| c.channel.group_id.clone())
                .ok_or_else(|| Problem::not_found("no such channel"))?;
            messages::check_readable(&mut state, &group, &t.channel_id, Some(actor))?;
            Place::Channel(t.channel_id)
        }
    };
    if start {
        inner.typing.start(inner, conn.id, actor, place);
    } else {
        inner.typing.stop(inner, conn.id, actor, place);
    }
    Ok(None)
}

fn presence_subscribe(conn: &Conn, corr: String, p: PresenceUsers) -> Flow {
    let inner = conn.inner.clone();
    let (hosted, refused): (Vec<Actor>, Vec<Actor>) = p.users.into_iter().partition(|u| {
        let state = inner.state.lock().unwrap();
        state.local_user(&inner.authority, u).is_some()
    });
    if !hosted.is_empty() {
        let ack = Event::PresenceSubscribed(PresenceUsersAck {
            users: hosted.clone(),
        });
        let ack = Outgoing::event(ack, Some(corr.clone()));
        inner.hub.follow_presence(conn.id, &hosted, true, ack);
        for user in hosted {
            if let Some(presence) = presence_of(&inner, &user) {
                let event =
                    Event::PresenceUpdate(ofscp_core::ws::PresenceChanged { user, presence });
                conn.emit(event, None);
            }
        }
    }
    if !refused.is_empty() {
        let p = Problem::not_found("no such user")
            .with_details(serde_json::json!({ "users": refused }));
        conn.error(p, Some(corr));
    }
    Flow::Continue
}

/// `presence.set` and `PUT /api/me/presence`: a full replace (§6.4, §7.5).
pub(crate) fn set_presence(
    inner: &crate::inner::Inner,
    actor: &Actor,
    availability: SetAvailability,
    status: Option<String>,
) -> Result<(), Problem> {
    {
        let mut state = inner.state.lock().unwrap();
        let user = state
            .local_user_mut(&inner.authority, actor)
            .ok_or_else(|| Problem::forbidden("presence is set at the user's home provider"))?;
        user.availability = match availability {
            SetAvailability::Online => Availability::Online,
            SetAvailability::Away => Availability::Away,
            SetAvailability::Dnd => Availability::Dnd,
        };
        user.status = status;
    }
    publish_presence(inner, actor);
    Ok(())
}
