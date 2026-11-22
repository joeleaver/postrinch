//! Changing the person's calendars from here: answering an event, making,
//! changing and deleting one. Google is told at once, its guests too when
//! the event has any, and the calendar is synced straight after, so what
//! is kept here shows the change.

use postrinch_model::{Answer, EmailAddress, EventDraft, EventWhen};
use serde_json::{Value, json};

use super::GmailSync;
use super::calendar::Answered;
use crate::SyncResult;
use crate::google::api::{AttendeeResource, response_status, to_event};

impl GmailSync {
    /// Answer an event of one of the account's calendars as the account,
    /// telling the organizer.
    pub async fn answer_event(
        &self,
        account: &EmailAddress,
        calendar: &str,
        event: &str,
        answer: Answer,
    ) -> SyncResult<Answered> {
        let found = match self.api.get_event(account, calendar, event).await {
            Ok(found) => found,
            Err(e) if e.is_not_found() => return Ok(Answered::NotInCalendar),
            Err(e) => return Err(e),
        };
        let mut guests = found.attendees.clone();
        let Some(me) = guests.iter_mut().find(|a| a.me) else {
            return Ok(Answered::NotAGuest);
        };
        me.response_status = Some(response_status(answer).to_owned());
        let changed = self
            .api
            .patch_event(
                account,
                calendar,
                event,
                &json!({ "attendees": guests }),
                true,
            )
            .await?;
        self.after_change(account, calendar).await;
        Ok(match to_event(&changed, calendar) {
            Some(event) => Answered::Done(Box::new(event)),
            None => Answered::NotInCalendar,
        })
    }

    /// Make an event; its guests are invited. Returns its id.
    pub async fn create_event(
        &self,
        account: &EmailAddress,
        draft: &EventDraft,
    ) -> SyncResult<String> {
        let mut body = fields(draft);
        body["attendees"] = json!(
            draft
                .guests
                .iter()
                .map(|g| new_guest(&g.address.to_string(), g.name.as_deref()))
                .collect::<Vec<_>>()
        );
        let made = self
            .api
            .insert_event(account, &draft.calendar, &body, !draft.guests.is_empty())
            .await?;
        self.after_change(account, &draft.calendar).await;
        Ok(made.id)
    }

    /// Change an event to what `draft` says. Guests who stay keep their
    /// answers; new ones are invited, and every guest is told.
    pub async fn change_event(
        &self,
        account: &EmailAddress,
        event: &str,
        draft: &EventDraft,
    ) -> SyncResult<()> {
        let found = self.api.get_event(account, &draft.calendar, event).await?;
        let wanted: Vec<String> = draft.guests.iter().map(|g| g.address.key()).collect();
        let mut guests: Vec<AttendeeResource> = found
            .attendees
            .iter()
            .filter(|a| {
                a.me || a.rest.get("organizer") == Some(&json!(true))
                    || a.email
                        .as_deref()
                        .is_some_and(|e| wanted.contains(&e.to_lowercase()))
            })
            .cloned()
            .collect();
        for g in &draft.guests {
            let key = g.address.key();
            let there = guests
                .iter()
                .any(|a| a.email.as_deref().is_some_and(|e| e.to_lowercase() == key));
            if !there {
                guests.push(
                    serde_json::from_value(new_guest(&g.address.to_string(), g.name.as_deref()))
                        .expect("a guest reads back"),
                );
            }
        }
        let tell = !guests.is_empty() || !found.attendees.is_empty();
        let mut body = fields(draft);
        body["attendees"] = json!(guests);
        self.api
            .patch_event(account, &draft.calendar, event, &body, tell)
            .await?;
        self.after_change(account, &draft.calendar).await;
        Ok(())
    }

    /// Delete an event (one time of a repeating one, when that is what it
    /// names); its guests are told.
    pub async fn delete_event(
        &self,
        account: &EmailAddress,
        calendar: &str,
        event: &str,
        tell: bool,
    ) -> SyncResult<()> {
        self.api
            .delete_event(account, calendar, event, tell)
            .await?;
        self.after_change(account, calendar).await;
        Ok(())
    }

    /// Sync the changed calendar now; trouble waits for the next pass.
    pub(super) async fn after_change(&self, account: &EmailAddress, calendar: &str) {
        if let Err(e) = self.sync_calendar_now(account, calendar).await {
            tracing::warn!("{account}: syncing calendar {calendar} after a change: {e}");
        }
    }
}

fn new_guest(address: &str, name: Option<&str>) -> Value {
    match name {
        Some(name) => json!({ "email": address, "displayName": name }),
        None => json!({ "email": address }),
    }
}

/// What a draft says, as the API's fields.
pub(crate) fn fields(draft: &EventDraft) -> Value {
    let (start, end) = match draft.when {
        EventWhen::Days { first, last } => {
            let day = |d: postrinch_model::Day| {
                jiff::civil::Date::new(d.year, d.month, d.day).unwrap_or_default()
            };
            let after = day(last).tomorrow().unwrap_or(day(last));
            (
                json!({ "date": day(first).to_string() }),
                json!({ "date": after.to_string() }),
            )
        }
        EventWhen::Times { start, end } => {
            let at = |t: postrinch_model::Timestamp| {
                jiff::Timestamp::from_millisecond(t.as_millis())
                    .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
                    .to_string()
            };
            let zone = |at: String| match &draft.time_zone {
                Some(z) => json!({ "dateTime": at, "timeZone": z }),
                None => json!({ "dateTime": at }),
            };
            (zone(at(start)), zone(at(end)))
        }
    };
    json!({
        "summary": draft.summary.trim(),
        "location": draft.location.trim(),
        "description": draft.description.trim(),
        "start": start,
        "end": end,
    })
}

#[cfg(test)]
mod tests {
    use postrinch_model::{Day, Timestamp};

    use super::*;

    fn draft(when: EventWhen) -> EventDraft {
        EventDraft {
            calendar: "me@example.org".into(),
            summary: " Walkthrough ".into(),
            location: String::new(),
            description: String::new(),
            when,
            time_zone: Some("America/Denver".into()),
            guests: Vec::new(),
        }
    }

    #[test]
    fn whole_days_end_the_day_after_and_times_carry_their_zone() {
        let d = |day| Day {
            year: 2026,
            month: 12,
            day,
        };
        let body = fields(&draft(EventWhen::Days {
            first: d(24),
            last: d(31),
        }));
        assert_eq!(body["start"], json!({"date": "2026-12-24"}));
        assert_eq!(body["end"], json!({"date": "2027-01-01"}));
        assert_eq!(body["summary"], "Walkthrough");
        let body = fields(&draft(EventWhen::Times {
            start: Timestamp::from_millis(0),
            end: Timestamp::from_millis(3_600_000),
        }));
        assert_eq!(
            body["start"],
            json!({"dateTime": "1970-01-01T00:00:00Z", "timeZone": "America/Denver"})
        );
    }
}
