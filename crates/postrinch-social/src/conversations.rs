//! Private conversations (§7.4): starting one, and who is on the other
//! side of one kept.
//!
//! Each message lives only with its recipient, so a conversation with
//! someone on another provider is read from both: home holds what the
//! person received, the other person's provider what they sent
//! (docs/ofscp.md, "Private messages").

use postrinch_ofscp::ofscp_core::wire::{DmConversation, ErrorCode};
use postrinch_ofscp::ofscp_core::{Actor, Authority, dm_id};
use postrinch_store::{PlaceKind, SocialPlace, social_account_key};

use crate::live::{Control, LiveEvent};
use crate::sync::Run;
use crate::{Social, SocialError, SocialResult, parse_actor};

impl Social {
    /// Start a private conversation between `actor`'s account and `with`
    /// (`handle@domain`), and give its place's id, the `dmId`. Nothing is
    /// sent: the conversation is kept here so a first message can be
    /// queued to it, and home learns of it when that message is sent (its
    /// conversation pointer, §7.4). A conversation already kept is given
    /// as it is.
    ///
    /// `with` is looked up at their provider first, so a mistyped address
    /// fails here rather than as a refused message
    /// ([`SocialError::NoSuchPerson`]).
    pub async fn start_conversation(&self, actor: &str, with: &str) -> SocialResult<String> {
        let actor = parse_actor(actor)?;
        let with = parse_actor(with)?;
        if same(&actor, &with) {
            return Err(SocialError::WithYourself(actor.to_string()));
        }
        let dm = dm_id(&actor, &with);
        let place = dm.to_string();
        let account = self.account(&actor).await?;
        let (name, id) = (account.actor.clone(), place.clone());
        let kept = self
            .blocking(move |store| store.social_place(&name, &id))
            .await?;
        if kept.is_some() {
            return Ok(place);
        }
        let identity = self.identity(&account).await?;
        let there = self.client(&identity, with.domain());
        let found = there.profile(&with).await;
        match self.checked(&identity, &there, found).await {
            Ok(_) => {}
            Err(SocialError::Transport(error))
                if error.problem().is_some_and(|p| p.is(&ErrorCode::NotFound)) =>
            {
                return Err(SocialError::NoSuchPerson(with.to_string()));
            }
            Err(error) => return Err(error),
        }
        let conversation = DmConversation {
            id: dm,
            participants: [actor.clone(), with],
            last_message: None,
            updated_at: there.provider().clock().now(),
            metadata: Vec::new(),
        };
        Run::new(self, &identity)
            .conversation(&conversation)
            .await?;
        let name = actor.to_string();
        self.live.tell(LiveEvent::Place {
            account: name.clone(),
            place: place.clone(),
        });
        // A live account opens a session at their provider, to follow what
        // the person sends there.
        self.live
            .control(&social_account_key(&name), Control::Refreshed);
        Ok(place)
    }
}

/// The other person in a conversation kept for `me`; `None` for a
/// channel, or a conversation whose participants do not read or that `me`
/// is not in.
pub(crate) fn counterparty(place: &SocialPlace, me: &Actor) -> Option<Actor> {
    if place.kind != PlaceKind::Conversation {
        return None;
    }
    let conversation: DmConversation = match serde_json::from_value(place.object.clone()) {
        Ok(conversation) => conversation,
        Err(error) => {
            tracing::warn!(account = %place.account, place = %place.id, %error, "a kept conversation that does not read");
            return None;
        }
    };
    let [a, b] = conversation.participants;
    match (same(&a, me), same(&b, me)) {
        (true, _) => Some(b),
        (false, true) => Some(a),
        (false, false) => None,
    }
}

/// Where the messages `me` sent in a conversation kept for them are held,
/// when that is not home: the other person's provider (§7.4). `None` for
/// a channel, and for a conversation with someone on `me`'s own provider,
/// whose home holds every message of it.
pub(crate) fn sent_provider(place: &SocialPlace, me: &Actor) -> Option<Authority> {
    let other = counterparty(place, me)?;
    (other.domain() != me.domain()).then(|| other.domain().clone())
}

/// Whether two actors are one, in any spelling.
fn same(a: &Actor, b: &Actor) -> bool {
    social_account_key(&a.to_string()) == social_account_key(&b.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn place(participants: [&str; 2], kind: PlaceKind) -> SocialPlace {
        let a = Actor::canonicalize(participants[0]).unwrap();
        let b = Actor::canonicalize(participants[1]).unwrap();
        SocialPlace {
            account: participants[0].into(),
            id: dm_id(&a, &b).to_string(),
            kind,
            group: None,
            followed: false,
            last_activity: None,
            object: json!({
                "id": dm_id(&a, &b).to_string(),
                "participants": participants,
                "updatedAt": "2026-01-01T00:00:00Z",
            }),
            cursors: Default::default(),
            read_marker: None,
            notices: None,
            muted: false,
        }
    }

    fn actor(s: &str) -> Actor {
        Actor::canonicalize(s).unwrap()
    }

    #[test]
    fn sent_messages_are_held_at_the_other_persons_provider() {
        let p = place(["ada@a.example", "erin@b.example"], PlaceKind::Conversation);
        let me = actor("ada@a.example");
        assert_eq!(counterparty(&p, &me), Some(actor("erin@b.example")));
        assert_eq!(
            sent_provider(&p, &me).map(|a| a.to_string()),
            Some("b.example".into())
        );
    }

    #[test]
    fn within_one_provider_home_holds_everything() {
        let p = place(["bob@a.example", "ada@a.example"], PlaceKind::Conversation);
        let me = actor("ada@a.example");
        assert_eq!(counterparty(&p, &me), Some(actor("bob@a.example")));
        assert_eq!(sent_provider(&p, &me), None);
    }

    #[test]
    fn a_channel_or_a_stranger_has_no_counterparty() {
        let channel = place(["ada@a.example", "erin@b.example"], PlaceKind::Channel);
        assert_eq!(counterparty(&channel, &actor("ada@a.example")), None);
        let theirs = place(["bob@a.example", "erin@b.example"], PlaceKind::Conversation);
        assert_eq!(counterparty(&theirs, &actor("ada@a.example")), None);
    }
}
