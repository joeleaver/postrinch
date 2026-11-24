//! Times relative to "now", so the store looks current on whatever day it
//! is seeded: mail from minutes and days ago, events in this week.

use jiff::civil::{Date, Time};
use jiff::{ToSpan, Zoned};
use postrinch_model::{Day, Timestamp};

/// When a message was sent, relative to now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At {
    /// So many minutes ago.
    MinutesAgo(i64),
    /// At a time of day, so many days ago (at least one).
    Day { days_ago: i64, hour: i8, minute: i8 },
}

/// `minutes` ago.
pub fn ago(minutes: i64) -> At {
    At::MinutesAgo(minutes)
}

/// At `hour:minute`, `days_ago` days before today.
pub fn day(days_ago: i64, hour: i8, minute: i8) -> At {
    At::Day {
        days_ago,
        hour,
        minute,
    }
}

/// Now, in the time zone the store is seeded for.
#[derive(Debug, Clone)]
pub struct Clock {
    now: Zoned,
}

impl Clock {
    pub fn new(now: Zoned) -> Self {
        Self { now }
    }

    pub fn now(&self) -> &Zoned {
        &self.now
    }

    /// The moment `at` stands for. A time of day that would fall after
    /// now (only possible across a change of clocks) is taken back to a
    /// minute before now, so nothing arrives from the future.
    pub fn resolve(&self, at: At) -> Zoned {
        let when = match at {
            At::MinutesAgo(minutes) => self.now.checked_sub(minutes.minutes()),
            At::Day {
                days_ago,
                hour,
                minute,
            } => self.on(self.today() - days_ago.days(), hour, minute),
        };
        let latest = self.now.checked_sub(1.minute()).expect("now is in range");
        match when {
            Ok(when) if when < latest => when,
            _ => latest,
        }
    }

    pub fn today(&self) -> Date {
        self.now.date()
    }

    /// Monday of this week.
    pub fn monday(&self) -> Date {
        let back = i64::from(self.today().weekday().to_monday_zero_offset());
        self.today() - back.days()
    }

    /// The day `offset` days from this week's Monday (0 is Monday, 6 is
    /// Sunday, negative is an earlier week).
    pub fn week_day(&self, offset: i64) -> Date {
        self.monday() + offset.days()
    }

    /// `hour:minute` on `date`, in the seeding time zone.
    pub fn on(&self, date: Date, hour: i8, minute: i8) -> Result<Zoned, jiff::Error> {
        date.to_datetime(Time::constant(hour, minute, 0, 0))
            .to_zoned(self.now.time_zone().clone())
    }

    /// The IANA name of the seeding time zone, when it has one.
    pub fn zone_name(&self) -> Option<String> {
        self.now.time_zone().iana_name().map(str::to_owned)
    }
}

/// The model's instant for a zoned time.
pub fn timestamp(when: &Zoned) -> Timestamp {
    Timestamp::from_millis(when.timestamp().as_millisecond())
}

/// The model's day for a date.
pub fn model_day(date: Date) -> Day {
    Day {
        year: date.year(),
        month: date.month(),
        day: date.day(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock() -> Clock {
        // A Wednesday afternoon.
        Clock::new("2026-11-25T15:30[America/Chicago]".parse().unwrap())
    }

    #[test]
    fn weeks_start_on_monday() {
        let c = clock();
        assert_eq!(c.monday().to_string(), "2026-11-23");
        assert_eq!(c.week_day(6).to_string(), "2026-11-29");
    }

    #[test]
    fn nothing_arrives_from_the_future() {
        let c = clock();
        assert_eq!(
            c.resolve(ago(30)).to_string(),
            "2026-11-25T15:00:00-06:00[America/Chicago]"
        );
        assert_eq!(
            c.resolve(day(2, 9, 5)).to_string(),
            "2026-11-23T09:05:00-06:00[America/Chicago]"
        );
        assert!(c.resolve(ago(0)) < *c.now());
    }
}
