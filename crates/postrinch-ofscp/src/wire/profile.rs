//! Users: profiles, the account, presence, privacy settings, contacts and
//! group lists (spec §5.1, §6).

use serde::{Deserialize, Deserializer, Serialize};

use crate::{Actor, Authority};

use super::{MetadataObject, Timestamp, open_enum};

/// A user's public profile (§5.1.1), also the answer to
/// `GET /api/users/{userRef}/profile` (§6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserProfile {
    /// The user's object URI, `https://{domain}/api/users/{handle}`.
    pub id: String,
    /// The handle.
    pub handle: String,
    /// The home provider.
    pub domain: Authority,
    /// The name to show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The avatar picture's URL (HTTPS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    /// A short self-description (§6.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    /// Whether this is a provider-local guest (§4.8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest: Option<bool>,
    /// When a guest account expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    /// When the profile last changed.
    pub updated_at: Timestamp,
    /// Extension data.
    #[serde(default)]
    pub metadata: Vec<MetadataObject>,
}

/// `GET /api/me`: your own account, never federated (§5.1.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserAccount {
    /// Your profile.
    pub profile: UserProfile,
    /// Provider-defined settings.
    pub settings: serde_json::Map<String, serde_json::Value>,
}

/// `PATCH /api/me/profile` (§6.3). Fields left `None` are unchanged.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileUpdate {
    /// A new display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// A new avatar URL, or `Some(None)` to remove the avatar.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub avatar: Option<Option<String>>,
    /// A new bio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    /// New extension data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Vec<MetadataObject>>,
}

/// A field that is present, even as `null`, is `Some`.
fn present<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(d).map(Some)
}

open_enum! {
    /// A user's availability (§6.4).
    pub enum Availability {
        /// Here.
        Online = "online",
        /// Away.
        Away = "away",
        /// Do not disturb.
        Dnd = "dnd",
        /// Not connected.
        Offline = "offline",
    }
}

/// A user's presence (§6.4), from `GET /api/users/{userRef}/presence`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Presence {
    /// Whether they are here.
    pub availability: Availability,
    /// A status line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// When they were last connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<Timestamp>,
    /// Extension data.
    #[serde(default)]
    pub metadata: Vec<MetadataObject>,
}

/// The availability you may set yourself: `offline` is derived from your
/// connections, never set (§7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SetAvailability {
    /// Here.
    Online,
    /// Away.
    Away,
    /// Do not disturb.
    Dnd,
}

/// `PUT /api/me/presence`, and the `presence.set` command (§6.4, §7.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresenceUpdate {
    /// Your availability.
    pub availability: SetAvailability,
    /// A status line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Extension data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Vec<MetadataObject>>,
}

open_enum! {
    /// Who may see something of yours (§6.1).
    pub enum VisibilityPolicy {
        /// Anyone.
        Public = "public",
        /// Any signed-in user, on any provider.
        Authenticated = "authenticated",
        /// People in a group with you.
        SharedGroups = "sharedGroups",
        /// Your contacts.
        Contacts = "contacts",
        /// Nobody.
        Nobody = "nobody",
    }
}

/// `GET /api/me/privacy` (§6.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivacySettings {
    /// Who sees your presence.
    pub presence_visibility: VisibilityPolicy,
    /// Who sees your profile.
    pub profile_visibility: VisibilityPolicy,
    /// Who sees your group memberships.
    pub membership_visibility: VisibilityPolicy,
    /// People who always may.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_list: Option<Vec<Actor>>,
    /// People who never may.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny_list: Option<Vec<Actor>>,
    /// Extension data.
    #[serde(default)]
    pub metadata: Vec<MetadataObject>,
}

/// `PUT /api/me/privacy` (§6.6). Fields left `None` are unchanged.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivacySettingsUpdate {
    /// Who sees your presence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_visibility: Option<VisibilityPolicy>,
    /// Who sees your profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_visibility: Option<VisibilityPolicy>,
    /// Who sees your group memberships.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub membership_visibility: Option<VisibilityPolicy>,
    /// People who always may.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_list: Option<Vec<Actor>>,
    /// People who never may.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny_list: Option<Vec<Actor>>,
    /// Extension data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Vec<MetadataObject>>,
}

/// `GET /api/users/{userRef}/groups`: the groups a user is in that you may
/// see (§6.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserGroupsResponse {
    /// The groups, by object URI.
    pub groups: Vec<UserGroup>,
    /// Extension data.
    #[serde(default)]
    pub metadata: Vec<MetadataObject>,
}

/// One group in a user's list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserGroup {
    /// The group's object URI.
    pub id: String,
}

open_enum! {
    /// Where a contact stands (§6.7).
    pub enum ContactState {
        /// Asked, not yet answered.
        Pending = "pending",
        /// Both agreed.
        Accepted = "accepted",
    }
}

open_enum! {
    /// Who asked, for a pending contact (§6.7).
    pub enum ContactDirection {
        /// You asked them.
        Outgoing = "outgoing",
        /// They asked you.
        Incoming = "incoming",
    }
}

/// One of your contacts (§6.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    /// Who.
    pub user: Actor,
    /// Where it stands.
    pub state: ContactState,
    /// Who asked, while pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<ContactDirection>,
    /// When it began.
    pub created_at: Timestamp,
    /// When it last changed.
    pub updated_at: Timestamp,
    /// Extension data.
    #[serde(default)]
    pub metadata: Vec<MetadataObject>,
}

/// `GET /api/me/contacts` (§6.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContactsResponse {
    /// Your contacts.
    pub contacts: Vec<Contact>,
    /// Extension data.
    #[serde(default)]
    pub metadata: Vec<MetadataObject>,
}

/// `POST /api/me/contacts` (§6.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactCreateRequest {
    /// Who to ask.
    pub user: Actor,
}

open_enum! {
    /// What a federated contact event does (§6.7).
    pub enum ContactAction {
        /// Ask.
        Request = "request",
        /// Agree to an outgoing request.
        Accept = "accept",
        /// End it.
        Remove = "remove",
    }
}

/// `POST /api/federation/contacts`, sent by a client to the other person's
/// provider (§6.7, "Federation").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactEvent {
    /// What it does.
    pub action: ContactAction,
    /// Who sends it.
    pub from: Actor,
    /// Who it is for.
    pub to: Actor,
}
