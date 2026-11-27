//! The notification feed (§10.1) and notification settings (§10.2), both
//! optional for providers (`404` if not offered) and home-only.
//!
//! Not here, for want of a type in ofscp-core: `POST
//! /api/me/notifications/seen` and `.../read` (no `{ ids }` request type)
//! and `DELETE /api/me/notification-settings` (no `{ cleared }` response
//! type).

use ofscp_core::wire::{
    NotificationKind, NotificationSetting, NotificationSettingsResponse, NotificationsResponse,
};
use reqwest::Method;

use super::Client;
use super::path::Query;
use crate::Result;

impl Client {
    /// The signer's notifications, newest first (§10.1), with unseen
    /// counts. `kind` filters; `limit` defaults to 30 (at most 100);
    /// `cursor` is a previous answer's `nextCursor`. Marks nothing seen.
    pub async fn notifications(
        &self,
        kind: Option<&NotificationKind>,
        limit: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<NotificationsResponse> {
        self.require_home()?;
        let mut query = Query::new();
        query.opt("type", kind.map(NotificationKind::as_str));
        query.opt("limit", limit.map(|l| l.to_string()).as_deref());
        query.opt("cursor", cursor);
        self.get("/api/me/notifications".to_owned(), query).await
    }

    /// The signer's notification settings (§10.2).
    pub async fn notification_settings(&self) -> Result<NotificationSettingsResponse> {
        self.require_home()?;
        self.get("/api/me/notification-settings".to_owned(), Query::new())
            .await
    }

    /// Set how much a group or channel notifies the signer (§10.2): an
    /// upsert, answered with the stored setting.
    pub async fn set_notification_setting(
        &self,
        setting: &NotificationSetting,
    ) -> Result<NotificationSetting> {
        self.require_home()?;
        self.send_json(
            Method::PUT,
            "/api/me/notification-settings".to_owned(),
            setting,
        )
        .await
    }
}
