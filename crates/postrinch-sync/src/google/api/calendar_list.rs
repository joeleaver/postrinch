//! The Calendar API's calls for keeping calendars here: the account's
//! calendars, and each one's events, page by page, from where the last
//! sync got to.

use postrinch_model::EmailAddress;
use reqwest::Method;
use serde::Deserialize;

use super::calendar::{EventResource, SERVICE};
use super::{GmailApi, Priority};
use crate::google::quota::Cost;
use crate::{SyncError, SyncResult};

/// A calendar as the account's calendar list gives it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CalendarResource {
    pub id: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub summary_override: Option<String>,
    #[serde(default)]
    pub background_color: Option<String>,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub selected: bool,
    #[serde(default)]
    pub access_role: Option<String>,
    #[serde(default)]
    pub time_zone: Option<String>,
    #[serde(default)]
    pub default_reminders: Vec<super::calendar::Reminder>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CalendarList {
    #[serde(default)]
    items: Vec<CalendarResource>,
    #[serde(default)]
    next_page_token: Option<String>,
}

/// One page of a calendar's events.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventPage {
    #[serde(default)]
    pub items: Vec<EventResource>,
    #[serde(default)]
    pub next_page_token: Option<String>,
    /// On the last page: where the next sync starts.
    #[serde(default)]
    pub next_sync_token: Option<String>,
}

/// Calendar sync waits behind what the person is waiting for.
const BG: Priority = Priority::Background;

impl GmailApi {
    /// Every calendar in the account's list.
    pub(crate) async fn calendar_list(
        &self,
        account: &EmailAddress,
    ) -> SyncResult<Vec<CalendarResource>> {
        let url = format!("{}/users/me/calendarList", self.calendar);
        let mut all = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut query = vec![("maxResults", "250")];
            if let Some(p) = &page {
                query.push(("pageToken", p.as_str()));
            }
            let body = self
                .call(
                    Method::GET,
                    account,
                    (Cost::Calendar, BG),
                    (&url, SERVICE),
                    &query,
                    None,
                    true,
                )
                .await?;
            let list: CalendarList = read(&body, "calendars")?;
            all.extend(list.items);
            match list.next_page_token {
                Some(next) => page = Some(next),
                None => return Ok(all),
            }
        }
    }

    /// One page of a calendar's events: every one (deleted ones too) since
    /// `sync` (where the last sync got to), or all of them without. Series
    /// come whole, with their changed times apart. Answered 410 when the
    /// calendar wants a sync from the start.
    pub(crate) async fn events_page(
        &self,
        account: &EmailAddress,
        calendar: &str,
        sync: Option<&str>,
        page: Option<&str>,
    ) -> SyncResult<EventPage> {
        let url = format!(
            "{}/calendars/{}/events",
            self.calendar,
            url::form_urlencoded::byte_serialize(calendar.as_bytes()).collect::<String>()
        );
        let mut query = vec![("maxResults", "2500"), ("singleEvents", "false")];
        match sync {
            Some(token) => query.push(("syncToken", token)),
            // Deleted ones are only needed to undo what is kept.
            None => query.push(("showDeleted", "false")),
        }
        if let Some(p) = page {
            query.push(("pageToken", p));
        }
        let body = self
            .call(
                Method::GET,
                account,
                (Cost::Calendar, BG),
                (&url, SERVICE),
                &query,
                None,
                true,
            )
            .await?;
        read(&body, "events")
    }
}

fn read<T: serde::de::DeserializeOwned>(body: &[u8], what: &str) -> SyncResult<T> {
    serde_json::from_slice(body)
        .map_err(|e| SyncError::api(SERVICE, 200, format!("unreadable {what}: {e}")))
}
