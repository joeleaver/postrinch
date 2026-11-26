//! Messages, reactions and timelines (spec §5.3, §7.1, §7.2).

use serde::{Deserialize, Serialize};

use crate::{Actor, LocalId};

use super::{Attachment, Content, MetadataObject, Paged, Timestamp, open_enum};

open_enum! {
    /// What a message is (§5.3). An unknown type is shown as a plain
    /// message, as well as its content allows (§2.3).
    pub enum MessageType {
        /// A chat line.
        Message = "message",
        /// A short post.
        Memo = "memo",
        /// A long post.
        Article = "article",
        /// A reaction, in discovery's `messageTypes` (§3.1).
        Reaction = "reaction",
    }
}

open_enum! {
    /// What a reference points at (§5.3).
    pub enum ReferenceKind {
        /// The message this one replies to.
        Reply = "reply",
        /// The message a reaction is on.
        Message = "message",
    }
}

/// A pointer from one message or reaction to another message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageReference {
    /// What kind of pointer.
    #[serde(rename = "type")]
    pub kind: ReferenceKind,
    /// The message's local id.
    pub id: LocalId,
}

/// What the reader may do with a message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePermissions {
    /// Until when its author may edit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_until: Option<Timestamp>,
}

/// A message in a channel or a private conversation (§5.3, `BaseMessage`).
/// A deleted one is a tombstone with `deleted_at` set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// The message's local id.
    pub id: LocalId,
    /// The id its author's client gave it, for matching the echo of a send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_message_id: Option<String>,
    /// Who wrote it.
    pub author: Actor,
    /// What it is.
    #[serde(rename = "type")]
    pub kind: MessageType,
    /// What it says.
    pub content: Content,
    /// Files attached to it.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// The message it replies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<MessageReference>,
    /// Tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// How many replies it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_count: Option<u64>,
    /// When it was sent.
    pub created_at: Timestamp,
    /// When it was last edited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<Timestamp>,
    /// When it was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<Timestamp>,
    /// What the reader may do with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<MessagePermissions>,
    /// Extension data.
    #[serde(default)]
    pub metadata: Vec<MetadataObject>,
}

/// A new channel message: `POST .../messages`, or the `message.create`
/// command (§7.1, `MessageCreate`). Sending again with the same
/// `client_message_id` is not an error; the provider answers with the first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageCreate {
    /// The client's own id for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_message_id: Option<String>,
    /// What it is (a plain message when left out).
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<MessageType>,
    /// What it says.
    pub content: Content,
    /// Files attached, uploaded first to `/api/media`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<Attachment>>,
    /// The message it replies to (kind `reply`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<MessageReference>,
    /// Tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Extension data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Vec<MetadataObject>>,
}

/// `PATCH .../messages/{messageId}`: an edit (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageUpdate {
    /// The new content.
    pub content: Content,
}

/// A reaction on a message (§5.3). The provider keeps no counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reaction {
    /// The reaction's local id.
    pub id: LocalId,
    /// Who reacted.
    pub author: Actor,
    /// Which reaction, such as `heart`.
    pub key: String,
    /// Its emoji.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unicode: Option<String>,
    /// A picture of it (HTTPS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// The message it is on (kind `message`).
    pub reference: MessageReference,
    /// When.
    pub created_at: Timestamp,
    /// Extension data.
    #[serde(default)]
    pub metadata: Vec<MetadataObject>,
}

/// One item of a channel's history: a message or a reaction (§7.2).
///
/// Reactions carry no type tag, so the two are told apart by their fields:
/// a message has `content`, a reaction has `key` (Postrinch's spec question
/// 31).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TimelineItem {
    /// A message.
    Message(Box<Message>),
    /// A reaction.
    Reaction(Box<Reaction>),
}

/// `GET .../messages` and `.../replies`: one page of history (§7.2).
pub type MessagesPage = Paged<TimelineItem>;
