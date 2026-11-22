//! The Calendar API's calls on one event of any calendar: read it, make
//! it, change it, delete it. Each says whether Google tells the guests.

use postrinch_model::EmailAddress;
use reqwest::Method;
use serde_json::Value;

use super::calendar::{EventResource, SERVICE};
use super::{FG, GmailApi, Payload};
use crate::google::quota::Cost;
use crate::{SyncError, SyncResult};

/// A calendar's or an event's id as one part of an address.
fn part(id: &str) -> String {
    url::form_urlencoded::byte_serialize(id.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

fn updates(tell: bool) -> &'static str {
    if tell { "all" } else { "none" }
}

impl GmailApi {
    fn event_url(&self, calendar: &str, event: Option<&str>) -> String {
        match event {
            Some(e) => format!(
                "{}/calendars/{}/events/{}",
                self.calendar,
                part(calendar),
                part(e)
            ),
            None => format!("{}/calendars/{}/events", self.calendar, part(calendar)),
        }
    }

    pub(crate) async fn get_event(
        &self,
        account: &EmailAddress,
        calendar: &str,
        event: &str,
    ) -> SyncResult<EventResource> {
        let url = self.event_url(calendar, Some(event));
        let body = self
            .call(
                Method::GET,
                account,
                (Cost::Calendar, FG),
                (&url, SERVICE),
                &[],
                None,
                true,
            )
            .await?;
        read(&body)
    }

    /// Make an event; with `tell`, Google invites its guests.
    pub(crate) async fn insert_event(
        &self,
        account: &EmailAddress,
        calendar: &str,
        event: &Value,
        tell: bool,
    ) -> SyncResult<EventResource> {
        let url = self.event_url(calendar, None);
        let body = self
            .call(
                Method::POST,
                account,
                (Cost::Calendar, FG),
                (&url, SERVICE),
                &[("sendUpdates", updates(tell))],
                Some(Payload::Json(event)),
                // Sent twice, it would be made twice.
                false,
            )
            .await?;
        read(&body)
    }

    /// Change what `change` names of an event; with `tell`, Google tells
    /// its guests (or, from a guest, the organizer).
    pub(crate) async fn patch_event(
        &self,
        account: &EmailAddress,
        calendar: &str,
        event: &str,
        change: &Value,
        tell: bool,
    ) -> SyncResult<EventResource> {
        let url = self.event_url(calendar, Some(event));
        let body = self
            .call(
                Method::PATCH,
                account,
                (Cost::Calendar, FG),
                (&url, SERVICE),
                &[("sendUpdates", updates(tell))],
                Some(Payload::Json(change)),
                // The same change twice is the same change.
                true,
            )
            .await?;
        read(&body)
    }

    /// Delete an event; with `tell`, Google tells its guests it is off.
    pub(crate) async fn delete_event(
        &self,
        account: &EmailAddress,
        calendar: &str,
        event: &str,
        tell: bool,
    ) -> SyncResult<()> {
        let url = self.event_url(calendar, Some(event));
        let done = self
            .call(
                Method::DELETE,
                account,
                (Cost::Calendar, FG),
                (&url, SERVICE),
                &[("sendUpdates", updates(tell))],
                None,
                true,
            )
            .await;
        match done {
            // Already gone (410) is what was wanted.
            Ok(_) | Err(SyncError::Api { status: 410, .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

fn read(body: &[u8]) -> SyncResult<EventResource> {
    serde_json::from_slice(body)
        .map_err(|e| SyncError::api(SERVICE, 200, format!("unreadable event: {e}")))
}
