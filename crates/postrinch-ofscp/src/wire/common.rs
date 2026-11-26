//! Shapes shared across the payloads: metadata, content, attachments,
//! paging, tiers, and the key algorithm (`schemas/v0.1/defs`).

use serde::{Deserialize, Serialize};

use crate::LocalId;

use super::open_enum;

/// An extension object (§5.4). Consumers ignore schemas they do not know.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetadataObject {
    /// The HTTPS URI naming the extension's schema.
    pub schema: String,
    /// The schema's version.
    pub version: String,
    /// The payload, validated by the schema, not here.
    pub data: serde_json::Value,
}

/// A body of text and its media type (`text/plain`, `text/markdown`, HTML).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    /// The text.
    pub text: String,
    /// Its IANA media type.
    pub mime: String,
}

/// A file uploaded to `/api/media` and attached to a message (§5.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// The attachment's local id.
    pub id: LocalId,
    /// Its IANA media type.
    pub mime: String,
    /// Where to fetch it (HTTPS).
    pub url: String,
    /// Its size in bytes.
    pub size: u64,
    /// The original file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// A content hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// Width in pixels, for pictures and video.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Height in pixels, for pictures and video.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Length in milliseconds, for audio and video.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// The cursors of one page of a list (`PagedResponse.page`). Cursors are
/// opaque (§2.5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    /// The cursor for the next (older) page, if there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// The cursor for the previous (newer) page, if there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_cursor: Option<String>,
}

/// One page of a list (`PagedResponse`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paged<T> {
    /// The items on this page.
    pub items: Vec<T>,
    /// Where the neighbouring pages are.
    pub page: Page,
}

open_enum! {
    /// Who may see a group or channel (§11). Every provider has `private`;
    /// a provider may define more, listed by `GET /api/tiers`.
    pub enum Tier {
        /// Members only.
        Private = "private",
        /// Members of the group.
        Group = "group",
        /// Anyone, including anonymous readers.
        Public = "public",
        /// Public, and listed for discovery.
        Discoverable = "discoverable",
    }
}

open_enum! {
    /// A device key's algorithm. v0.1 has only Ed25519 (§4.3.3).
    pub enum KeyAlgorithm {
        /// Ed25519.
        Ed25519 = "Ed25519",
    }
}
