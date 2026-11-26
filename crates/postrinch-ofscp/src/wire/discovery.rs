//! Provider discovery (spec §3.1), known providers (§8.6), tiers (§11) and
//! the discovery feed (§8.4).

use serde::{Deserialize, Serialize};

use crate::{Actor, Authority, ObjectRef};

use super::{
    Content, KeyAlgorithm, MessageType, MetadataObject, Paged, Tier, Timestamp, open_enum,
};

/// `GET https://{domain}/.well-known/ofscp-provider` (§3.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderDiscovery {
    /// Who the provider is and how to sign in.
    pub provider: ProviderInfo,
    /// What it supports.
    pub capabilities: Capabilities,
    /// Endpoint overrides, by name. The standard base path is `/api` (§3.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoints: Option<serde_json::Map<String, serde_json::Value>>,
}

/// The `provider` block of discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    /// The provider's authority.
    pub domain: Authority,
    /// The protocol version it speaks (SemVer, §2.1).
    pub protocol_version: String,
    /// The server software.
    pub software: Software,
    /// How to reach its operator (often a `mailto:`).
    pub contact: String,
    /// Where to sign in and register.
    pub authentication: AuthenticationInfo,
    /// The provider's own signing keys (§8.1).
    pub public_keys: Vec<ProviderKey>,
}

/// Server software name and version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Software {
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: String,
}

/// Sign-in endpoints and the registration mode (§4.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthenticationInfo {
    /// The login endpoint (HTTPS).
    pub login_endpoint: String,
    /// The registration endpoint (HTTPS), if registration is offered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_endpoint: Option<String>,
    /// Who may register.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration: Option<RegistrationMode>,
}

open_enum! {
    /// Who may register on a provider (§4.1.1).
    pub enum RegistrationMode {
        /// Anyone.
        Open = "open",
        /// Only with an account invite.
        Invite = "invite",
        /// Nobody, by themselves.
        Closed = "closed",
    }
}

/// One of the provider's own signing keys (§8.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderKey {
    /// The key's id.
    pub key_id: String,
    /// Its algorithm.
    pub algorithm: KeyAlgorithm,
    /// The public key, base64.
    pub public_key: String,
    /// When it was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<Timestamp>,
}

/// The `capabilities` block of discovery.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// The message types it accepts (`reaction` among them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_types: Option<Vec<MessageType>>,
    /// The tiers it offers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiers: Option<Vec<Tier>>,
    /// The metadata extensions it knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_schemas: Option<Vec<MetadataSchema>>,
    /// Size limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<Limits>,
    /// How it federates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub federation: Option<FederationCapabilities>,
    /// What it shares for discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery: Option<DiscoveryCapabilities>,
}

/// A metadata extension a provider knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataSchema {
    /// Its short id.
    pub id: String,
    /// Its schema's URI.
    pub uri: String,
}

/// A provider's size limits.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    /// The largest upload it takes, in bytes (§5.8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_upload_bytes: Option<u64>,
}

/// How a provider federates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FederationCapabilities {
    /// How remote members get live channel events (§8.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realtime_delivery: Option<RealtimeDelivery>,
}

open_enum! {
    /// How remote members get live channel events (§8.5).
    pub enum RealtimeDelivery {
        /// They connect straight to this provider's socket.
        DirectWs = "direct-ws",
        /// Not offered.
        None = "none",
    }
}

/// What a provider shares for discovery (§8.4, §8.6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryCapabilities {
    /// Whether it serves its known-provider list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shares_known_providers: Option<bool>,
    /// Whether it serves a discovery feed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discover_feed: Option<bool>,
}

/// `GET /api/providers`: the providers this one knows (§8.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProvidersResponse {
    /// The known providers.
    pub providers: Vec<KnownProvider>,
    /// Extension data.
    #[serde(default)]
    pub metadata: Vec<MetadataObject>,
}

/// A provider another provider knows of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownProvider {
    /// Its authority.
    pub domain: Authority,
    /// Its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// When it was added to the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at: Option<Timestamp>,
}

/// `GET /api/tiers` (§11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TiersResponse {
    /// The tiers this provider offers.
    pub tiers: Vec<TierInfo>,
}

/// One tier, described for people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierInfo {
    /// The tier's value.
    pub id: Tier,
    /// Its name.
    pub name: String,
    /// What it means.
    pub description: String,
}

/// `GET /api/discover`: one page of discoverable channels (§8.4).
pub type DiscoverResponse = Paged<DiscoverItem>;

/// A discoverable channel, perhaps with a recent message as a sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverItem {
    /// The channel (usually an object URI).
    pub channel: ObjectRef,
    /// Its group (usually an object URI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<ObjectRef>,
    /// The provider that hosts it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Authority>,
    /// A recent message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<DiscoverSample>,
    /// Extension data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Vec<MetadataObject>>,
}

/// A recent message in a discoverable channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverSample {
    /// The message's id on the channel's provider.
    pub id: String,
    /// Who wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<Actor>,
    /// When.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<Timestamp>,
    /// What it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Content>,
}
