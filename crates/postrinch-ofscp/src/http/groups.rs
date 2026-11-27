//! Groups and channels (§5.5), on the group's host provider: the caller
//! picks the [`Client`] for that provider. Invites are in `invites`.

use ofscp_core::LocalId;
use ofscp_core::wire::{
    Channel, ChannelCreateRequest, ChannelUpdateRequest, ChannelsResponse, Group,
    GroupCreateRequest, GroupUpdateRequest,
};
use reqwest::Method;

use super::path::{Query, path};
use super::{Client, Provider};
use crate::Result;

impl Provider {
    /// A `public` or `discoverable` group, read as an anonymous viewer
    /// (§5.5, §4.5). Any other tier is a `403`.
    pub async fn group(&self, group_id: &LocalId) -> Result<Group> {
        self.get_anonymous(group_path(group_id), Query::new()).await
    }

    /// A channel the anonymous viewer may read (§5.5, §11.3); any other is
    /// a `404`.
    pub async fn channel(&self, group_id: &LocalId, channel_id: &LocalId) -> Result<Channel> {
        self.get_anonymous(channel_path(group_id, channel_id), Query::new())
            .await
    }
}

impl Client {
    /// Create a group on this provider (§5.5); the signer becomes its
    /// `owner`. Answered `201` with the new group.
    pub async fn create_group(&self, request: &GroupCreateRequest) -> Result<Group> {
        self.send_json(Method::POST, "/api/groups".to_owned(), request)
            .await
    }

    /// A group (§5.5). For a member it carries their `membership`, which
    /// is authoritative; its absence proves nothing.
    pub async fn group(&self, group_id: &LocalId) -> Result<Group> {
        self.get(group_path(group_id), Query::new()).await
    }

    /// Change a group's settings (§5.5); fields left `None` stay. Needs the
    /// group's `manage` permission.
    pub async fn update_group(
        &self,
        group_id: &LocalId,
        update: &GroupUpdateRequest,
    ) -> Result<Group> {
        self.send_json(Method::PATCH, group_path(group_id), update)
            .await
    }

    /// Delete a group and its channels (§5.5). Owner only.
    pub async fn delete_group(&self, group_id: &LocalId) -> Result<()> {
        self.send_no_content(Method::DELETE, group_path(group_id), Query::new())
            .await
    }

    /// Create a channel in a group (§5.5). Needs `manage`; answered `201`
    /// with the new channel.
    pub async fn create_channel(
        &self,
        group_id: &LocalId,
        request: &ChannelCreateRequest,
    ) -> Result<Channel> {
        let path = path("/api/groups/{}/channels", &[group_id.as_str()]);
        self.send_json(Method::POST, path, request).await
    }

    /// The group's channels the signer can read (§5.5, §11.3): those it
    /// cannot are left out. The answer is one unpaged `{ items }`.
    pub async fn channels(&self, group_id: &LocalId) -> Result<Vec<Channel>> {
        let path = path("/api/groups/{}/channels", &[group_id.as_str()]);
        let list: ChannelsResponse = self.get(path, Query::new()).await?;
        Ok(list.items)
    }

    /// A channel (§5.5), under effective read access (§11.3): one the
    /// signer cannot read is a `404`.
    pub async fn channel(&self, group_id: &LocalId, channel_id: &LocalId) -> Result<Channel> {
        self.get(channel_path(group_id, channel_id), Query::new())
            .await
    }

    /// Change a channel (§5.5); its kind cannot change. Needs `manage`.
    pub async fn update_channel(
        &self,
        group_id: &LocalId,
        channel_id: &LocalId,
        update: &ChannelUpdateRequest,
    ) -> Result<Channel> {
        self.send_json(Method::PATCH, channel_path(group_id, channel_id), update)
            .await
    }

    /// Delete a channel (§5.5). Needs `manage`.
    pub async fn delete_channel(&self, group_id: &LocalId, channel_id: &LocalId) -> Result<()> {
        self.send_no_content(
            Method::DELETE,
            channel_path(group_id, channel_id),
            Query::new(),
        )
        .await
    }
}

fn group_path(group_id: &LocalId) -> String {
    path("/api/groups/{}", &[group_id.as_str()])
}

fn channel_path(group_id: &LocalId, channel_id: &LocalId) -> String {
    path(
        "/api/groups/{}/channels/{}",
        &[group_id.as_str(), channel_id.as_str()],
    )
}
