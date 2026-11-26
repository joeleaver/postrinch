//! Places (channels and private conversations) and groups: listing them,
//! following a channel (§7.6), and how each tells the person of new
//! messages (§10.2).

use postrinch_store::{
    ContentKind, Notices, PlaceKind, SocialChange, SocialGroup, SocialPlace, social_account_key,
};
use serde::{Deserialize, Serialize};

use super::overlay::{PlaceOutbox, SocialPending, lay_over_group};
use super::{new_id, now, same_actor, str_at};
use crate::{CommandError, CommandResult, Commands};

/// A place as the sidebar, a view's tabs and All channels show it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialPlaceInfo {
    /// The place, its pending follow and notices setting made.
    pub place: SocialPlace,
    /// A channel's name, or the other person in a conversation.
    pub name: String,
    /// A channel's group's name.
    pub group_name: Option<String>,
    pub unread: usize,
    /// Unread messages that mention the person.
    pub mentions: usize,
    /// The kinds the account may start a thread with here (§5.2.1); empty
    /// where it may only reply, or not post at all.
    pub may_post: Vec<ContentKind>,
    pub may_react: bool,
    /// The last change to the place still on its way, if any.
    pub pending: Option<SocialPending>,
}

/// A group as the view panel shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialGroupInfo {
    /// The group, its pending notices setting made.
    pub group: SocialGroup,
    pub name: String,
    pub pending: Option<SocialPending>,
}

impl Commands {
    /// Every place of every account this caller sees, as All channels
    /// lists them: by account, then by name.
    pub fn social_places(&self) -> CommandResult<Vec<SocialPlaceInfo>> {
        let mut out = Vec::new();
        for account in self.store.social_accounts()? {
            if !self.sees_social(&account.actor) {
                continue;
            }
            for place in self.places_by_name(&account.actor)? {
                out.push(self.place_info(place)?);
            }
        }
        Ok(out)
    }

    /// An account's places, by name, as All channels and the built-in
    /// views' tabs list them.
    pub(crate) fn places_by_name(&self, actor: &str) -> CommandResult<Vec<SocialPlace>> {
        let mut places = self.store.social_places(actor)?;
        places.sort_by_cached_key(|p| place_name(p).to_lowercase());
        Ok(places)
    }

    /// The place `id` (a channel URI or a dmId) of an account.
    pub fn social_place(&self, account: &str, id: &str) -> CommandResult<SocialPlaceInfo> {
        let place = self.check_place(account, id)?;
        self.place_info(place)
    }

    /// The groups of every account this caller sees, by account, then by
    /// name.
    pub fn social_groups(&self) -> CommandResult<Vec<SocialGroupInfo>> {
        let mut out = Vec::new();
        for account in self.store.social_accounts()? {
            if !self.sees_social(&account.actor) {
                continue;
            }
            let outbox = self.store.social_outbox(&account.actor)?;
            let mut groups: Vec<SocialGroupInfo> = self
                .store
                .social_groups(&account.actor)?
                .into_iter()
                .map(|group| group_info(group, &outbox))
                .collect();
            groups.sort_by_cached_key(|p| p.name.to_lowercase());
            out.extend(groups);
        }
        Ok(out)
    }

    /// Follow a channel, so that its posts come to Home, or stop following
    /// it. It shows at once, and reaches the home provider through the
    /// outbox.
    pub fn follow_social_place(
        &self,
        account: &str,
        id: &str,
        on: bool,
    ) -> CommandResult<SocialPlaceInfo> {
        let place = self.check_place(account, id)?;
        if place.kind == PlaceKind::Conversation {
            return Err(CommandError::Invalid(
                "Only channels are followed; a conversation always comes to Messages.".into(),
            ));
        }
        let shown = self.place_info(place.clone())?;
        if shown.place.followed != on {
            let change = SocialChange::Follow {
                group: place.group.clone(),
                on,
            };
            self.queue_change(&place.account, Some(&place.id), &change)?;
        }
        self.place_info(place)
    }

    /// How a channel tells the person of new messages; `None` for its
    /// group's setting. Synced with the home provider (§10.2).
    pub fn set_social_place_notices(
        &self,
        account: &str,
        id: &str,
        notices: Option<Notices>,
    ) -> CommandResult<SocialPlaceInfo> {
        let place = self.check_place(account, id)?;
        if place.kind == PlaceKind::Conversation {
            return Err(CommandError::Invalid(
                "A private conversation always tells you of new messages.".into(),
            ));
        }
        if self.place_info(place.clone())?.place.notices != notices {
            let change = SocialChange::ChannelNotices { notices };
            self.queue_change(&place.account, Some(&place.id), &change)?;
        }
        self.place_info(place)
    }

    /// How a group's channels tell the person of new messages; `None` for
    /// the provider's default (§10.2).
    pub fn set_social_group_notices(
        &self,
        account: &str,
        group: &str,
        notices: Option<Notices>,
    ) -> CommandResult<SocialGroupInfo> {
        let account = self.check_social(account)?;
        let Some(kept) = self.store.social_group(&account.actor, group)? else {
            return Err(CommandError::Invalid(format!(
                "{} knows no group {group}.",
                account.actor
            )));
        };
        let outbox = self.store.social_outbox(&account.actor)?;
        if group_info(kept.clone(), &outbox).group.notices != notices {
            let change = SocialChange::GroupNotices {
                group: kept.uri.clone(),
                notices,
            };
            self.queue_change(&account.actor, None, &change)?;
        }
        let outbox = self.store.social_outbox(&account.actor)?;
        Ok(group_info(kept, &outbox))
    }

    /// The place `id` of `account` as kept, when this caller sees the
    /// account and the account knows the place.
    pub(crate) fn check_place(&self, account: &str, id: &str) -> CommandResult<SocialPlace> {
        let account = self.check_social(account)?;
        self.store
            .social_place(&account.actor, id)?
            .ok_or_else(|| CommandError::Invalid(format!("{} knows no place {id}.", account.actor)))
    }

    /// The changes to a place still on their way.
    pub(crate) fn place_outbox(&self, account: &str, place: &str) -> CommandResult<PlaceOutbox> {
        Ok(PlaceOutbox::new(
            self.store.social_outbox_for(account, place)?,
        ))
    }

    /// Queue `change` from `account`, made now, with a new client id.
    pub(crate) fn queue_change(
        &self,
        account: &str,
        place: Option<&str>,
        change: &SocialChange,
    ) -> CommandResult<postrinch_store::SocialOutboxEntry> {
        Ok(self
            .store
            .queue_social_change(account, &new_id(), place, change, now())?)
    }

    /// A place as shown.
    pub(crate) fn place_info(&self, mut place: SocialPlace) -> CommandResult<SocialPlaceInfo> {
        place.account = social_account_key(&place.account);
        let pending = self
            .place_outbox(&place.account, &place.id)?
            .lay_over_place(&mut place);
        let unread = self
            .store
            .social_unread_messages(&place.account, &place.id)?;
        let group_name = match &place.group {
            Some(uri) => self
                .store
                .social_group(&place.account, uri)?
                .map(|g| group_name(&g)),
            None => None,
        };
        let standing = self.standing(&place)?;
        Ok(SocialPlaceInfo {
            name: place_name(&place),
            group_name,
            unread: unread.len(),
            mentions: unread.iter().filter(|m| m.mentions_me).count(),
            may_post: standing.kinds(),
            may_react: standing.may_react(),
            pending,
            place,
        })
    }
}

/// A group as shown, from its account's changes still on their way.
fn group_info(
    mut group: SocialGroup,
    outbox: &[postrinch_store::SocialOutboxEntry],
) -> SocialGroupInfo {
    let pending = lay_over_group(&mut group, outbox);
    SocialGroupInfo {
        name: group_name(&group),
        group,
        pending,
    }
}

/// A group's name, or its URI when it gives none.
fn group_name(group: &SocialGroup) -> String {
    str_at(&group.group, "name")
        .unwrap_or(&group.uri)
        .to_owned()
}

/// A channel's name; for a conversation, the other person's actor (§7.4
/// `participants`). The id when the place gives neither.
pub(crate) fn place_name(place: &SocialPlace) -> String {
    match place.kind {
        PlaceKind::Channel => str_at(&place.object, "name").map(str::to_owned),
        PlaceKind::Conversation => place
            .object
            .get("participants")
            .and_then(serde_json::Value::as_array)
            .and_then(|people| {
                people
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .find(|p| !same_actor(p, &place.account))
                    .map(str::to_owned)
            }),
    }
    .unwrap_or_else(|| place.id.clone())
}
