//! The Calendar API's calls for invitations: finding the event an
//! invitation describes in the person's calendar, and answering it there.

use postrinch_model::EmailAddress;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{FG, GmailApi, Payload};
use crate::google::quota::Cost;
use crate::{SyncError, SyncResult};

pub(super) const SERVICE: &str = "Google Calendar";

/// An event as the Calendar API gives it, as far as Postrinch reads it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventResource {
    pub id: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// `transparent` for an event that leaves the person free.
    #[serde(default)]
    pub transparency: Option<String>,
    pub start: Option<EventTime>,
    pub end: Option<EventTime>,
    #[serde(default)]
    pub organizer: Option<Person>,
    #[serde(default)]
    pub attendees: Vec<AttendeeResource>,
    #[serde(default)]
    pub html_link: Option<String>,
    #[serde(default)]
    pub recurring_event_id: Option<String>,
    #[serde(default)]
    pub original_start_time: Option<EventTime>,
    /// A series' `RRULE`, `EXRULE`, `RDATE` and `EXDATE` lines.
    #[serde(default)]
    pub recurrence: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventTime {
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub date_time: Option<String>,
    #[serde(default)]
    pub time_zone: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Person {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// A guest, with every field the API gave kept, so that a change sends
/// the list back as it was but for the person's own answer.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttendeeResource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_status: Option<String>,
    #[serde(default, rename = "self", skip_serializing_if = "std::ops::Not::not")]
    pub me: bool,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

#[derive(Deserialize)]
struct EventList {
    #[serde(default)]
    items: Vec<EventResource>,
}

impl GmailApi {
    /// The events in the account's main calendar kept under `uid` (an
    /// iCalendar `UID`): a single event, or a repeating one and the times
    /// of it that were changed.
    pub(crate) async fn events_by_uid(
        &self,
        account: &EmailAddress,
        uid: &str,
    ) -> SyncResult<Vec<EventResource>> {
        let url = format!("{}/calendars/primary/events", self.calendar);
        let body = self
            .call(
                Method::GET,
                account,
                (Cost::Calendar, FG),
                (&url, SERVICE),
                &[("iCalUID", uid), ("showDeleted", "true")],
                None,
                true,
            )
            .await?;
        let list: EventList = serde_json::from_slice(&body)
            .map_err(|e| SyncError::api(SERVICE, 200, format!("unreadable events: {e}")))?;
        Ok(list.items)
    }

    /// Give the event these guests (the whole list, as the API asks), and
    /// tell the organizer.
    pub(crate) async fn set_attendees(
        &self,
        account: &EmailAddress,
        event: &str,
        attendees: &[AttendeeResource],
    ) -> SyncResult<EventResource> {
        let url = format!("{}/calendars/primary/events/{event}", self.calendar);
        let body = json!({ "attendees": attendees });
        let answer = self
            .call(
                Method::PATCH,
                account,
                (Cost::Calendar, FG),
                (&url, SERVICE),
                &[("sendUpdates", "all")],
                Some(Payload::Json(&body)),
                // Answering twice says the same thing twice.
                true,
            )
            .await?;
        serde_json::from_slice(&answer)
            .map_err(|e| SyncError::api(SERVICE, 200, format!("unreadable event: {e}")))
    }
}
