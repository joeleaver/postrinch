//! Dates as the window shows them, in the person's time zone.

use jiff::tz::TimeZone;
use jiff::{Timestamp as Instant, Zoned};
use postrinch_model::Timestamp;

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
}
