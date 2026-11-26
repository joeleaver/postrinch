//! Private conversations between two people (spec §7.4, §8.3).

use serde::{Deserialize, Serialize};

use crate::{Actor, DmId};

use super::{Attachment, Content, Message, MessageReference, MetadataObject, Paged, Timestamp};

/// A private conversation, as `GET /api/me/dms` lists it (§7.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmConversation {
    /// Its id, derived from the two participants.
    pub id: DmId,
    /// The two people in it.
    pub participants: [Actor; 2],
    /// The newest message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message: Option<Message>,
    /// When it last changed.
    pub updated_at: Timestamp,
    /// Extension data.
    #[serde(default)]
    pub metadata: Vec<MetadataObject>,
}

/// `GET /api/me/dms`: one page of your conversations (§7.4).
pub type DmConversationsResponse = Paged<DmConversation>;

/// A private message, sent to the *recipient's* provider:
/// `POST /api/federation/dms/{dmId}/messages` (§7.4, §8.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmMessageCreate {
    /// The client's own id for it.
    pub client_message_id: String,
    /// The recipient. Clients should name them (§7.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Actor>,
    /// What it says.
    pub content: Content,
    /// Files attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<Attachment>>,
    /// The message it replies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<MessageReference>,
}

/// `PUT /api/me/dms/{dmId}`: a pointer at home to a conversation whose
/// messages live on the other person's provider (§7.4, "Conversation
/// pointers").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPointer {
    /// The other person.
    pub with: Actor,
    /// The message just sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message: Option<Message>,
}
