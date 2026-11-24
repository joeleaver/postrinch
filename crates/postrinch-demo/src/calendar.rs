//! Each account's calendars, and a week of events in them as calendar
//! sync keeps them: this week's, some overlapping, one repeating, one
//! over a whole day, and the invitation the work inbox holds, not yet
//! answered.

use jiff::Zoned;
use postrinch_model::{
    Answer, Attendee, CalendarEvent, CalendarInfo, CalendarRole, EventRecord, EventWhen,
};
use postrinch_store::Store;

use crate::DemoResult;
use crate::clock::{Clock, model_day, timestamp};
use crate::people::{
    INES, JONAH, MARCUS, MAYA, OLLIE, PRIYA, Person, RUTH, SAM_HOME, SAM_WORK, THEO,
};

const FAMILY: &str = "family.demo";
const STUDIO: &str = "studio.demo";

/// The invitation's event: the id Sam's calendar gives it, and the
/// `UID` its organizer's invitation carries.
pub const WORKSHOP_ID: &str = "hcpackaging2026";
pub const WORKSHOP_UID: &str = "hcpackaging2026@harborcoffee.example";
pub const WORKSHOP_TITLE: &str = "Harbor Coffee: packaging workshop";
pub const WORKSHOP_PLACE: &str = "Harbor Coffee roastery, 14 Wharf Lane";

/// When the workshop is: this Thursday, 14:00 to 15:30.
pub fn workshop(clock: &Clock) -> DemoResult<(Zoned, Zoned)> {
    let day = clock.week_day(3);
    Ok((clock.on(day, 14, 0)?, clock.on(day, 15, 30)?))
}

/// The invitation Priya sends for the workshop, as iCalendar text.
pub fn workshop_invitation(clock: &Clock) -> DemoResult<String> {
    let (start, end) = workshop(clock)?;
    let utc = |z: &Zoned| z.timestamp().strftime("%Y%m%dT%H%M%SZ").to_string();
    let stamp = utc(&clock.resolve(crate::clock::ago(90)));
    let lines = [
        "BEGIN:VCALENDAR".to_owned(),
        "PRODID:-//Harbor Coffee//Demo//EN".to_owned(),
        "VERSION:2.0".to_owned(),
        "METHOD:REQUEST".to_owned(),
        "BEGIN:VEVENT".to_owned(),
        format!("UID:{WORKSHOP_UID}"),
        format!("DTSTAMP:{stamp}"),
        format!("DTSTART:{}", utc(&start)),
        format!("DTEND:{}", utc(&end)),
        format!("SUMMARY:{WORKSHOP_TITLE}"),
        format!("LOCATION:{}", WORKSHOP_PLACE.replace(',', "\\,")),
        format!("ORGANIZER;CN={}:mailto:{}", PRIYA.name, PRIYA.address),
        format!(
            "ATTENDEE;CN={};PARTSTAT=ACCEPTED;ROLE=REQ-PARTICIPANT:mailto:{}",
            PRIYA.name, PRIYA.address
        ),
        format!(
            "ATTENDEE;CN={};PARTSTAT=ACCEPTED;ROLE=REQ-PARTICIPANT:mailto:{}",
            OLLIE.name, OLLIE.address
        ),
        format!(
            "ATTENDEE;CN={};PARTSTAT=NEEDS-ACTION;RSVP=TRUE;ROLE=REQ-PARTICIPANT:mailto:{}",
            SAM_WORK.name, SAM_WORK.address
        ),
        format!(
            "ATTENDEE;CN={};PARTSTAT=ACCEPTED;ROLE=REQ-PARTICIPANT:mailto:{}",
            THEO.name, THEO.address
        ),
        "SEQUENCE:0".to_owned(),
        "STATUS:CONFIRMED".to_owned(),
        "END:VEVENT".to_owned(),
        "END:VCALENDAR".to_owned(),
    ];
    Ok(lines.iter().map(|l| fold(l)).collect())
}

/// One content line, folded as RFC 5545 §3.1 asks: no line over 75
/// octets, each continuation starting with a space, never inside a
/// character.
fn fold(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 8);
    let mut width = 0;
    for c in line.chars() {
        if width + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += c.len_utf8();
    }
    out.push_str("\r\n");
    out
}

fn calendar(
    id: &str,
    name: &str,
    color: &str,
    primary: bool,
    role: CalendarRole,
    zone: Option<String>,
) -> CalendarInfo {
    CalendarInfo {
        id: id.to_owned(),
        name: name.to_owned(),
        color: Some(color.to_owned()),
        primary,
        writable: role != CalendarRole::Reader && role != CalendarRole::FreeBusy,
        role,
        shown: true,
        time_zone: zone,
        reminders: vec![10],
    }
}

/// An event of the account's own, with no guests.
fn event(id: &str, calendar: &str, summary: &str, when: EventWhen) -> CalendarEvent {
    CalendarEvent {
        id: id.to_owned(),
        calendar: calendar.to_owned(),
        summary: Some(summary.to_owned()),
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

fn at(location: &str, mut event: CalendarEvent) -> CalendarEvent {
    event.location = Some(location.to_owned());
    event
}

/// A meeting `organizer` called with `guests`, everyone's answer yes. Sam
/// is a guest at one Sam did not call.
fn meeting(organizer: Person, guests: &[Person], mut event: CalendarEvent) -> CalendarEvent {
    event.organizer = Some(organizer.mailbox());
    event.guest = ![SAM_HOME.address, SAM_WORK.address].contains(&organizer.address);
    event.attendees = std::iter::once(organizer)
        .chain(guests.iter().copied())
        .map(|p| Attendee {
            mailbox: p.mailbox(),
            answer: Some(Answer::Yes),
        })
        .collect();
    event.answer = Some(Answer::Yes);
    event
}

struct Week<'a> {
    clock: &'a Clock,
}

impl Week<'_> {
    /// `day` (0 is this Monday) from `start` to `end`, each `(hour, minute)`.
    fn times(&self, day: i64, start: (i8, i8), end: (i8, i8)) -> DemoResult<EventWhen> {
        let date = self.clock.week_day(day);
        Ok(EventWhen::Times {
            start: timestamp(&self.clock.on(date, start.0, start.1)?),
            end: timestamp(&self.clock.on(date, end.0, end.1)?),
        })
    }

    fn days(&self, first: i64, last: i64) -> EventWhen {
        let day = |n| model_day(self.clock.week_day(n));
        EventWhen::Days {
            first: day(first),
            last: day(last),
        }
    }
}

fn record(event: CalendarEvent, clock: &Clock) -> EventRecord {
    EventRecord {
        event,
        recurrence: Vec::new(),
        time_zone: clock.zone_name(),
        series: None,
        original_start: None,
    }
}

/// Store both accounts' calendars and events. Returns how many events.
pub fn put(store: &Store, clock: &Clock) -> DemoResult<usize> {
    let zone = clock.zone_name();
    let week = Week { clock };
    let home = SAM_HOME.email();
    let work = SAM_WORK.email();
    let home_primary = SAM_HOME.address;
    let work_primary = SAM_WORK.address;
    store.put_calendar(
        &home,
        &calendar(
            home_primary,
            SAM_HOME.name,
            "#7986cb",
            true,
            CalendarRole::Owner,
            zone.clone(),
        ),
    )?;
    store.put_calendar(
        &home,
        &calendar(
            FAMILY,
            "Family",
            "#33b679",
            false,
            CalendarRole::Writer,
            zone.clone(),
        ),
    )?;
    store.put_calendar(
        &work,
        &calendar(
            work_primary,
            SAM_WORK.name,
            "#f4511e",
            true,
            CalendarRole::Owner,
            zone.clone(),
        ),
    )?;
    store.put_calendar(
        &work,
        &calendar(
            STUDIO,
            "Studio",
            "#8e24aa",
            false,
            CalendarRole::Writer,
            zone,
        ),
    )?;

    let mut standup = record(
        at(
            "Studio, upstairs",
            meeting(
                INES,
                &[SAM_WORK, THEO],
                event(
                    "standup",
                    STUDIO,
                    "Studio stand-up",
                    week.times(-14, (9, 30), (9, 45))?,
                ),
            ),
        ),
        clock,
    );
    standup.recurrence = vec!["RRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR".to_owned()];

    let (start, end) = workshop(clock)?;
    let mut workshop = at(
        WORKSHOP_PLACE,
        event(
            WORKSHOP_ID,
            work_primary,
            WORKSHOP_TITLE,
            EventWhen::Times {
                start: timestamp(&start),
                end: timestamp(&end),
            },
        ),
    );
    workshop.organizer = Some(PRIYA.mailbox());
    workshop.attendees = [
        (PRIYA, Some(Answer::Yes)),
        (OLLIE, Some(Answer::Yes)),
        (SAM_WORK, None),
        (THEO, Some(Answer::Yes)),
    ]
    .into_iter()
    .map(|(p, answer)| Attendee {
        mailbox: p.mailbox(),
        answer,
    })
    .collect();
    workshop.guest = true;
    workshop.description =
        Some("Bring the cup sleeve mock-ups and the two box options.".to_owned());

    let work_events = [
        standup,
        record(
            at(
                "Video call",
                meeting(
                    SAM_WORK,
                    &[PRIYA, THEO],
                    event(
                        "hclogoreview",
                        work_primary,
                        "Harbor Coffee: logo review",
                        week.times(0, (13, 0), (14, 0))?,
                    ),
                ),
            ),
            clock,
        ),
        record(
            event(
                "critique",
                STUDIO,
                "Portfolio critique",
                week.times(1, (10, 0), (11, 0))?,
            ),
            clock,
        ),
        record(
            at(
                "Video call",
                meeting(
                    SAM_WORK,
                    &[MARCUS],
                    event(
                        "fernhillcall",
                        work_primary,
                        "Call with Fernhill Bakery",
                        week.times(1, (10, 30), (11, 30))?,
                    ),
                ),
            ),
            clock,
        ),
        record(
            at(
                "Juniper Café",
                meeting(
                    SAM_WORK,
                    &[THEO],
                    event(
                        "lunchtheo",
                        work_primary,
                        "Lunch with Theo",
                        week.times(2, (12, 0), (13, 0))?,
                    ),
                ),
            ),
            clock,
        ),
        record(workshop, clock),
        record(
            event(
                "focus",
                work_primary,
                "Focus: menu boards",
                week.times(3, (9, 0), (11, 30))?,
            ),
            clock,
        ),
        record(
            at(
                "Cedar Lodge",
                event("offsite", STUDIO, "Studio offsite", week.days(4, 4)),
            ),
            clock,
        ),
        record(
            event(
                "invoices",
                work_primary,
                "Invoice run",
                week.times(4, (16, 0), (16, 45))?,
            ),
            clock,
        ),
    ];
    let family = |id, summary, when| record(event(id, FAMILY, summary, when), clock);
    let home_events = [
        record(
            at(
                "Brightside Dental",
                event(
                    "dentist",
                    home_primary,
                    "Dentist",
                    week.times(2, (8, 0), (8, 45))?,
                ),
            ),
            clock,
        ),
        record(
            at(
                "Boulder Barn",
                meeting(
                    SAM_HOME,
                    &[JONAH],
                    event(
                        "climbing",
                        home_primary,
                        "Climbing with Jonah",
                        week.times(1, (18, 30), (20, 0))?,
                    ),
                ),
            ),
            clock,
        ),
        record(
            at(
                "Maya's place",
                meeting(
                    MAYA,
                    &[SAM_HOME, JONAH],
                    event(
                        "bookclub",
                        home_primary,
                        "Book club",
                        week.times(3, (19, 0), (21, 0))?,
                    ),
                ),
            ),
            clock,
        ),
        family("ruthbirthday", "Ruth's birthday", week.days(5, 5)),
        family(
            "market",
            "Farmers market",
            week.times(6, (10, 0), (11, 30))?,
        ),
        record(
            at(
                "Ruth's",
                meeting(
                    RUTH,
                    &[SAM_HOME],
                    event(
                        "sundaydinner",
                        FAMILY,
                        "Sunday dinner",
                        week.times(6, (17, 30), (20, 0))?,
                    ),
                ),
            ),
            clock,
        ),
    ];
    let mut count = 0;
    for (account, events) in [(&work, &work_events[..]), (&home, &home_events[..])] {
        for record in events {
            store.put_event(account, &record.event.calendar, record)?;
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::fold;

    #[test]
    fn long_lines_fold_and_unfold_whole() {
        let line = format!(
            "ATTENDEE;CN=Inês Duarte;PARTSTAT=NEEDS-ACTION:mailto:{}",
            "a".repeat(80)
        );
        let folded = fold(&line);
        assert!(folded.ends_with("\r\n"));
        for part in folded.trim_end_matches("\r\n").split("\r\n") {
            assert!(part.len() <= 75, "{part}");
        }
        assert_eq!(folded.trim_end_matches("\r\n").replace("\r\n ", ""), line);
        assert_eq!(fold("VERSION:2.0"), "VERSION:2.0\r\n");
    }
}
