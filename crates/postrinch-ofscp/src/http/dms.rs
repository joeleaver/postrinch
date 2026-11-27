//! Private conversations (§7.4, §8.3).
//!
//! A message lives only in its recipient's inbox, so the provider a call
//! goes to depends on whose message it is: sending, and editing, deleting
//! or reacting to a message, go to the provider that holds it (for a
//! message the signer sent, the recipient's home); the conversation list
//! and pointers are at the signer's home; history is read from both
//! participants' providers and merged (§7.4 *Reading a whole
//! conversation*). The caller picks the [`Client`].
//!
//! Not here: `PUT /api/dms/{dmId}/messages/{messageId}/reactions/{key}`,
//! whose optional `{ unicode, image }` body has no type in ofscp-core and
//! whose answer the spec leaves unspecified.

use ofscp_core::wire::{
    ConversationPointer, DmConversation, DmConversationsResponse, DmMessageCreate, Message,
    MessageUpdate, MessagesPage, Paged, Reaction,
};
use ofscp_core::{DmId, LocalId};
use reqwest::Method;

use super::Client;
use super::messages::Posted;
use super::path::{PageRequest, Query, path};
use super::send::{Auth, Call};
use crate::Result;

impl Client {
    /// Send a private message, signed straight to the recipient's home
    /// provider (§7.4 *Sending*, §8.3). A retry with the same
    /// `clientMessageId` answers the first, as [`Posted::Existing`].
    pub async fn send_dm(&self, dm_id: &DmId, message: &DmMessageCreate) -> Result<Posted> {
        let path = path("/api/federation/dms/{}/messages", &[dm_id.as_str()]);
        let call = Call::new(Method::POST, path).json(message)?;
        let reply = self
            .provider()
            .execute(Auth::Signed(self.identity()), call)
            .await?;
        Posted::from_reply(&reply)
    }

    /// Record a conversation at the signer's home after sending to another
    /// provider (§7.4 *Conversation pointers*). Idempotent.
    pub async fn put_conversation_pointer(
        &self,
        dm_id: &DmId,
        pointer: &ConversationPointer,
    ) -> Result<DmConversation> {
        self.require_home()?;
        self.send_json(
            Method::PUT,
            path("/api/me/dms/{}", &[dm_id.as_str()]),
            pointer,
        )
        .await
    }

    /// One page of the signer's conversations at home, newest activity
    /// first (§7.4 *Listing & reading*).
    pub async fn conversations(&self, page: &PageRequest) -> Result<DmConversationsResponse> {
        self.require_home()?;
        self.get("/api/me/dms".to_owned(), page.query()).await
    }

    /// One page of a conversation as this provider holds it for the signer
    /// (§7.4 *Listing & reading*): what they received here, and what they
    /// sent that sits in the other person's inbox here.
    pub async fn dm_messages(&self, dm_id: &DmId, page: &PageRequest) -> Result<MessagesPage> {
        self.get(dm_messages_path(dm_id), page.query()).await
    }

    /// One page of the replies to a private message, as this provider holds
    /// them for the signer (§7.4 *Reactions, replies & typing*).
    pub async fn dm_replies(
        &self,
        dm_id: &DmId,
        message_id: &LocalId,
        page: &PageRequest,
    ) -> Result<MessagesPage> {
        let path = format!("{}/replies", dm_message_path(dm_id, message_id));
        self.get(path, page.query()).await
    }

    /// One page of a private message's reactions, at the provider holding
    /// the message (§7.4 *Reactions*, optional for providers).
    pub async fn dm_reactions(
        &self,
        dm_id: &DmId,
        message_id: &LocalId,
        page: &PageRequest,
    ) -> Result<Paged<Reaction>> {
        let path = format!("{}/reactions", dm_message_path(dm_id, message_id));
        self.get(path, page.query()).await
    }

    /// Take back the signer's reaction `key` on a private message, at the
    /// provider holding the message (§7.4 *Reactions*). §8.3 gives
    /// `/api/federation/dms/…` only as an "e.g."; §7.4's path is used.
    pub async fn remove_dm_reaction(
        &self,
        dm_id: &DmId,
        message_id: &LocalId,
        key: &str,
    ) -> Result<()> {
        let path = format!(
            "{}{}",
            dm_message_path(dm_id, message_id),
            path("/reactions/{}", &[key])
        );
        self.send_no_content(Method::DELETE, path, Query::new())
            .await
    }

    /// Edit a private message the signer sent, at its recipient's home
    /// (§7.4 *Edits & deletes*, §8.3). The path is §8.3's "e.g.", the only
    /// one the spec gives; the answer is the edited message, as in §7.1.
    pub async fn edit_dm(
        &self,
        dm_id: &DmId,
        message_id: &LocalId,
        update: &MessageUpdate,
    ) -> Result<Message> {
        self.send_json(
            Method::PATCH,
            federation_message_path(dm_id, message_id),
            update,
        )
        .await
    }

    /// Delete a private message the signer sent, at its recipient's home,
    /// leaving a tombstone (§7.4 *Edits & deletes*, §8.3, §7.1). The path
    /// is §8.3's "e.g.", the only one the spec gives.
    pub async fn delete_dm(&self, dm_id: &DmId, message_id: &LocalId) -> Result<()> {
        self.send_no_content(
            Method::DELETE,
            federation_message_path(dm_id, message_id),
            Query::new(),
        )
        .await
    }
}

fn dm_messages_path(dm_id: &DmId) -> String {
    path("/api/dms/{}/messages", &[dm_id.as_str()])
}

fn dm_message_path(dm_id: &DmId, message_id: &LocalId) -> String {
    path(
        "/api/dms/{}/messages/{}",
        &[dm_id.as_str(), message_id.as_str()],
    )
}

fn federation_message_path(dm_id: &DmId, message_id: &LocalId) -> String {
    path(
        "/api/federation/dms/{}/messages/{}",
        &[dm_id.as_str(), message_id.as_str()],
    )
}
