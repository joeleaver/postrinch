//! Groups, channels and members, and the access rules of §5.2, §5.2.1 and
//! §11.3.

use std::collections::BTreeMap;

use ofscp_core::Actor;
use ofscp_core::wire::{
    Channel, ChannelPermissions, Group, GroupPermissions, JoinRequest, Member, MessageType, Role,
    Tier,
};

use super::timeline::Timeline;

pub(crate) const OWNER: &str = "owner";

#[derive(Debug)]
pub(crate) struct GroupRec {
    /// The group as served, without the caller's `membership`.
    pub(crate) group: Group,
    pub(crate) members: BTreeMap<Actor, Member>,
    /// Pending requests to join (§5.7, `request` policy); approving one
    /// is a seeding call, `FakeProvider::add_member`.
    pub(crate) requests: Vec<JoinRequest>,
}

#[derive(Debug)]
pub(crate) struct ChannelRec {
    pub(crate) channel: Channel,
    pub(crate) timeline: Timeline,
}

/// The group permission map with the spec's defaults filled in, as a
/// provider returns it (§5.2): `post: [admin, member]`, `moderate` and
/// `manage: [admin]`, `members.set-nickname` owner-only.
pub(crate) fn effective_permissions(given: Option<GroupPermissions>) -> GroupPermissions {
    let mut p = given.unwrap_or_default();
    let roles = |list: &[&str]| Some(list.iter().map(|r| r.to_string()).collect());
    p.post = p.post.or_else(|| roles(&["admin", "member"]));
    p.moderate = p.moderate.or_else(|| roles(&["admin"]));
    p.manage = p.manage.or_else(|| roles(&["admin"]));
    p
}

/// Readable by anyone, signed or not (§11)? Unknown tiers count as
/// `group`.
fn open_tier(tier: &Tier) -> bool {
    matches!(tier, Tier::Public | Tier::Discoverable)
}

/// Does `role` hold the group action `list` grants? The owner holds every
/// permission; unknown roles hold nothing (§5.2).
fn role_in(role: Option<&Role>, list: Option<&Vec<Role>>) -> bool {
    match role {
        Some(r) if r == OWNER => true,
        Some(r) => list.is_some_and(|l| l.contains(r)),
        None => false,
    }
}

impl GroupRec {
    pub(crate) fn role_of(&self, actor: Option<&Actor>) -> Option<&Role> {
        actor.and_then(|a| self.members.get(a)).map(|m| &m.role)
    }

    pub(crate) fn is_member(&self, actor: Option<&Actor>) -> bool {
        self.role_of(actor).is_some()
    }

    /// Can `viewer` (`None`: anonymous) read the group itself?
    pub(crate) fn can_view(&self, viewer: Option<&Actor>) -> bool {
        open_tier(&self.group.tier) || self.is_member(viewer)
    }

    /// Effective read access to a channel (§11.3): the group's tier allows
    /// AND the channel's tier allows AND `view` is absent or lists the
    /// role. A channel the viewer cannot read is answered as missing.
    pub(crate) fn can_read(&self, channel: &Channel, viewer: Option<&Actor>) -> bool {
        let member = self.is_member(viewer);
        let tiers = (open_tier(&self.group.tier) || member) && (open_tier(&channel.tier) || member);
        let view = channel.permissions.as_ref().and_then(|p| p.view.as_ref());
        tiers && (view.is_none() || role_in(self.role_of(viewer), view))
    }

    /// May `actor` post a message of `kind` here (§5.2.1)? The channel's
    /// `post:<type>` list wins over the group's `post`.
    pub(crate) fn can_post(&self, channel: &Channel, actor: &Actor, kind: &MessageType) -> bool {
        let per_type = channel.permissions.as_ref().and_then(|p| match kind {
            MessageType::Memo => p.post_memo.as_ref(),
            MessageType::Article => p.post_article.as_ref(),
            _ => p.post_message.as_ref(),
        });
        let list = per_type.or(self.group.permissions.post.as_ref());
        role_in(self.role_of(Some(actor)), list)
    }

    /// Is `actor` restricted to replies here (`replyOnly`, §5.2.1)?
    pub(crate) fn reply_only(&self, channel: &Channel, actor: &Actor) -> bool {
        let list = channel
            .permissions
            .as_ref()
            .and_then(|p| p.reply_only.as_ref());
        let role = self.role_of(Some(actor));
        role.is_some_and(|r| r != OWNER) && role_in(role, list)
    }

    /// May `actor` react here? Absent `react` means anyone who can post.
    pub(crate) fn can_react(&self, channel: &Channel, actor: &Actor) -> bool {
        match channel.permissions.as_ref().and_then(|p| p.react.as_ref()) {
            Some(list) => role_in(self.role_of(Some(actor)), Some(list)),
            None => self.is_member(Some(actor)),
        }
    }

    pub(crate) fn can_moderate(&self, actor: &Actor) -> bool {
        role_in(
            self.role_of(Some(actor)),
            self.group.permissions.moderate.as_ref(),
        )
    }
}

/// The types `replyOnlyTo` lets a reply-restricted actor answer.
pub(crate) fn reply_only_to(channel: &Channel) -> Option<&Vec<MessageType>> {
    channel
        .permissions
        .as_ref()
        .and_then(|p: &ChannelPermissions| p.reply_only_to.as_ref())
}
