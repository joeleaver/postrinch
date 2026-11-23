use serde_json::{Value, json};

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
    let e = to_event(&r, "me@example.org").unwrap();
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
    let e = to_event(&r, "me@example.org").unwrap();
    assert!(e.repeating);
    let EventWhen::Days { first, last } = e.when else {
        panic!()
    };
    assert_eq!((first.day, last.day), (24, 25));
    assert!(is_time(&r, "20261201T090000"));
    assert!(!is_time(&r, "20261208T090000"));
}

#[test]
fn a_series_keeps_its_rule_and_a_cancelled_time_its_original_start() {
    let series = event(json!({
        "id": "s1",
        "summary": "Standup",
        "transparency": "transparent",
        "start": {"dateTime": "2026-11-02T09:00:00-05:00", "timeZone": "America/New_York"},
        "end": {"dateTime": "2026-11-02T09:15:00-05:00", "timeZone": "America/New_York"},
        "recurrence": ["RRULE:FREQ=WEEKLY;BYDAY=MO"],
    }));
    let r = to_record(&series, "work").unwrap();
    assert_eq!(r.recurrence, ["RRULE:FREQ=WEEKLY;BYDAY=MO"]);
    assert_eq!(r.time_zone.as_deref(), Some("America/New_York"));
    assert!(r.event.repeating && !r.event.busy);
    assert_eq!(r.event.calendar, "work");

    let gone = event(json!({
        "id": "s1_20261109T140000Z",
        "status": "cancelled",
        "recurringEventId": "s1",
        "originalStartTime": {"dateTime": "2026-11-09T09:00:00-05:00"},
    }));
    let r = to_record(&gone, "work").unwrap();
    assert!(r.event.cancelled);
    assert_eq!(r.series.as_deref(), Some("s1"));
    let Some(EventStart::At(at)) = r.original_start else {
        panic!("{:?}", r.original_start)
    };
    assert_eq!(r.event.when, EventWhen::Times { start: at, end: at });
}

#[test]
fn calendars_take_the_person_s_name_for_them() {
    let c: CalendarResource = serde_json::from_value(json!({
        "id": "x@group.calendar.google.com",
        "summary": "Team",
        "summaryOverride": "Work team",
        "accessRole": "reader",
        "selected": true,
    }))
    .unwrap();
    let info = to_calendar(&c);
    assert_eq!(info.name, "Work team");
    assert!(!info.writable && info.shown && !info.primary);
}

#[test]
fn reminders_are_the_event_s_or_its_calendar_s() {
    let own = event(json!({
        "id": "r1",
        "start": {"dateTime": "2026-11-24T10:00:00Z"},
        "end": {"dateTime": "2026-11-24T11:00:00Z"},
        "reminders": {"useDefault": false, "overrides": [
            {"method": "email", "minutes": 1440}, {"method": "popup", "minutes": 30},
            {"method": "popup", "minutes": 5},
        ]},
    }));
    assert_eq!(to_event(&own, "c").unwrap().reminders, Some(vec![5, 30]));
    let default = event(json!({
        "id": "r2",
        "start": {"dateTime": "2026-11-24T10:00:00Z"},
        "end": {"dateTime": "2026-11-24T11:00:00Z"},
        "reminders": {"useDefault": true},
    }));
    assert_eq!(to_event(&default, "c").unwrap().reminders, None);
    let c: CalendarResource = serde_json::from_value(json!({
        "id": "c", "defaultReminders": [{"method": "popup", "minutes": 10}],
    }))
    .unwrap();
    assert_eq!(to_calendar(&c).reminders, vec![10]);
}
