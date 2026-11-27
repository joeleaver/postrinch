//! How each place tells the person of new messages (docs/ofscp.md,
//! "Organizing: views, not providers"): a channel by its own notices, else
//! its group's, else the provider's default (§10.2); a private
//! conversation always, unless muted on this computer. What gets through
//! is what the rail's badge counts and what a desktop notice tells
//! ([`super::arrived`]); what calls for the person (a mention, a reply to
//! them, a private message) makes a view's count a badge.

use postrinch_store::{
    Notices, PlaceKind, SocialMessage, SocialOutboxEntry, SocialPlace, social_account_key,
};
use serde::{Deserialize, Serialize};

use super::overlay::lay_over_group;
use super::places::SocialPlaceInfo;
use super::same_actor;
use crate::{CommandError, CommandResult, Commands};

/// How a place tells the person of new messages, settled from its own
/// setting, its group's and the defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Heard {
    /// Every new message.
    Every,
    /// Only mentions of the person and replies to them.
    MentionsAndReplies,
    /// None.
    Nothing,
}

impl Heard {
    /// A channel's or a group's setting, as heard.
    fn of(notices: Notices) -> Self {
        match notices {
            Notices::Everything => Self::Every,
            Notices::MentionsAndReplies => Self::MentionsAndReplies,
            Notices::Nothing => Self::Nothing,
        }
    }
}

/// What the provider's default is taken to be, for a channel whose group
/// sets none: the spec names no default (§10.2), and mentions and replies
/// tells the person of what is theirs without a busy group's every
/// message.
pub const PROVIDER_DEFAULT: Notices = Notices::MentionsAndReplies;

/// What a new message is to the person, by its place's notices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Notice {
    /// Its place's notices keep it quiet.
    Quiet,
    /// It gets through: counted on the rail and told.
    Told,
    /// It gets through and is to the person: a private message, a mention
    /// or a reply to them. A view's count of these is a badge.
    Called,
}

impl Notice {
    /// Whether it gets through at all.
    pub fn told(self) -> bool {
        self != Self::Quiet
    }
}

/// What a message is to the person: `heard` is how its place tells them,
/// and `to_me` whether it is a private message, mentions them or answers
/// one of theirs.
pub fn notice(heard: Heard, to_me: bool) -> Notice {
    match (heard, to_me) {
        (Heard::Nothing, _) | (Heard::MentionsAndReplies, false) => Notice::Quiet,
        (Heard::Every, false) => Notice::Told,
        (Heard::Every | Heard::MentionsAndReplies, true) => Notice::Called,
    }
}

/// How a place is heard, from its notices (as shown, pending changes
/// made) and its group's: a conversation always, unless muted; a channel
/// by its own setting, else its group's, else [`PROVIDER_DEFAULT`].
pub fn heard(place: &SocialPlace, group: Option<Notices>) -> Heard {
    match place.kind {
        PlaceKind::Conversation if place.muted => Heard::Nothing,
        PlaceKind::Conversation => Heard::Every,
        PlaceKind::Channel => Heard::of(place.notices.or(group).unwrap_or(PROVIDER_DEFAULT)),
    }
}

/// A place and how it is heard, ready to judge its messages by.
#[derive(Clone)]
pub(crate) struct Hearing {
    pub place: SocialPlace,
    pub heard: Heard,
}

impl Commands {
    /// Mute a private conversation on this computer, or hear it again. A
    /// muted conversation tells of no new message and leaves the rail's
    /// count; its messages still show, and still count as unread in its
    /// views. Muting is local: no provider is told.
    pub fn mute_social_conversation(
        &self,
        account: &str,
        id: &str,
        muted: bool,
    ) -> CommandResult<SocialPlaceInfo> {
        let place = self.check_place(account, id)?;
        if place.kind != PlaceKind::Conversation {
            return Err(CommandError::Invalid(
                "Only a private conversation is muted; a channel has its notices.".into(),
            ));
        }
        if place.muted != muted {
            self.store
                .change_social_place(&place.account, &place.id, |p| p.muted = muted)?;
        }
        self.social_place(&place.account, &place.id)
    }

    /// How many unread messages get through their places' notices, in
    /// every account this caller sees: the rail's badge for Social.
    pub fn social_noticed(&self) -> CommandResult<usize> {
        let mut told = 0;
        for account in self.store.social_accounts()? {
            if !self.sees_social(&account.actor) {
                continue;
            }
            for place in self.store.social_places(&account.actor)? {
                let hearing = self.hearing(place)?;
                if hearing.heard == Heard::Nothing {
                    continue;
                }
                for message in self
                    .store
                    .social_unread_messages(&hearing.place.account, &hearing.place.id)?
                {
                    told += usize::from(self.notice_of(&hearing, &message)?.told());
                }
            }
        }
        Ok(told)
    }

    /// `place` as shown, pending changes made, and how it is heard.
    pub(crate) fn hearing(&self, mut place: SocialPlace) -> CommandResult<Hearing> {
        place.account = social_account_key(&place.account);
        self.place_outbox(&place.account, &place.id)?
            .lay_over_place(&mut place);
        let group = match (&place.kind, &place.group) {
            (PlaceKind::Channel, Some(uri)) if place.notices.is_none() => {
                let outbox = self.store.social_outbox(&place.account)?;
                self.group_notices(&place.account, uri, &outbox)?
            }
            _ => None,
        };
        let heard = heard(&place, group);
        Ok(Hearing { place, heard })
    }

    /// A group's notices as shown, pending changes made; `None` for the
    /// provider's default, or a group not kept.
    fn group_notices(
        &self,
        account: &str,
        uri: &str,
        outbox: &[SocialOutboxEntry],
    ) -> CommandResult<Option<Notices>> {
        Ok(self
            .store
            .social_group(account, uri)?
            .and_then(|mut group| {
                lay_over_group(&mut group, outbox);
                group.notices
            }))
    }

    /// What an unread `message` of a heard place is to the person. A
    /// message of their own, or one deleted, is quiet.
    pub(crate) fn notice_of(
        &self,
        hearing: &Hearing,
        message: &SocialMessage,
    ) -> CommandResult<Notice> {
        let place = &hearing.place;
        if hearing.heard == Heard::Nothing
            || message.deleted
            || same_actor(&message.author, &place.account)
        {
            return Ok(Notice::Quiet);
        }
        let to_me = place.kind == PlaceKind::Conversation
            || message.mentions_me
            || self.answers(&place.account, message)?;
        Ok(notice(hearing.heard, to_me))
    }

    /// Whether `message` replies to one of `account`'s own.
    fn answers(&self, account: &str, message: &SocialMessage) -> CommandResult<bool> {
        let Some(parent) = &message.parent else {
            return Ok(false);
        };
        Ok(self
            .store
            .social_message(account, &message.place, parent)?
            .is_some_and(|p| same_actor(&p.author, account)))
    }
}
