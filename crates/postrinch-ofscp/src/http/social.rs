//! Contacts (§6.7), follows (§7.6) and read markers (§7.7): pointers and
//! state kept at the signer's home provider, so every call here is
//! home-only.
//!
//! Not here: `POST /api/federation/contacts` (§6.7 *Federation*), which a
//! client sends to the counterparty's home with a `ContactEvent`; the spec
//! does not say what it answers.

use ofscp_core::wire::{
    Contact, ContactCreateRequest, ContactsResponse, Follow, FollowCreateRequest, FollowsResponse,
    ReadMarkersResponse, ReadMarkersUpdateRequest,
};
use ofscp_core::{Actor, ObjectRef};
use reqwest::Method;

use super::Client;
use super::path::{Query, path};
use crate::Result;

impl Client {
    /// The signer's contacts (§6.7), pending ones in both directions too.
    pub async fn contacts(&self) -> Result<ContactsResponse> {
        self.require_home()?;
        self.get("/api/me/contacts".to_owned(), Query::new()).await
    }

    /// Ask someone to be a contact (§6.7): records an outgoing pending
    /// entry. A remote counterparty also needs the federated `request`.
    pub async fn add_contact(&self, request: &ContactCreateRequest) -> Result<Contact> {
        self.require_home()?;
        self.send_json(Method::POST, "/api/me/contacts".to_owned(), request)
            .await
    }

    /// Accept someone's incoming contact request (§6.7).
    pub async fn accept_contact(&self, user: &Actor) -> Result<Contact> {
        self.require_home()?;
        let path = path("/api/me/contacts/{}/accept", &[&user.to_string()]);
        self.send_empty(Method::POST, path).await
    }

    /// Cancel, decline or end a contact (§6.7).
    pub async fn remove_contact(&self, user: &Actor) -> Result<()> {
        self.require_home()?;
        let path = path("/api/me/contacts/{}", &[&user.to_string()]);
        self.send_no_content(Method::DELETE, path, Query::new())
            .await
    }

    /// The channels the signer follows (§7.6).
    pub async fn follows(&self) -> Result<FollowsResponse> {
        self.require_home()?;
        self.get("/api/me/follows".to_owned(), Query::new()).await
    }

    /// Follow a channel (§7.6). Idempotent: an existing follow is answered
    /// as it is. A channel the signer cannot read may be a `404`.
    pub async fn follow(&self, request: &FollowCreateRequest) -> Result<Follow> {
        self.require_home()?;
        self.send_json(Method::POST, "/api/me/follows".to_owned(), request)
            .await
    }

    /// Stop following a channel (§7.6), named as it was followed.
    pub async fn unfollow(&self, channel: &ObjectRef) -> Result<()> {
        self.require_home()?;
        let path = path("/api/me/follows/{}", &[&channel.to_string()]);
        self.send_no_content(Method::DELETE, path, Query::new())
            .await
    }

    /// The signer's read markers and unread counts (§7.7, optional for
    /// providers: a `404` if not offered).
    pub async fn read_markers(&self) -> Result<ReadMarkersResponse> {
        self.require_home()?;
        self.get("/api/me/read-markers".to_owned(), Query::new())
            .await
    }

    /// Move read markers forward (§7.7). Answers only the scopes that
    /// advanced; others are ignored without error.
    pub async fn update_read_markers(
        &self,
        request: &ReadMarkersUpdateRequest,
    ) -> Result<ReadMarkersResponse> {
        self.require_home()?;
        self.send_json(Method::PATCH, "/api/me/read-markers".to_owned(), request)
            .await
    }
}
