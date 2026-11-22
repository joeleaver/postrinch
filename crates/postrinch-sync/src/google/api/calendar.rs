//! The Calendar API's calls for invitations: finding the event an
//! invitation describes in the person's calendar, and answering it there.

use postrinch_model::{Answer, Attendee, CalendarEvent, Day, EmailAddress, EventWhen, Mailbox};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{FG, GmailApi, Payload};
use crate::google::quota::Cost;
use crate::{SyncError, SyncResult};

const SERVICE: &str = "Google Calendar";

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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventTime {
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub date_time: Option<String>,
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

/// The API's word for an answer.
pub(crate) fn response_status(answer: Answer) -> &'static str {
    match answer {
        Answer::Yes => "accepted",
        Answer::Maybe => "tentative",
        Answer::No => "declined",
    }
}

fn answer_of(status: Option<&str>) -> Option<Answer> {
    match status? {
        "accepted" => Some(Answer::Yes),
        "tentative" => Some(Answer::Maybe),
        "declined" => Some(Answer::No),
        _ => None,
    }
}

fn mailbox(email: Option<&str>, name: Option<&str>) -> Option<Mailbox> {
    let address = EmailAddress::parse(email?).ok()?;
    Some(Mailbox::new(name.map(str::to_owned), address))
}

/// The event as Postrinch keeps it; `None` when it has no start or end.
pub(crate) fn to_event(r: &EventResource) -> Option<CalendarEvent> {
    Some(CalendarEvent {
        id: r.id.clone(),
        summary: r.summary.clone().filter(|s| !s.trim().is_empty()),
        location: r.location.clone().filter(|s| !s.trim().is_empty()),
        when: when(r.start.as_ref()?, r.end.as_ref()?)?,
        organizer: r
            .organizer
            .as_ref()
            .and_then(|o| mailbox(o.email.as_deref(), o.display_name.as_deref())),
        attendees: r
            .attendees
            .iter()
            .filter_map(|a| {
                Some(Attendee {
                    mailbox: mailbox(a.email.as_deref(), a.display_name.as_deref())?,
                    answer: answer_of(a.response_status.as_deref()),
                })
            })
            .collect(),
        guest: r.attendees.iter().any(|a| a.me),
        answer: r
            .attendees
            .iter()
            .find(|a| a.me)
            .and_then(|a| answer_of(a.response_status.as_deref())),
        cancelled: r.status.as_deref() == Some("cancelled"),
        repeating: r.recurring_event_id.is_some(),
        link: r.html_link.clone(),
    })
}

fn when(start: &EventTime, end: &EventTime) -> Option<EventWhen> {
    if let (Some(first), Some(after)) = (&start.date, &end.date) {
        let first: jiff::civil::Date = first.parse().ok()?;
        let after: jiff::civil::Date = after.parse().ok()?;
        // The end day is the day after the last.
        let last = if after > first {
            after.yesterday().ok()?
        } else {
            first
        };
        return Some(EventWhen::Days {
            first: day(first),
            last: day(last),
        });
    }
    let at = |t: &EventTime| -> Option<postrinch_model::Timestamp> {
        let t: jiff::Timestamp = t.date_time.as_deref()?.parse().ok()?;
        Some(postrinch_model::Timestamp::from_millis(t.as_millisecond()))
    };
    Some(EventWhen::Times {
        start: at(start)?,
        end: at(end)?,
    })
}

fn day(d: jiff::civil::Date) -> Day {
    Day {
        year: d.year(),
        month: d.month(),
        day: d.day(),
    }
}

/// Whether `event` is the time `recurrence` (an iCalendar `RECURRENCE-ID`
/// value, `20261201T090000` or `20261201`) of a repeating event, comparing
/// the digits of its original start as the calendar writes it.
pub(crate) fn is_time(event: &EventResource, recurrence: &str) -> bool {
    let Some(original) = &event.original_start_time else {
        return false;
    };
    let digits = |s: &str| s.chars().filter(char::is_ascii_digit).collect::<String>();
    let want = digits(recurrence);
    let have = match (&original.date_time, &original.date) {
        (Some(t), _) => digits(t.get(..19).unwrap_or(t)),
        (None, Some(d)) => digits(d),
        _ => return false,
    };
    !want.is_empty() && have == want
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(json: Value) -> EventResource {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn reads_an_event_and_the_person_s_answer() {
        let r = event(json!({
            "id": "e1",
            "status": "confirmed",
            "summary": "Plans",
            "start": {"dateTime": "2026-11-24T10:00:00-05:00", "timeZone": "America/New_York"},
            "end": {"dateTime": "2026-11-24T11:00:00-05:00"},
            "organizer": {"email": "ada@example.org", "displayName": "Ada"},
            "attendees": [
                {"email": "ada@example.org", "responseStatus": "accepted", "organizer": true},
                {"email": "me@example.org", "responseStatus": "needsAction", "self": true},
            ],
            "htmlLink": "https://www.google.com/calendar/event?eid=x",
        }));
        let e = to_event(&r).unwrap();
        assert_eq!(e.summary.as_deref(), Some("Plans"));
        assert_eq!(e.answer, None);
        assert_eq!(e.attendees[0].answer, Some(Answer::Yes));
        let EventWhen::Times { start, end } = e.when else {
            panic!()
        };
        assert_eq!(end.as_millis() - start.as_millis(), 3_600_000);
        // Sent back as it came, but for what is changed.
        let back = serde_json::to_value(&r.attendees).unwrap();
        assert_eq!(back[0]["organizer"], json!(true));
        assert_eq!(back[1]["self"], json!(true));
        assert!(back[0].get("self").is_none());
    }

    #[test]
    fn whole_days_and_changed_times() {
        let r = event(json!({
            "id": "e2_20261201T090000Z",
            "start": {"date": "2026-12-24"},
            "end": {"date": "2026-12-26"},
            "recurringEventId": "e2",
            "originalStartTime": {"dateTime": "2026-12-01T09:00:00Z"},
        }));
        let e = to_event(&r).unwrap();
        assert!(e.repeating);
        let EventWhen::Days { first, last } = e.when else {
            panic!()
        };
        assert_eq!((first.day, last.day), (24, 25));
        assert!(is_time(&r, "20261201T090000"));
        assert!(!is_time(&r, "20261208T090000"));
    }
}
