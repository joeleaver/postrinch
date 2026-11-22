//! The Calendar API's events and calendars as Postrinch keeps them.

use postrinch_model::{
    Answer, Attendee, CalendarEvent, CalendarInfo, Day, EmailAddress, EventRecord, EventStart,
    EventWhen, Mailbox, Timestamp,
};

use super::calendar::{EventResource, EventTime};
use super::calendar_list::CalendarResource;

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

fn text(t: &Option<String>) -> Option<String> {
    t.clone().filter(|s| !s.trim().is_empty())
}

/// The event in `calendar` as Postrinch keeps it; `None` when it has no
/// start or end.
pub(crate) fn to_event(r: &EventResource, calendar: &str) -> Option<CalendarEvent> {
    Some(CalendarEvent {
        id: r.id.clone(),
        calendar: calendar.to_owned(),
        summary: text(&r.summary),
        location: text(&r.location),
        description: text(&r.description),
        busy: r.transparency.as_deref() != Some("transparent"),
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
        repeating: r.recurring_event_id.is_some() || !r.recurrence.is_empty(),
        link: r.html_link.clone(),
    })
}

/// The event as its calendar keeps it. A cancelled time of a series comes
/// without its own times; it is kept at the time it was, to leave out.
pub(crate) fn to_record(r: &EventResource, calendar: &str) -> Option<EventRecord> {
    let original_start = r.original_start_time.as_ref().and_then(start_of);
    let event = match to_event(r, calendar) {
        Some(event) => event,
        None if r.recurring_event_id.is_some() => {
            let mut at_original = r.clone();
            at_original.start = r.original_start_time.clone();
            at_original.end = r.original_start_time.clone();
            to_event(&at_original, calendar)?
        }
        None => return None,
    };
    Some(EventRecord {
        event,
        recurrence: r.recurrence.clone(),
        time_zone: r.start.as_ref().and_then(|s| s.time_zone.clone()),
        series: r.recurring_event_id.clone(),
        original_start,
    })
}

/// One calendar as the account lists it.
pub(crate) fn to_calendar(c: &CalendarResource) -> CalendarInfo {
    CalendarInfo {
        id: c.id.clone(),
        name: text(&c.summary_override)
            .or_else(|| text(&c.summary))
            .unwrap_or_else(|| c.id.clone()),
        color: c.background_color.clone(),
        primary: c.primary,
        writable: matches!(c.access_role.as_deref(), Some("owner" | "writer")),
        shown: c.selected,
        time_zone: c.time_zone.clone(),
    }
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
    Some(EventWhen::Times {
        start: moment(start)?,
        end: moment(end)?,
    })
}

fn moment(t: &EventTime) -> Option<Timestamp> {
    let t: jiff::Timestamp = t.date_time.as_deref()?.parse().ok()?;
    Some(Timestamp::from_millis(t.as_millisecond()))
}

fn start_of(t: &EventTime) -> Option<EventStart> {
    match (&t.date_time, &t.date) {
        (Some(_), _) => moment(t).map(EventStart::At),
        (None, Some(d)) => d.parse().ok().map(|d| EventStart::Day(day(d))),
        _ => None,
    }
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
#[path = "calendar_convert_tests.rs"]
mod tests;
