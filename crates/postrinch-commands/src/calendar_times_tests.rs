use postrinch_model::CalendarEvent;

use super::*;

fn at(s: &str) -> Timestamp {
    Timestamp::from_millis(s.parse::<jiff::Timestamp>().unwrap().as_millisecond())
}

fn event(id: &str, when: EventWhen) -> CalendarEvent {
    CalendarEvent {
        id: id.into(),
        calendar: "me@example.org".into(),
        summary: Some(id.into()),
        location: None,
        description: None,
        busy: true,
        when,
        organizer: None,
        attendees: Vec::new(),
        guest: false,
        answer: None,
        cancelled: false,
        repeating: false,
        link: None,
        reminders: None,
    }
}

fn record(id: &str, when: EventWhen, recurrence: &[&str], zone: Option<&str>) -> EventRecord {
    EventRecord {
        event: event(id, when),
        recurrence: recurrence.iter().map(|l| l.to_string()).collect(),
        time_zone: zone.map(str::to_owned),
        series: None,
        original_start: None,
    }
}

fn starts(found: &[CalendarEvent]) -> Vec<String> {
    found
        .iter()
        .map(|e| match e.when {
            EventWhen::Times { start, .. } => jiff::Timestamp::from_millisecond(start.as_millis())
                .unwrap()
                .to_string(),
            EventWhen::Days { first, .. } => {
                format!("{}-{:02}-{:02}", first.year, first.month, first.day)
            }
        })
        .collect()
}

/// Mondays at 09:00 in New York, which leaves summer time on 1 Nov 2026.
fn standup() -> EventRecord {
    record(
        "s1",
        EventWhen::Times {
            start: at("2026-10-19T13:00:00Z"),
            end: at("2026-10-19T13:15:00Z"),
        },
        &[
            "RRULE:FREQ=WEEKLY;BYDAY=MO",
            "EXDATE;TZID=America/New_York:20261026T090000",
        ],
        Some("America/New_York"),
    )
}

#[test]
fn a_series_keeps_its_wall_time_across_summer_time() {
    let found = between(
        &[standup()],
        at("2026-10-19T00:00:00Z"),
        at("2026-11-10T00:00:00Z"),
        &TimeZone::UTC,
    );
    // 26 Oct is left out; from November 09:00 is 14:00 UTC.
    assert_eq!(
        starts(&found),
        [
            "2026-10-19T13:00:00Z",
            "2026-11-02T14:00:00Z",
            "2026-11-09T14:00:00Z"
        ]
    );
    assert_eq!(found[1].id, "s1_20261102T140000Z");
    assert!(found.iter().all(|e| e.repeating));
    let EventWhen::Times { start, end } = found[1].when else {
        panic!()
    };
    assert_eq!(end.as_millis() - start.as_millis(), 15 * 60_000);
}

#[test]
fn changed_and_cancelled_times_stand_apart() {
    let mut moved = record(
        "s1_20261102T140000Z",
        EventWhen::Times {
            start: at("2026-11-02T16:00:00Z"),
            end: at("2026-11-02T16:30:00Z"),
        },
        &[],
        None,
    );
    moved.series = Some("s1".into());
    moved.original_start = Some(EventStart::At(at("2026-11-02T14:00:00Z")));
    let mut gone = record(
        "s1_20261109T140000Z",
        EventWhen::Times {
            start: at("2026-11-09T14:00:00Z"),
            end: at("2026-11-09T14:00:00Z"),
        },
        &[],
        None,
    );
    gone.series = Some("s1".into());
    gone.original_start = Some(EventStart::At(at("2026-11-09T14:00:00Z")));
    gone.event.cancelled = true;
    let found = between(
        &[standup(), moved, gone],
        at("2026-11-01T00:00:00Z"),
        at("2026-11-17T00:00:00Z"),
        &TimeZone::UTC,
    );
    assert_eq!(
        starts(&found),
        ["2026-11-02T16:00:00Z", "2026-11-16T14:00:00Z"]
    );
}

#[test]
fn whole_days_repeat_as_days() {
    let day = |month, day| Day {
        year: 2026,
        month,
        day,
    };
    let birthday = record(
        "b",
        EventWhen::Days {
            first: day(12, 10),
            last: day(12, 11),
        },
        &["RRULE:FREQ=YEARLY;COUNT=3", "RDATE;VALUE=DATE:20270601"],
        None,
    );
    let found = between(
        &[birthday],
        at("2026-01-01T00:00:00Z"),
        at("2040-01-01T00:00:00Z"),
        &TimeZone::UTC,
    );
    assert_eq!(
        starts(&found),
        ["2026-12-10", "2027-06-01", "2027-12-10", "2028-12-10"]
    );
    assert_eq!(found[0].id, "b_20261210");
    let EventWhen::Days { last, .. } = found[2].when else {
        panic!()
    };
    assert_eq!((last.month, last.day), (12, 11));
}

#[test]
fn single_events_are_found_by_overlap() {
    let lunch = record(
        "l",
        EventWhen::Times {
            start: at("2026-11-24T12:00:00Z"),
            end: at("2026-11-24T13:00:00Z"),
        },
        &[],
        None,
    );
    let tz = TimeZone::UTC;
    let found = |from, to| between(std::slice::from_ref(&lunch), at(from), at(to), &tz).len();
    assert_eq!(found("2026-11-24T12:30:00Z", "2026-11-24T14:00:00Z"), 1);
    assert_eq!(found("2026-11-24T13:00:00Z", "2026-11-24T14:00:00Z"), 0);
    assert_eq!(found("2026-11-24T10:00:00Z", "2026-11-24T12:00:00Z"), 0);
}

#[test]
fn a_split_series_names_its_times_as_google_does() {
    let when = EventWhen::Times {
        start: at("2026-11-25T23:30:00Z"),
        end: at("2026-11-26T00:30:00Z"),
    };
    assert_eq!(
        instance_id("70r3_R20260701T223000", &when),
        "70r3_20261125T233000Z"
    );
    assert_eq!(instance_id("70r3", &when), "70r3_20261125T233000Z");
    // Only a split's own suffix goes.
    assert_eq!(instance_id("a_Rx", &when), "a_Rx_20261125T233000Z");
}

#[test]
fn a_whole_day_series_may_end_on_a_bare_day() {
    let day = Day {
        year: 2020,
        month: 6,
        day: 3,
    };
    let anniversary = record(
        "a",
        EventWhen::Days {
            first: day,
            last: day,
        },
        &["RRULE:FREQ=YEARLY;UNTIL=20250603"],
        None,
    );
    let found = between(
        &[anniversary],
        at("2020-01-01T00:00:00Z"),
        at("2030-01-01T00:00:00Z"),
        &TimeZone::UTC,
    );
    // 2020 to 2025, its last.
    assert_eq!(found.len(), 6);
}
