//! Invites, members and join requests (spec §5.6, §5.7).

use serde::{Deserialize, Serialize};

use crate::{Actor, LocalId};

use super::{Role, Timestamp, open_enum};

/// A link that lets people join a group (§5.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invite {
    /// The invite's local id.
    pub id: LocalId,
    /// The group it is for.
    pub group_id: LocalId,
    /// The channel to land in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<LocalId>,
    /// The secret token in the link.
    pub token: String,
    /// The role it gives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// Whether someone without an account may use it as a guest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grants_guest: Option<bool>,
    /// How many times it may be used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<u32>,
    /// How many times it has been used.
    pub uses: u32,
    /// When it stops working.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    /// Who made it.
    pub created_by: Actor,
    /// When.
    pub created_at: Timestamp,
}

/// `POST /api/groups/{groupId}/invites` (§5.6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteCreateRequest {
    /// The channel to land in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<LocalId>,
    /// The role it gives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// Whether someone without an account may use it as a guest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grants_guest: Option<bool>,
    /// How many times it may be used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<u32>,
    /// When it stops working.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
}

/// A member of a group (§5.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    /// Who.
    pub user: Actor,
    /// Their role.
    pub role: Role,
    /// Their nickname in this group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name_override: Option<String>,
    /// When they joined.
    pub joined_at: Timestamp,
}

open_enum! {
    /// Where a join request stands (§5.7).
    pub enum JoinRequestState {
        /// Waiting for a manager.
        Pending = "pending",
        /// Let in.
        Approved = "approved",
        /// Turned down.
        Denied = "denied",
    }
}

/// A request to join a group whose policy is `request` (§5.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequest {
    /// The request's local id.
    pub id: LocalId,
    /// The group.
    pub group_id: LocalId,
    /// Who asked.
    pub user: Actor,
    /// Where it stands.
    pub state: JoinRequestState,
    /// What they said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// When they asked.
    pub requested_at: Timestamp,
}
