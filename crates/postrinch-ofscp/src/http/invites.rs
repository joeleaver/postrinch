//! Invites and join links (§5.6), on the group's host provider: made,
//! listed and revoked by its managers, and redeemed by anyone holding the
//! token.
//!
//! The invite list is paged per §7.2, newest first (re-ofscp#43).

use ofscp_core::LocalId;
use ofscp_core::wire::{
    Invite, InviteCreateRequest, InviteCreateResponse, InviteRedeemResponse, Paged,
};
use reqwest::Method;

use super::Client;
use super::path::{PageRequest, Query, path};
use crate::Result;

impl Client {
    /// Make an invite to a group (§5.6). Needs `manage`; answered `201`
    /// with an [`InviteCreateResponse`], the only answer that carries the
    /// invite's token: listings and reads of an invite leave it out.
    pub async fn create_invite(
        &self,
        group_id: &LocalId,
        request: &InviteCreateRequest,
    ) -> Result<InviteCreateResponse> {
        let path = path("/api/groups/{}/invites", &[group_id.as_str()]);
        self.send_json(Method::POST, path, request).await
    }

    /// Revoke an invite (§5.6). Needs `manage`.
    pub async fn revoke_invite(&self, group_id: &LocalId, invite_id: &LocalId) -> Result<()> {
        let path = path(
            "/api/groups/{}/invites/{}",
            &[group_id.as_str(), invite_id.as_str()],
        );
        self.send_no_content(Method::DELETE, path, Query::new())
            .await
    }

    /// Redeem an invite as this account (§5.6), at the group's provider.
    /// An unknown or expired token is a `404`, a used-up one a `409`.
    pub async fn redeem_invite(&self, token: &str) -> Result<InviteRedeemResponse> {
        self.send_empty(Method::POST, path("/api/invites/{}/redeem", &[token]))
            .await
    }

    /// One page of a group's invites (§5.6). Needs `manage`. Providers may
    /// leave each invite's token out, and usually do.
    pub async fn invites(&self, group_id: &LocalId, page: &PageRequest) -> Result<Paged<Invite>> {
        self.get(
            path("/api/groups/{}/invites", &[group_id.as_str()]),
            page.query(),
        )
        .await
    }
}
