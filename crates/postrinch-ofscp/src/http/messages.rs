//! Channel messages over REST: history and replies (§7.2), create, edit
//! and delete (§7.1), and reactions (§7.1 *Reactions*), on the channel's
//! host provider.
//!
//! Not here: `PUT …/reactions/{key}`, whose optional `{ unicode, image }`
//! body has no type in ofscp-core and whose answer the spec leaves
//! unspecified. Over a live connection, `reaction.add` does the same.

use ofscp_core::LocalId;
use ofscp_core::wire::{Message, MessageCreate, MessageUpdate, MessagesPage, Paged, Reaction};
use reqwest::Method;

use super::path::{PageRequest, Query, path};
use super::response::Reply;
use super::send::{Auth, Call};
use super::{Client, Provider};
use crate::{Error, Result};

/// How a provider answered a create keyed by `clientMessageId` (§7.1,
/// §7.4): either a new message, or the one a retry already made.
#[derive(Debug, Clone, PartialEq)]
pub enum Posted {
    /// `201 Created`: the new message.
    Created(Message),
    /// `200 OK`: the message an earlier send with the same
    /// `clientMessageId` made, as it now stands (perhaps edited or a
    /// tombstone). Match it to the pending send; it is not a new message.
    Existing(Message),
}

impl Posted {
    /// The message, either way.
    pub fn message(&self) -> &Message {
        match self {
            Self::Created(m) | Self::Existing(m) => m,
        }
    }

    /// The message, either way, taken.
    pub fn into_message(self) -> Message {
        match self {
            Self::Created(m) | Self::Existing(m) => m,
        }
    }

    /// The outcome a create's answer means, by its status.
    pub(super) fn from_reply(reply: &Reply) -> Result<Self> {
        match reply.status {
            201 => reply.json().map(Self::Created),
            200 => reply.json().map(Self::Existing),
            status => Err(Error::UnexpectedStatus {
                authority: reply.authority.clone(),
                status,
                expected: "201 Created or 200 OK",
            }),
        }
    }
}

impl Provider {
    /// One page of a channel's history as an anonymous viewer (§7.2,
    /// §4.5): messages and reactions, in the direction of travel.
    pub async fn messages(
        &self,
        group_id: &LocalId,
        channel_id: &LocalId,
        page: &PageRequest,
    ) -> Result<MessagesPage> {
        self.get_anonymous(messages_path(group_id, channel_id), page.query())
            .await
    }

    /// One page of the replies to a message, as an anonymous viewer (§7.2
    /// *Replies*, §4.5).
    pub async fn replies(
        &self,
        group_id: &LocalId,
        channel_id: &LocalId,
        message_id: &LocalId,
        page: &PageRequest,
    ) -> Result<MessagesPage> {
        let path = format!("{}/replies", message_path(group_id, channel_id, message_id));
        self.get_anonymous(path, page.query()).await
    }
}

impl Client {
    /// One page of a channel's history (§7.2): messages and reactions, in
    /// the direction of travel. A channel the signer cannot read is a `404`.
    pub async fn messages(
        &self,
        group_id: &LocalId,
        channel_id: &LocalId,
        page: &PageRequest,
    ) -> Result<MessagesPage> {
        self.get(messages_path(group_id, channel_id), page.query())
            .await
    }

    /// One page of the replies to a message (§7.2 *Replies*).
    pub async fn replies(
        &self,
        group_id: &LocalId,
        channel_id: &LocalId,
        message_id: &LocalId,
        page: &PageRequest,
    ) -> Result<MessagesPage> {
        let path = format!("{}/replies", message_path(group_id, channel_id, message_id));
        self.get(path, page.query()).await
    }

    /// Post a message over REST (§7.1 *Sending messages*). A retry with the
    /// same `clientMessageId` answers the first message, as [`Posted::Existing`].
    pub async fn post_message(
        &self,
        group_id: &LocalId,
        channel_id: &LocalId,
        message: &MessageCreate,
    ) -> Result<Posted> {
        let call = Call::new(Method::POST, messages_path(group_id, channel_id)).json(message)?;
        let reply = self
            .provider()
            .execute(Auth::Signed(self.identity()), call)
            .await?;
        Posted::from_reply(&reply)
    }

    /// Edit one of the signer's messages (§7.1 *Editing & deleting*);
    /// answers it with `editedAt` set.
    pub async fn edit_message(
        &self,
        group_id: &LocalId,
        channel_id: &LocalId,
        message_id: &LocalId,
        update: &MessageUpdate,
    ) -> Result<Message> {
        self.send_json(
            Method::PATCH,
            message_path(group_id, channel_id, message_id),
            update,
        )
        .await
    }

    /// Delete a message, leaving a tombstone (§7.1 *Editing & deleting*).
    /// The author or a `moderate` holder may.
    pub async fn delete_message(
        &self,
        group_id: &LocalId,
        channel_id: &LocalId,
        message_id: &LocalId,
    ) -> Result<()> {
        let path = message_path(group_id, channel_id, message_id);
        self.send_no_content(Method::DELETE, path, Query::new())
            .await
    }

    /// One page of a message's reactions (§7.1 *Reactions*).
    pub async fn reactions(
        &self,
        group_id: &LocalId,
        channel_id: &LocalId,
        message_id: &LocalId,
        page: &PageRequest,
    ) -> Result<Paged<Reaction>> {
        let path = format!(
            "{}/reactions",
            message_path(group_id, channel_id, message_id)
        );
        self.get(path, page.query()).await
    }

    /// Take back the signer's reaction `key` on a message (§7.1
    /// *Reactions*).
    pub async fn remove_reaction(
        &self,
        group_id: &LocalId,
        channel_id: &LocalId,
        message_id: &LocalId,
        key: &str,
    ) -> Result<()> {
        let path = format!(
            "{}{}",
            message_path(group_id, channel_id, message_id),
            path("/reactions/{}", &[key])
        );
        self.send_no_content(Method::DELETE, path, Query::new())
            .await
    }
}

fn messages_path(group_id: &LocalId, channel_id: &LocalId) -> String {
    path(
        "/api/groups/{}/channels/{}/messages",
        &[group_id.as_str(), channel_id.as_str()],
    )
}

fn message_path(group_id: &LocalId, channel_id: &LocalId, message_id: &LocalId) -> String {
    path(
        "/api/groups/{}/channels/{}/messages/{}",
        &[group_id.as_str(), channel_id.as_str(), message_id.as_str()],
    )
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use reqwest::header::HeaderMap;
    use serde_json::json;

    use super::*;

    fn reply(status: u16) -> Reply {
        let message = json!({
            "id": "msg_999", "clientMessageId": "cmsg_1", "author": "alice@a.com",
            "type": "message", "content": {"mime": "text/plain", "text": "hi"},
            "attachments": [], "createdAt": "2026-01-01T12:00:00Z", "metadata": []
        });
        Reply {
            authority: "a.example".to_owned(),
            status,
            headers: HeaderMap::new(),
            body: Bytes::from(message.to_string()),
        }
    }

    #[test]
    fn a_create_is_read_by_its_status() {
        let created = Posted::from_reply(&reply(201)).unwrap();
        assert!(matches!(created, Posted::Created(_)));
        assert_eq!(created.message().id.as_str(), "msg_999");
        let existing = Posted::from_reply(&reply(200)).unwrap();
        assert!(matches!(existing, Posted::Existing(_)));
        assert_eq!(existing.into_message().id.as_str(), "msg_999");
    }

    #[test]
    fn a_create_with_another_success_status_is_an_error() {
        let err = Posted::from_reply(&reply(202)).unwrap_err();
        assert!(matches!(err, Error::UnexpectedStatus { status: 202, .. }));
    }
}
