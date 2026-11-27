//! What an account may do in a place, as its group and channel permit
//! (§5.2 group permissions, §5.2.1 per-channel permissions): post each
//! kind, post only replies, react, and moderate others' messages. The
//! provider decides in the end; these say beforehand what it will refuse,
//! so the window offers only what can be done.

use postrinch_store::{ContentKind, PlaceKind, SocialGroup, SocialMessage, SocialPlace};
use serde_json::Value;

use super::{kind_of, same_actor, str_at, type_name};
use crate::{CommandError, CommandResult, Commands};

/// The group's `post` roles when it names none (§5.2).
const DEFAULT_POST: [&str; 2] = ["admin", "member"];
/// The group's `moderate` roles when it names none (§5.2).
const DEFAULT_MODERATE: [&str; 1] = ["admin"];
/// The group's `manage` roles when it names none (§5.2).
const DEFAULT_MANAGE: [&str; 1] = ["admin"];

/// Where an account stands in a place.
pub(crate) struct Standing {
    /// A private conversation: either side may write chat lines in it.
    conversation: bool,
    /// The group's owner, who may do everything (§5.2).
    owner: bool,
    /// Its role in the group; `None` when not a member.
    role: Option<String>,
    /// The group's `permissions`, action to roles.
    group: Value,
    /// The channel's `permissions` (§5.2.1).
    channel: Value,
}

impl Commands {
    /// Where the account of `place` stands in it.
    pub(crate) fn standing(&self, place: &SocialPlace) -> CommandResult<Standing> {
        let mut standing = Standing {
            conversation: place.kind == PlaceKind::Conversation,
            owner: false,
            role: None,
            group: Value::Null,
            channel: place.object.get("permissions").cloned().unwrap_or_default(),
        };
        if let Some(uri) = &place.group
            && let Some(group) = self.store.social_group(&place.account, uri)?
        {
            let Standing {
                owner, role, group, ..
            } = Standing::in_group(&group);
            (standing.owner, standing.role, standing.group) = (owner, role, group);
        }
        Ok(standing)
    }
}

impl Standing {
    /// Where the account of `group` stands in the group itself.
    pub(crate) fn in_group(group: &SocialGroup) -> Self {
        Standing {
            conversation: false,
            owner: str_at(&group.group, "owner").is_some_and(|o| same_actor(o, &group.account)),
            role: group.role.clone(),
            group: group.group.get("permissions").cloned().unwrap_or_default(),
            channel: Value::Null,
        }
    }

    /// Whether the account may manage the group (§5.2 `manage`): its
    /// invites among the rest.
    pub fn may_manage(&self) -> bool {
        !self.conversation && (self.owner || self.granted(&self.group, "manage", &DEFAULT_MANAGE))
    }

    /// The kinds the account may post here as messages that answer none.
    pub fn kinds(&self) -> Vec<ContentKind> {
        if self.reply_only().is_some() {
            return Vec::new();
        }
        [ContentKind::Chat, ContentKind::Memo, ContentKind::Article]
            .into_iter()
            .filter(|&kind| self.may_post(kind))
            .collect()
    }

    /// An error unless the account may post a `kind` here, answering
    /// `parent` when given (already the message that answers none).
    pub fn check_post(
        &self,
        kind: ContentKind,
        parent: Option<&SocialMessage>,
    ) -> CommandResult<()> {
        if self.conversation {
            return match kind {
                ContentKind::Chat => Ok(()),
                ContentKind::Memo | ContentKind::Article => Err(CommandError::Invalid(
                    "A private conversation takes chat messages only.".into(),
                )),
            };
        }
        if !self.may_post(kind) {
            return Err(CommandError::Invalid(format!(
                "You can't post {} here.",
                plural(kind)
            )));
        }
        if let Some(allowed) = self.reply_only() {
            let Some(parent) = parent else {
                return Err(CommandError::Invalid(
                    "You can only reply here, not start a thread.".into(),
                ));
            };
            if let Some(to) = &allowed
                && !to.contains(&parent.kind)
            {
                let kinds: Vec<&str> = to
                    .iter()
                    .map(|t| match kind_of(t) {
                        Some(kind) => plural(kind),
                        None => t.as_str(),
                    })
                    .collect();
                return Err(CommandError::Invalid(format!(
                    "You can only reply here to {}.",
                    kinds.join(" and ")
                )));
            }
        }
        Ok(())
    }

    /// Whether the account may react here: as the channel's `react` says,
    /// or else anyone who reads the channel (§5.2.1).
    pub fn may_react(&self) -> bool {
        self.conversation
            || self.owner
            || self
                .channel
                .get("react")
                .is_none_or(|roles| self.listed(roles))
    }

    /// Whether the account may delete others' messages here (§5.2
    /// `moderate`).
    pub fn may_moderate(&self) -> bool {
        !self.conversation
            && (self.owner || self.granted(&self.group, "moderate", &DEFAULT_MODERATE))
    }

    /// Whether the account may post a `kind` at all: as the channel's
    /// `post:<type>` says, or else the group's `post` (§5.2.1).
    fn may_post(&self, kind: ContentKind) -> bool {
        if self.conversation {
            return kind == ContentKind::Chat;
        }
        if self.owner {
            return true;
        }
        match self.channel.get(format!("post:{}", type_name(kind))) {
            Some(roles) => self.listed(roles),
            None => self.granted(&self.group, "post", &DEFAULT_POST),
        }
    }

    /// When the account may post only replies here (§5.2.1 `replyOnly`),
    /// the types of the messages it may answer: `None` for any.
    fn reply_only(&self) -> Option<Option<Vec<String>>> {
        if self.conversation || self.owner {
            return None;
        }
        let restricted = self
            .channel
            .get("replyOnly")
            .is_some_and(|r| self.listed(r));
        restricted.then(|| {
            self.channel
                .get("replyOnlyTo")
                .and_then(Value::as_array)
                .map(|to| {
                    to.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
        })
    }

    /// Whether `permissions` grants `action` to the account's role, the
    /// `default` roles when it does not name the action.
    fn granted(&self, permissions: &Value, action: &str, default: &[&str]) -> bool {
        match permissions.get(action) {
            Some(roles) => self.listed(roles),
            None => self.role.as_deref().is_some_and(|r| default.contains(&r)),
        }
    }

    /// Whether the account's role is among `roles` (exact membership).
    fn listed(&self, roles: &Value) -> bool {
        let Some(role) = &self.role else {
            return false;
        };
        roles
            .as_array()
            .is_some_and(|roles| roles.iter().any(|r| r.as_str() == Some(role)))
    }
}

/// A kind named in a sentence.
fn plural(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Chat => "chat messages",
        ContentKind::Memo => "memos",
        ContentKind::Article => "articles",
    }
}
