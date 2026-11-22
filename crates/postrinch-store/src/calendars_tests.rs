use postrinch_model::{CalendarEvent, Day, EventWhen};

use super::*;

fn info(id: &str) -> CalendarInfo {
    CalendarInfo {
        id: id.into(),
        name: id.into(),
        color: None,
        primary: id == "me@example.com",
        writable: true,
        shown: true,
        time_zone: Some("Europe/London".into()),
    }
}

fn record(id: &str) -> EventRecord {
    let day = Day {
        year: 2026,
        month: 12,
        day: 24,
    };
    EventRecord {
        event: CalendarEvent {
            id: id.into(),
            calendar: "me@example.com".into(),
            summary: Some(id.into()),
            location: None,
            description: None,
            busy: true,
            when: EventWhen::Days {
                first: day,
                last: day,
            },
            organizer: None,
            attendees: Vec::new(),
            guest: false,
            answer: None,
            cancelled: false,
            repeating: false,
            link: None,
        },
        recurrence: Vec::new(),
        time_zone: None,
        series: None,
        original_start: None,
    }
}

#[test]
fn calendars_and_events_are_kept_per_account() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let me = EmailAddress::parse("me@example.com").unwrap();
    let other = EmailAddress::parse("other@example.com").unwrap();
    store.put_calendar(&me, &info("me@example.com")).unwrap();
    store.put_calendar(&me, &info("holidays")).unwrap();
    store
        .put_calendar(&other, &info("other@example.com"))
        .unwrap();
    store
        .set_calendar_sync_token(&me, "holidays", Some("t1"))
        .unwrap();
    // Listing it again keeps where its sync got to.
    store.put_calendar(&me, &info("holidays")).unwrap();
    assert_eq!(
        store
            .calendar_sync_token(&me, "holidays")
            .unwrap()
            .as_deref(),
        Some("t1")
    );
    assert_eq!(store.calendars(&me).unwrap().len(), 2);

    store
        .put_event(&me, "me@example.com", &record("a"))
        .unwrap();
    store.put_event(&me, "holidays", &record("b")).unwrap();
    store
        .put_event(&other, "other@example.com", &record("c"))
        .unwrap();
    let mut changed = record("a");
    changed.event.summary = Some("A, changed".into());
    store.put_event(&me, "me@example.com", &changed).unwrap();
    let mut mine = store.events(&me).unwrap();
    mine.sort_by(|x, y| x.1.event.id.cmp(&y.1.event.id));
    let shown: Vec<_> = mine
        .iter()
        .map(|(c, r)| (c.as_str(), r.event.summary.as_deref().unwrap()))
        .collect();
    assert_eq!(shown, [("me@example.com", "A, changed"), ("holidays", "b")]);

    store.remove_event(&me, "me@example.com", "a").unwrap();
    store.remove_calendar(&me, "holidays").unwrap();
    assert!(store.events(&me).unwrap().is_empty());
    assert_eq!(store.calendars(&me).unwrap().len(), 1);
    assert_eq!(store.events(&other).unwrap().len(), 1);
}
