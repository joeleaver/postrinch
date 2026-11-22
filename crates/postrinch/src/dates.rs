//! Dates as the window shows them, in the person's time zone.

use jiff::tz::TimeZone;
use jiff::{Timestamp as Instant, Zoned};
use postrinch_model::{Day, EventWhen, Timestamp};

/// A list row's date: the time today, the weekday this week, the day and
/// month this year, and the year as well before that.
pub fn short(at: Timestamp, now: &Zoned) -> String {
    let Some(at) = zoned(at, now.time_zone()) else {
        return String::new();
    };
    let (today, day) = (now.date(), at.date());
    if day == today {
        at.strftime("%H:%M").to_string()
    } else if day < today && today.since(day).is_ok_and(|span| span.get_days() < 7) {
        at.strftime("%a").to_string()
    } else if day.year() == today.year() {
        at.strftime("%-d %b").to_string()
    } else {
        at.strftime("%-d %b %Y").to_string()
    }
}

/// A message's date in full: "Mon 3 Nov 2026, 14:05".
pub fn long(at: Timestamp, tz: &TimeZone) -> String {
    zoned(at, tz)
        .map(|at| at.strftime("%a %-d %b %Y, %H:%M").to_string())
        .unwrap_or_default()
}

/// When an event is, in full: "Tue 24 Nov 2026, 10:00 – 11:00", "Thu 24
/// Dec 2026", "Thu 24 Dec – Sat 26 Dec 2026", or across days with times
/// on both.
pub fn event(when: &EventWhen, tz: &TimeZone) -> String {
    match *when {
        EventWhen::Days { first, last } => {
            let (Some(first), Some(last)) = (civil(first), civil(last)) else {
                return String::new();
            };
            if first == last {
                first.strftime("%a %-d %b %Y").to_string()
            } else if first.year() == last.year() {
                format!(
                    "{} \u{2013} {}",
                    first.strftime("%a %-d %b"),
                    last.strftime("%a %-d %b %Y")
                )
            } else {
                format!(
                    "{} \u{2013} {}",
                    first.strftime("%a %-d %b %Y"),
                    last.strftime("%a %-d %b %Y")
                )
            }
        }
        EventWhen::Times { start, end } => {
            let (Some(start), Some(end)) = (zoned(start, tz), zoned(end, tz)) else {
                return String::new();
            };
            let from = start.strftime("%a %-d %b %Y, %H:%M");
            if end <= start {
                from.to_string()
            } else if end.date() == start.date() {
                format!("{from} \u{2013} {}", end.strftime("%H:%M"))
            } else {
                format!("{from} \u{2013} {}", end.strftime("%a %-d %b %Y, %H:%M"))
            }
        }
    }
}

/// An event's first day, as a calendar page shows it: ("NOV", "24").
pub fn event_page(when: &EventWhen, tz: &TimeZone) -> (String, String) {
    let day = match *when {
        EventWhen::Days { first, .. } => civil(first),
        EventWhen::Times { start, .. } => zoned(start, tz).map(|z| z.date()),
    };
    day.map(|d| {
        (
            d.strftime("%b").to_string().to_uppercase(),
            d.day().to_string(),
        )
    })
    .unwrap_or_default()
}

fn civil(d: Day) -> Option<jiff::civil::Date> {
    jiff::civil::Date::new(d.year, d.month, d.day).ok()
}

/// Now, in the person's time zone.
pub fn now() -> Zoned {
    Zoned::now()
}

fn zoned(at: Timestamp, tz: &TimeZone) -> Option<Zoned> {
    Instant::from_millisecond(at.as_millis())
        .ok()
        .map(|t| t.to_zoned(tz.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Timestamp {
        let t: Instant = s.parse().unwrap();
        Timestamp::from_millis(t.as_millisecond())
    }

    fn now(s: &str) -> Zoned {
        s.parse::<Instant>().unwrap().to_zoned(TimeZone::UTC)
    }

    #[test]
    fn shortens_by_how_long_ago() {
        let now = now("2026-11-03T15:00:00Z");
        assert_eq!(short(at("2026-11-03T09:30:00Z"), &now), "09:30");
        assert_eq!(short(at("2026-10-31T09:30:00Z"), &now), "Sat");
        assert_eq!(short(at("2026-10-12T09:30:00Z"), &now), "12 Oct");
        assert_eq!(short(at("2025-08-01T09:30:00Z"), &now), "1 Aug 2025");
    }

    #[test]
    fn long_dates_say_everything() {
        assert_eq!(
            long(at("2026-11-02T14:05:00Z"), &TimeZone::UTC),
            "Mon 2 Nov 2026, 14:05"
        );
    }

    #[test]
    fn events_say_when_without_saying_it_twice() {
        let tz = TimeZone::UTC;
        let times = |a, b| EventWhen::Times {
            start: at(a),
            end: at(b),
        };
        assert_eq!(
            event(&times("2026-11-24T10:00:00Z", "2026-11-24T11:00:00Z"), &tz),
            "Tue 24 Nov 2026, 10:00 \u{2013} 11:00"
        );
        assert_eq!(
            event(&times("2026-11-24T22:00:00Z", "2026-11-25T01:00:00Z"), &tz),
            "Tue 24 Nov 2026, 22:00 \u{2013} Wed 25 Nov 2026, 01:00"
        );
        let d = |month, day| Day {
            year: 2026,
            month,
            day,
        };
        let days = |first, last| EventWhen::Days { first, last };
        assert_eq!(event(&days(d(12, 24), d(12, 24)), &tz), "Thu 24 Dec 2026");
        assert_eq!(
            event(&days(d(12, 24), d(12, 26)), &tz),
            "Thu 24 Dec \u{2013} Sat 26 Dec 2026"
        );
        assert_eq!(
            event_page(&days(d(12, 24), d(12, 26)), &tz),
            ("DEC".to_owned(), "24".to_owned())
        );
    }
}
