//! The Gmail API's responses, as far as sync reads them.

use base64::Engine;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use serde::Deserialize;

use crate::{SyncError, SyncResult};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Profile {
    pub history_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LabelResource {
    pub id: String,
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: String,
}

#[derive(Deserialize)]
pub(super) struct LabelList {
    #[serde(default)]
    pub(super) labels: Vec<LabelResource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MessageRef {
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MessageList {
    #[serde(default)]
    pub messages: Vec<MessageRef>,
    pub next_page_token: Option<String>,
}

/// A message as `format=raw` returns it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawMessage {
    pub id: String,
    pub thread_id: String,
    #[serde(default)]
    pub label_ids: Vec<String>,
    #[serde(default)]
    pub snippet: String,
    /// Milliseconds since the epoch, as a string.
    pub internal_date: String,
    pub raw: String,
}

impl RawMessage {
    /// The message's bytes. Gmail documents base64url; accept it padded or
    /// not.
    pub(crate) fn bytes(&self) -> SyncResult<Vec<u8>> {
        URL_SAFE_NO_PAD
            .decode(self.raw.trim_end_matches('='))
            .or_else(|_| URL_SAFE.decode(&self.raw))
            .map_err(|e| SyncError::Mime(format!("message {}: raw is not base64url: {e}", self.id)))
    }
}

/// A message as `format=metadata` returns it: everything but the body.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MetadataMessage {
    pub id: String,
    pub thread_id: String,
    #[serde(default)]
    pub label_ids: Vec<String>,
    #[serde(default)]
    pub snippet: String,
    /// Milliseconds since the epoch, as a string.
    pub internal_date: String,
    #[serde(default)]
    pub size_estimate: u64,
    #[serde(default)]
    pub payload: MetadataPayload,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct MetadataPayload {
    #[serde(default)]
    pub headers: Vec<Header>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Header {
    pub name: String,
    pub value: String,
}

impl MetadataMessage {
    /// The headers as the top of an RFC 5322 message, ending in the blank
    /// line, for the same parser that reads whole messages. Gmail returns
    /// each value unfolded, so one header is one line.
    pub(crate) fn header_block(&self) -> Vec<u8> {
        let mut block = Vec::new();
        for h in &self.payload.headers {
            let value = h.value.replace(['\r', '\n'], " ");
            block.extend_from_slice(format!("{}: {value}\r\n", h.name).as_bytes());
        }
        block.extend_from_slice(b"\r\n");
        block
    }
}

/// A message as `format=minimal` returns it: just where it is filed.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MinimalMessage {
    pub id: String,
    #[serde(default)]
    pub label_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryMessage {
    pub message: MessageRef,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryRecord {
    #[serde(default)]
    pub messages_added: Vec<HistoryMessage>,
    #[serde(default)]
    pub messages_deleted: Vec<HistoryMessage>,
    #[serde(default)]
    pub labels_added: Vec<HistoryMessage>,
    #[serde(default)]
    pub labels_removed: Vec<HistoryMessage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryPage {
    #[serde(default)]
    pub history: Vec<HistoryRecord>,
    pub next_page_token: Option<String>,
    pub history_id: String,
}

#[derive(Deserialize)]
pub(super) struct ErrorBody {
    pub(super) error: ErrorDetail,
}

#[derive(Deserialize)]
pub(super) struct ErrorDetail {
    #[serde(default)]
    pub(super) message: String,
    #[serde(default)]
    pub(super) errors: Vec<ErrorReason>,
}

#[derive(Deserialize)]
pub(super) struct ErrorReason {
    #[serde(default)]
    pub(super) reason: String,
}
