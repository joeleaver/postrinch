//! What a group's managers do about who joins (§5.6, §5.7): make, list
//! and revoke invites, and answer join requests. These ask the group's
//! provider and keep nothing here: what the provider holds is the truth,
//! and the window asks again when it shows them.

use std::sync::Arc;

use jiff::Timestamp;
use postrinch_ofscp::Identity;
use postrinch_ofscp::http::{Client, PageRequest};
use postrinch_ofscp::ofscp_core::wire::{Invite, InviteCreateRequest};
use postrinch_ofscp::ofscp_core::{Authority, LocalId};

use super::InviteLink;
use crate::paging::next_page;
use crate::uris::{channel_uri, parse_channel_uri, parse_group_uri};
use crate::{Social, SocialError, SocialResult, parse_actor};

/// An invite to a group, as its managers see it. Its token is not here:
/// a provider gives it only when the invite is made ([`MadeInvite`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInvite {
    /// The invite's id in its group.
    pub id: String,
    /// The channel it lands in, as a place id (a channel URI).
    pub channel: Option<String>,
    /// The role it gives; `None` for the group's default, `member`.
    pub role: Option<String>,
    /// How many times it has been used.
    pub uses: u32,
    /// How many times it may be, if there is a limit.
    pub max_uses: Option<u32>,
    /// When it stops working, in milliseconds since the epoch.
    pub expires_at: Option<i64>,
    /// Who made it.
    pub created_by: String,
    /// When, in milliseconds since the epoch.
    pub created_at: i64,
}

/// What an invite to make gives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MakeInvite {
    /// The channel to land in, as a place id (a channel URI of the group).
    pub channel: Option<String>,
    /// The role to give; `None` for `member`.
    pub role: Option<String>,
    /// How many times it may be used; `None` for no limit.
    pub max_uses: Option<u32>,
    /// When it stops working, in milliseconds since the epoch; `None` for
    /// never.
    pub expires_at: Option<i64>,
}

/// An invite just made, with the link to share. The link is given now
/// and never again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MadeInvite {
    pub invite: GroupInvite,
    pub link: InviteLink,
}

/// A request to join, waiting for a manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRequest {
    /// The request's id in its group.
    pub id: String,
    /// Who asked.
    pub user: String,
    /// What they said, if anything.
    pub message: Option<String>,
    /// When they asked, in milliseconds since the epoch.
    pub requested_at: i64,
}

/// A group named by its URI, and the signed client for its provider.
struct AtGroup {
    identity: Arc<Identity>,
    client: Client,
    authority: Authority,
    id: LocalId,
}

impl Social {
    /// The invites to the group at `group` (its URI), every page of them.
    pub async fn group_invites(&self, actor: &str, group: &str) -> SocialResult<Vec<GroupInvite>> {
        let at = self.at_group(actor, group).await?;
        let mut invites = Vec::new();
        let mut page = PageRequest::newest();
        loop {
            let found = at.client.invites(&at.id, &page).await;
            let found = self.checked(&at.identity, &at.client, found).await?;
            invites.extend(found.items.iter().map(|i| group_invite(&at.authority, i)));
            match next_page(&found, &page) {
                Some(after) => page = after,
                None => return Ok(invites),
            }
        }
    }

    /// Make an invite to the group at `group` (its URI), as `invite`
    /// says. The provider refuses a role its maker does not hold all of,
    /// and one that is not the group's.
    pub async fn make_invite(
        &self,
        actor: &str,
        group: &str,
        invite: MakeInvite,
    ) -> SocialResult<MadeInvite> {
        let at = self.at_group(actor, group).await?;
        // The channel must be one of this group's, not one of the same
        // local id in another group or on another provider.
        let channel_id = match &invite.channel {
            Some(place) => Some(
                parse_channel_uri(place)
                    .filter(|c| c.authority == at.authority && c.group == at.id)
                    .ok_or_else(|| SocialError::NotAChannel(place.clone()))?
                    .channel,
            ),
            None => None,
        };
        let expires_at = invite
            .expires_at
            .map(|ms| Timestamp::from_millisecond(ms).map_err(|_| SocialError::NotATime(ms)))
            .transpose()?;
        let request = InviteCreateRequest {
            channel_id,
            role: invite.role,
            grants_guest: None,
            max_uses: invite.max_uses,
            expires_at,
        };
        let made = at.client.create_invite(&at.id, &request).await;
        let made = self.checked(&at.identity, &at.client, made).await?;
        let link = InviteLink::new(at.authority.clone(), made.token.clone());
        let invite = Invite {
            id: made.id,
            group_id: made.group_id,
            channel_id: made.channel_id,
            token: None,
            role: made.role,
            grants_guest: made.grants_guest,
            max_uses: made.max_uses,
            uses: made.uses,
            expires_at: made.expires_at,
            created_by: made.created_by,
            created_at: made.created_at,
        };
        Ok(MadeInvite {
            invite: group_invite(&at.authority, &invite),
            link,
        })
    }

    /// Revoke invite `invite` (its id) to the group at `group`: its link
    /// stops working.
    pub async fn revoke_invite(&self, actor: &str, group: &str, invite: &str) -> SocialResult<()> {
        let at = self.at_group(actor, group).await?;
        let invite = local_id(invite)?;
        let revoked = at.client.revoke_invite(&at.id, &invite).await;
        self.checked(&at.identity, &at.client, revoked).await
    }

    /// The requests to join the group at `group` waiting for a manager or
    /// moderator, every page of them.
    pub async fn join_requests(
        &self,
        actor: &str,
        group: &str,
    ) -> SocialResult<Vec<PendingRequest>> {
        let at = self.at_group(actor, group).await?;
        let mut requests = Vec::new();
        let mut page = PageRequest::newest();
        loop {
            let found = at.client.join_requests(&at.id, &page).await;
            let found = self.checked(&at.identity, &at.client, found).await?;
            requests.extend(found.items.iter().map(|r| PendingRequest {
                id: r.id.as_str().to_owned(),
                user: r.user.to_string(),
                message: r.message.clone(),
                requested_at: r.requested_at.as_millisecond(),
            }));
            match next_page(&found, &page) {
                Some(after) => page = after,
                None => return Ok(requests),
            }
        }
    }

    /// Let the person who made request `request` into the group at
    /// `group`, as a `member`. A group that now takes members by invite
    /// only refuses it.
    pub async fn approve_join_request(
        &self,
        actor: &str,
        group: &str,
        request: &str,
    ) -> SocialResult<()> {
        let at = self.at_group(actor, group).await?;
        let request = local_id(request)?;
        let approved = at.client.approve_join_request(&at.id, &request).await;
        self.checked(&at.identity, &at.client, approved)
            .await
            .map(drop)
    }

    /// Refuse request `request` to join the group at `group`.
    pub async fn deny_join_request(
        &self,
        actor: &str,
        group: &str,
        request: &str,
    ) -> SocialResult<()> {
        let at = self.at_group(actor, group).await?;
        let request = local_id(request)?;
        let denied = at.client.deny_join_request(&at.id, &request).await;
        self.checked(&at.identity, &at.client, denied).await
    }

    /// The group at `group` (its URI), for `actor` to act on.
    async fn at_group(&self, actor: &str, group: &str) -> SocialResult<AtGroup> {
        let actor = parse_actor(actor)?;
        let (authority, id) =
            parse_group_uri(group).ok_or_else(|| SocialError::NotAGroup(group.to_owned()))?;
        let account = self.account(&actor).await?;
        let identity = self.identity(&account).await?;
        let client = self.client(&identity, &authority);
        Ok(AtGroup {
            identity,
            client,
            authority,
            id,
        })
    }
}

/// An invite as its managers see it.
fn group_invite(authority: &Authority, invite: &Invite) -> GroupInvite {
    GroupInvite {
        id: invite.id.as_str().to_owned(),
        channel: invite
            .channel_id
            .as_ref()
            .map(|channel| channel_uri(authority, &invite.group_id, channel)),
        role: invite.role.clone(),
        uses: invite.uses,
        max_uses: invite.max_uses,
        expires_at: invite.expires_at.map(|at| at.as_millisecond()),
        created_by: invite.created_by.to_string(),
        created_at: invite.created_at.as_millisecond(),
    }
}

/// An id as the window gave it back.
fn local_id(id: &str) -> SocialResult<LocalId> {
    LocalId::new(id).map_err(|_| SocialError::NotAnId(id.to_owned()))
}
