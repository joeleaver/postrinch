//! Joining groups (§5.6, §5.7; docs/ofscp.md "Joining"): by an invite
//! link, which may name a group on any provider, or by asking a group
//! that takes members openly or by request. Its managers make and revoke
//! invites and answer requests ([`manage`]).
//!
//! A group joined is kept here at once, as the person's, and the account
//! synced. A group elsewhere is listed at home by a pointer (§5.7 *My
//! groups*); the sync writes it, as it writes again any pointer home is
//! missing for a group kept here, so a pointer that does not go now goes
//! on a later sync and the group is not lost. A group asked to join is
//! kept as asked, and a sync that finds the person let in keeps it as
//! theirs.

mod link;
mod manage;

use std::sync::Arc;

use postrinch_ofscp::http::{Client, JoinOutcome};
use postrinch_ofscp::ofscp_core::LocalId;
use postrinch_ofscp::{Error, Identity};
use postrinch_store::{GroupFound, SocialAccount, SocialGroup, social_account_key};

use crate::live::{Control, LiveEvent};
use crate::sync::Pages;
use crate::uris::{channel_uri, group_uri, parse_group_uri};
use crate::{Social, SocialError, SocialResult, parse_actor};

pub use link::InviteLink;
pub use manage::{GroupInvite, MadeInvite, MakeInvite, PendingRequest};

/// A group joined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joined {
    /// The account that joined.
    pub account: String,
    /// The group's URI.
    pub group: String,
    /// The channel the invite lands in, as a place id (a channel URI),
    /// when it names one that is still there.
    pub channel: Option<String>,
    /// The person's role in the group.
    pub role: String,
}

/// How asking to join a group went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinAnswer {
    /// The group takes members openly, or the person was in it already.
    Joined(Joined),
    /// The group takes requests: this one waits for its managers.
    Asked { account: String, group: String },
}

impl Social {
    /// Join the group `link` is an invite to, on whichever provider holds
    /// it. An invite that has expired, was revoked or is used up is
    /// refused with the reason; the person already in the group stays in
    /// it as they are.
    pub async fn redeem_invite(&self, actor: &str, link: &InviteLink) -> SocialResult<Joined> {
        let actor = parse_actor(actor)?;
        let held = self.hold(&actor).await;
        let account = self.account(&actor).await?;
        let identity = self.identity(&account).await?;
        let client = self.client(&identity, link.authority());
        let redeemed = match client.redeem_invite(link.token()).await {
            Err(error) if status(&error) == Some(404) => return Err(SocialError::InviteNotValid),
            Err(error) if status(&error) == Some(409) => return Err(SocialError::InviteUsedUp),
            result => self.checked(&identity, &client, result).await?,
        };
        let group_id = redeemed.group_id;
        self.joined(&account, &identity, &client, &group_id, &redeemed.role)
            .await?;
        drop(held);
        self.groups_changed(&account.actor);
        Ok(Joined {
            account: account.actor,
            group: group_uri(link.authority(), &group_id),
            channel: redeemed
                .channel_id
                .map(|channel| channel_uri(link.authority(), &group_id, &channel)),
            role: redeemed.role,
        })
    }

    /// Ask to join the group at `group` (its URI). One that takes members
    /// openly lets the person in; one that takes requests keeps this one
    /// for its managers (asking again is the same request); one that takes
    /// members by invite only is refused as such.
    pub async fn join_group(&self, actor: &str, group: &str) -> SocialResult<JoinAnswer> {
        let actor = parse_actor(actor)?;
        let (authority, group_id) =
            parse_group_uri(group).ok_or_else(|| SocialError::NotAGroup(group.to_owned()))?;
        let held = self.hold(&actor).await;
        let account = self.account(&actor).await?;
        let identity = self.identity(&account).await?;
        let client = self.client(&identity, &authority);
        let uri = group_uri(&authority, &group_id);
        let outcome = match client.join(&group_id).await {
            Err(error) if status(&error) == Some(403) => {
                return Err(SocialError::InviteOnly { group: uri });
            }
            result => self.checked(&identity, &client, result).await?,
        };
        let answer = match outcome {
            JoinOutcome::Joined(member) | JoinOutcome::AlreadyMember(member) => {
                self.joined(&account, &identity, &client, &group_id, &member.role)
                    .await?;
                JoinAnswer::Joined(Joined {
                    account: account.actor.clone(),
                    group: uri,
                    channel: None,
                    role: member.role,
                })
            }
            JoinOutcome::Requested(_) => {
                self.keep_known(&identity, &client, &group_id, GroupFound::Asked, None)
                    .await?;
                JoinAnswer::Asked {
                    account: account.actor.clone(),
                    group: uri,
                }
            }
        };
        drop(held);
        self.groups_changed(&account.actor);
        Ok(answer)
    }

    /// The person is in `group_id` at `client`'s provider as `role`: keep
    /// the group as theirs, then sync, which lists it at home and keeps
    /// its channels. A sync that fails here leaves the group kept, for the
    /// next sync to finish; the account's own trouble (it must sign in
    /// again) is the person's to hear of now.
    async fn joined(
        &self,
        account: &SocialAccount,
        identity: &Arc<Identity>,
        client: &Client,
        group_id: &LocalId,
        role: &str,
    ) -> SocialResult<()> {
        self.keep_known(identity, client, group_id, GroupFound::Member, Some(role))
            .await?;
        match self.sync_as(account, identity, Pages::Unresumable).await {
            Ok(_) => Ok(()),
            Err(error @ SocialError::Unusable { .. }) => Err(error),
            Err(error) => {
                tracing::warn!(account = %account.actor, %error, "sync after joining failed; the next sync finishes it");
                Ok(())
            }
        }
    }

    /// Keep group `group_id` at `client`'s provider as `found` says the
    /// person knows it, keeping its notices. The person is in the group
    /// (or asked) already, so a failed read of it does not lose that: the
    /// group is kept with what is known of it, which is nothing new, and
    /// the next sync reads it (`sync/outside.rs`). Only the account's own
    /// trouble is the person's to hear of now.
    async fn keep_known(
        &self,
        identity: &Arc<Identity>,
        client: &Client,
        group_id: &LocalId,
        found: GroupFound,
        role: Option<&str>,
    ) -> SocialResult<()> {
        let read = self
            .checked(identity, client, client.group(group_id).await)
            .await;
        let authority = client.provider().endpoint().authority().clone();
        let uri = group_uri(&authority, group_id);
        let read_group = read
            .as_ref()
            .ok()
            .map(|g| serde_json::to_value(g).expect("a wire object always serializes"));
        let record = SocialGroup {
            account: identity.actor().to_string(),
            uri: uri.clone(),
            provider: authority.to_string(),
            group: serde_json::Value::Null,
            role: role.map(str::to_owned),
            found,
            notices: None,
        };
        self.blocking(move |store| {
            let kept = store.social_group(&record.account, &record.uri)?;
            let notices = kept.as_ref().and_then(|g| g.notices);
            let group = read_group
                .or_else(|| kept.map(|g| g.group))
                .unwrap_or_else(|| serde_json::json!({}));
            store.put_social_group(&SocialGroup {
                notices,
                group,
                ..record
            })
        })
        .await?;
        match read {
            Ok(_) => Ok(()),
            Err(error @ SocialError::Unusable { .. }) => Err(error),
            Err(error) => {
                tracing::warn!(group = %uri, %error, "group not read after joining; the next sync reads it");
                Ok(())
            }
        }
    }

    /// Tell the window the account's groups changed, and have a live
    /// account subscribe what it now holds.
    fn groups_changed(&self, account: &str) {
        self.live.tell(LiveEvent::Groups {
            account: account.to_owned(),
        });
        self.live
            .control(&social_account_key(account), Control::Refreshed);
    }
}

/// The HTTP status a provider refused a request with.
fn status(error: &Error) -> Option<u16> {
    error.problem().map(|p| p.status)
}
