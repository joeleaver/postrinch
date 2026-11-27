//! Membership (§5.7): joining and leaving, members, join requests, and the
//! "My groups" pointers kept at the user's home provider.
//!
//! Group calls go to the group's host provider, local or remote (§8.2);
//! the pointer calls go to the signer's home. Not here, for want of a type
//! in ofscp-core: the role change and display-name `PATCH`es (no request
//! body type), and the join request's optional `{ message }` body
//! (re-ofscp#43). The pending join requests are listed paged per §7.2,
//! newest first (re-ofscp#43).

use ofscp_core::wire::{
    GroupPointer, GroupPointerRequest, GroupPointersResponse, JoinRequest, Member, Paged,
};
use ofscp_core::{Actor, LocalId, ObjectUri};
use reqwest::Method;

use super::Client;
use super::path::{PageRequest, Query, path};
use super::response::Reply;
use super::send::{Auth, Call};
use crate::{Error, Result};

/// How a provider answered a join (§5.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinOutcome {
    /// `201`: an `open` group; the caller is now a member.
    Joined(Member),
    /// `202`: a `request` group; the request waits for a manager.
    Requested(JoinRequest),
    /// `200`: the caller was a member already.
    AlreadyMember(Member),
}

impl JoinOutcome {
    /// The outcome a join's answer means, by its status.
    fn from_reply(reply: &Reply) -> Result<Self> {
        match reply.status {
            201 => reply.json().map(Self::Joined),
            202 => reply.json().map(Self::Requested),
            200 => reply.json().map(Self::AlreadyMember),
            status => Err(Error::UnexpectedStatus {
                authority: reply.authority.clone(),
                status,
                expected: "201 Created, 202 Accepted or 200 OK",
            }),
        }
    }
}

impl Client {
    /// Join a group, at its provider (§5.7). An `invite` or `private`-tier
    /// group is a `403`: redeem an invite instead (§5.6).
    pub async fn join(&self, group_id: &LocalId) -> Result<JoinOutcome> {
        let call = Call::new(
            Method::POST,
            path("/api/groups/{}/join", &[group_id.as_str()]),
        );
        let reply = self
            .provider()
            .execute(Auth::Signed(self.identity()), call)
            .await?;
        JoinOutcome::from_reply(&reply)
    }

    /// Leave a group (§5.7). The owner must hand ownership on first
    /// (`409` otherwise).
    pub async fn leave(&self, group_id: &LocalId) -> Result<()> {
        let path = path("/api/groups/{}/leave", &[group_id.as_str()]);
        self.send_no_content(Method::POST, path, Query::new()).await
    }

    /// One page of a group's members (§5.7, paged per §7.2).
    pub async fn members(&self, group_id: &LocalId, page: &PageRequest) -> Result<Paged<Member>> {
        self.get(
            path("/api/groups/{}/members", &[group_id.as_str()]),
            page.query(),
        )
        .await
    }

    /// Remove a member (§5.7). Needs `moderate` and every permission the
    /// member holds; the owner cannot be removed.
    pub async fn kick(&self, group_id: &LocalId, user: &Actor) -> Result<()> {
        let user = user.to_string();
        let path = path("/api/groups/{}/members/{}", &[group_id.as_str(), &user]);
        self.send_no_content(Method::DELETE, path, Query::new())
            .await
    }

    /// One page of a group's pending join requests (§5.7). Needs `manage`
    /// or `moderate`.
    pub async fn join_requests(
        &self,
        group_id: &LocalId,
        page: &PageRequest,
    ) -> Result<Paged<JoinRequest>> {
        self.get(
            path("/api/groups/{}/requests", &[group_id.as_str()]),
            page.query(),
        )
        .await
    }

    /// Let a pending join request in (§5.7); answers the new member.
    pub async fn approve_join_request(
        &self,
        group_id: &LocalId,
        request_id: &LocalId,
    ) -> Result<Member> {
        let path = path(
            "/api/groups/{}/requests/{}/approve",
            &[group_id.as_str(), request_id.as_str()],
        );
        self.send_empty(Method::POST, path).await
    }

    /// Turn down a pending join request (§5.7).
    pub async fn deny_join_request(&self, group_id: &LocalId, request_id: &LocalId) -> Result<()> {
        let path = path(
            "/api/groups/{}/requests/{}/deny",
            &[group_id.as_str(), request_id.as_str()],
        );
        self.send_no_content(Method::POST, path, Query::new()).await
    }

    /// The signer's group list at home (§5.7, *My groups*): the groups home
    /// hosts that they are in, and every pointer they recorded.
    pub async fn my_groups(&self) -> Result<GroupPointersResponse> {
        self.require_home()?;
        self.get("/api/me/groups".to_owned(), Query::new()).await
    }

    /// Record or update the pointer to a group, at home (§5.7, *My
    /// groups*), with an optional role hint. Idempotent.
    pub async fn put_group_pointer(
        &self,
        group: &ObjectUri,
        request: &GroupPointerRequest,
    ) -> Result<GroupPointer> {
        self.require_home()?;
        self.send_json(Method::PUT, group_pointer_path(group), request)
            .await
    }

    /// Remove the pointer to a group, at home (§5.7, *My groups*); `204`
    /// also when there was none. A hosted group the signer is in is a
    /// `409`: leave it instead.
    pub async fn delete_group_pointer(&self, group: &ObjectUri) -> Result<()> {
        self.require_home()?;
        self.send_no_content(Method::DELETE, group_pointer_path(group), Query::new())
            .await
    }
}

fn group_pointer_path(group: &ObjectUri) -> String {
    path("/api/me/groups/{}", &[&group.to_string()])
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use reqwest::header::HeaderMap;
    use serde_json::json;

    use super::*;

    fn reply(status: u16, body: serde_json::Value) -> Reply {
        Reply {
            authority: "b.example".to_owned(),
            status,
            headers: HeaderMap::new(),
            body: Bytes::from(body.to_string()),
        }
    }

    fn member() -> serde_json::Value {
        json!({"user": "jane@a.com", "role": "member", "joinedAt": "2025-03-01T12:00:00Z"})
    }

    #[test]
    fn a_join_is_read_by_its_status() {
        assert!(matches!(
            JoinOutcome::from_reply(&reply(201, member())).unwrap(),
            JoinOutcome::Joined(m) if m.role == "member"
        ));
        assert!(matches!(
            JoinOutcome::from_reply(&reply(200, member())).unwrap(),
            JoinOutcome::AlreadyMember(_)
        ));
        let request = json!({
            "id": "jr_1", "groupId": "grp_1", "user": "jane@a.com",
            "state": "pending", "requestedAt": "2025-03-01T12:00:00Z"
        });
        assert!(matches!(
            JoinOutcome::from_reply(&reply(202, request)).unwrap(),
            JoinOutcome::Requested(r) if r.id.as_str() == "jr_1"
        ));
    }

    #[test]
    fn a_join_with_another_success_status_is_an_error() {
        let err = JoinOutcome::from_reply(&reply(204, json!({}))).unwrap_err();
        assert!(matches!(err, Error::UnexpectedStatus { status: 204, .. }));
    }

    #[test]
    fn a_join_body_that_does_not_match_its_status_is_a_decode_error() {
        let err = JoinOutcome::from_reply(&reply(202, member())).unwrap_err();
        assert!(matches!(err, Error::Decode { .. }));
    }
}
