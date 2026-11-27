//! Where a queued change goes: the place it names, read from the store,
//! and the provider holding each message it names (docs/ofscp.md,
//! "Private messages").

use postrinch_ofscp::ofscp_core::wire::DmConversation;
use postrinch_ofscp::ofscp_core::{Actor, Authority, DmId, LocalId};
use postrinch_store::{PlaceKind, SocialPlace, social_account_key};

use crate::uris::{ChannelAt, parse_channel_uri, parse_dm_message_url};

/// A place a change goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// A channel, at the provider hosting it.
    Channel(ChannelAt),
    /// A private conversation: its id, and the other person in it. Each
    /// message lives with its recipient (§7.4).
    Conversation { dm: DmId, other: Actor },
}

/// A message as the provider holding it names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageAt {
    pub holder: Authority,
    pub id: LocalId,
}

impl Target {
    /// Where `place`, kept for `me`, is; `None` for a place whose id or
    /// conversation does not read, or a conversation `me` is not in (it
    /// cannot be sent to).
    pub(crate) fn of(place: &SocialPlace, me: &Actor) -> Option<Self> {
        match place.kind {
            PlaceKind::Channel => parse_channel_uri(&place.id).map(Self::Channel),
            PlaceKind::Conversation => {
                let dm = DmId::parse(&place.id).ok()?;
                let conversation: DmConversation =
                    serde_json::from_value(place.object.clone()).ok()?;
                let me = social_account_key(&me.to_string());
                let is_me = |who: &Actor| social_account_key(&who.to_string()) == me;
                let [a, b] = conversation.participants;
                let other = match (is_me(&a), is_me(&b)) {
                    (true, _) => b,
                    (false, true) => a,
                    (false, false) => return None,
                };
                Some(Self::Conversation { dm, other })
            }
        }
    }

    /// The provider hosting the place: a channel's, or home for a
    /// conversation. What its messages are kept against.
    pub(crate) fn host(&self, home: &Authority) -> Authority {
        match self {
            Self::Channel(at) => at.authority.clone(),
            Self::Conversation { .. } => home.clone(),
        }
    }

    /// The provider that will hold a message sent here, when it is not
    /// home: what an upload for it names as its `target` (§5.8 *Target*).
    /// A channel's host; in a conversation, the other person's provider,
    /// which keeps the messages sent to them (§7.4).
    pub(crate) fn upload_target(&self, home: &Authority) -> Option<Authority> {
        let holder = match self {
            Self::Channel(at) => &at.authority,
            Self::Conversation { other, .. } => other.domain(),
        };
        (holder != home).then(|| holder.clone())
    }

    /// The message kept here as `kept`, at the provider holding it: in a
    /// channel, its host; in a conversation, home for a bare id, and the
    /// provider its URL names for one held at the other person's
    /// provider. `None` for an id that is neither.
    pub(crate) fn message(&self, kept: &str, home: &Authority) -> Option<MessageAt> {
        let at = |holder: &Authority, id: &str| {
            Some(MessageAt {
                holder: holder.clone(),
                id: LocalId::new(id).ok()?,
            })
        };
        match self {
            Self::Channel(channel) => at(&channel.authority, kept),
            Self::Conversation { .. } => match parse_dm_message_url(kept) {
                Some((holder, id)) => Some(MessageAt { holder, id }),
                None => at(home, kept),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn conversation(me: &str, other: &str) -> SocialPlace {
        let a = Actor::canonicalize(me).unwrap();
        let b = Actor::canonicalize(other).unwrap();
        let dm = DmId::derive(&a, &b);
        SocialPlace {
            account: me.into(),
            id: dm.to_string(),
            kind: PlaceKind::Conversation,
            group: None,
            followed: false,
            last_activity: None,
            object: json!({
                "id": dm.to_string(),
                "participants": [me, other],
                "updatedAt": "2026-01-01T00:00:00Z",
            }),
            cursors: Default::default(),
            read_marker: None,
            notices: None,
            muted: false,
        }
    }

    #[test]
    fn a_conversations_messages_are_held_where_their_keys_say() {
        let me = Actor::canonicalize("ada@home.example").unwrap();
        let home = me.domain().clone();
        let place = conversation("ada@home.example", "bob@away.example");
        let target = Target::of(&place, &me).unwrap();
        let Target::Conversation { other, .. } = &target else {
            panic!("a conversation");
        };
        assert_eq!(other.to_string(), "bob@away.example");
        assert_eq!(target.host(&home), home);
        let stranger = Actor::canonicalize("carol@else.example").unwrap();
        assert_eq!(Target::of(&place, &stranger), None);

        let mine = target.message("m1", &home).unwrap();
        assert_eq!(
            (mine.holder.to_string(), mine.id.as_str()),
            ("home.example".into(), "m1")
        );
        let url = format!("https://away.example/api/dms/{}/messages/m2", place.id);
        let theirs = target.message(&url, &home).unwrap();
        assert_eq!(theirs.holder.to_string(), "away.example");
        assert_eq!(theirs.id.as_str(), "m2");
    }

    #[test]
    fn a_channels_messages_are_held_by_its_host() {
        let me = Actor::canonicalize("ada@home.example").unwrap();
        let mut place = conversation("ada@home.example", "bob@away.example");
        place.kind = PlaceKind::Channel;
        place.id = "https://groups.example/api/groups/g1/channels/c1".into();
        let target = Target::of(&place, &me).unwrap();
        let held = target.message("m1", me.domain()).unwrap();
        assert_eq!(held.holder.to_string(), "groups.example");
        assert_eq!(target.host(me.domain()).to_string(), "groups.example");
    }
}
