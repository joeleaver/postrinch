use postrinch_model::{
    Account, CalendarEvent, CalendarInfo, Day, EventRecord, EventWhen, Provider, Timestamp,
};
use postrinch_store::Store;

use super::*;

fn at(s: &str) -> Timestamp {
    Timestamp::from_millis(s.parse::<jiff::Timestamp>().unwrap().as_millisecond())
}

fn record(id: &str, when: EventWhen, reminders: Option<Vec<i64>>) -> EventRecord {
    EventRecord {
        event: CalendarEvent {
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
            reminders,
        },
        recurrence: Vec::new(),
        time_zone: None,
        series: None,
        original_start: None,
    }
}

fn timed(id: &str, start: &str, reminders: Option<Vec<i64>>) -> EventRecord {
    let s = at(start);
    record(
        id,
        EventWhen::Times {
            start: s,
            end: Timestamp::from_millis(s.as_millis() + 3_600_000),
        },
        reminders,
    )
}

#[test]
fn reminders_fall_due_by_the_event_its_calendar_or_the_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let me = EmailAddress::parse("me@example.org").unwrap();
    let other = EmailAddress::parse("other@example.org").unwrap();
    for a in [&me, &other] {
        store
            .put_account(&Account::new(a.clone(), Provider::Google))
            .unwrap();
    }
    let calendar = |id: &str, reminders: Vec<i64>| CalendarInfo {
        id: id.into(),
        name: id.into(),
        color: None,
        primary: true,
        writable: true,
        role: Default::default(),
        shown: true,
        time_zone: None,
        reminders,
    };
    store
        .put_calendar(&me, &calendar("me@example.org", vec![10]))
        .unwrap();
    store
        .put_calendar(&me, &calendar("quiet", vec![10]))
        .unwrap();
    store
        .put_calendar(&other, &calendar("other@example.org", vec![10]))
        .unwrap();
    let mut declined = timed("declined", "2026-11-24T10:00:00Z", None);
    declined.event.answer = Some(Answer::No);
    let mut none = timed("none", "2026-11-24T10:00:00Z", Some(Vec::new()));
    none.event.calendar = "me@example.org".into();
    for r in [
        timed("own", "2026-11-24T10:00:00Z", Some(vec![5, 30])),
        timed("default", "2026-11-24T10:00:00Z", None),
        none,
        declined,
        record(
            "day",
            EventWhen::Days {
                first: Day {
                    year: 2026,
                    month: 11,
                    day: 24,
                },
                last: Day {
                    year: 2026,
                    month: 11,
                    day: 24,
                },
            },
            None,
        ),
    ] {
        store.put_event(&me, "me@example.org", &r).unwrap();
    }
    let mut hushed = timed("hushed", "2026-11-24T10:00:00Z", None);
    hushed.event.calendar = "quiet".into();
    store.put_event(&me, "quiet", &hushed).unwrap();
    // The same event in the other account's calendar reminds once.
    let mut copy = timed("own", "2026-11-24T10:00:00Z", Some(vec![5, 30]));
    copy.event.calendar = "other@example.org".into();
    store.put_event(&other, "other@example.org", &copy).unwrap();

    let commands = Commands::new(store);
    let rules = ReminderRules {
        quiet_calendars: [format!("{}\u{1f}quiet", me.key())].into_iter().collect(),
    };
    let tz = TimeZone::system();
    let midnight = jiff::civil::date(2026, 11, 24)
        .at(0, 0, 0, 0)
        .to_zoned(tz)
        .unwrap()
        .timestamp();
    let due = commands
        .reminders_due(
            at("2026-11-23T00:00:00Z"),
            at("2026-11-24T23:59:00Z"),
            &rules,
        )
        .unwrap();
    let mut got: Vec<(String, i64)> = due
        .iter()
        .map(|d| (d.happening.event.id.clone(), d.minutes))
        .collect();
    got.sort();
    assert_eq!(
        got,
        [
            ("day".to_owned(), 10),
            ("default".to_owned(), 10),
            ("own".to_owned(), 5),
            ("own".to_owned(), 30),
        ]
    );
    let day = due.iter().find(|d| d.happening.event.id == "day").unwrap();
    // Its calendar's ten minutes, back from its midnight.
    assert_eq!(day.at.as_millis(), midnight.as_millisecond() - 10 * 60_000);
    // Nothing falls due twice: the next stretch starts where this ended.
    let later = commands
        .reminders_due(
            at("2026-11-24T23:59:00Z"),
            at("2026-11-25T23:59:00Z"),
            &rules,
        )
        .unwrap();
    assert!(later.is_empty(), "{later:?}");
}
