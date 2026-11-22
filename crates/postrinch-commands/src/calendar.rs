//! Calendars as kept here: each account's calendars, the events between
//! two moments, and when the person is free.

use jiff::civil::Time;
use jiff::tz::TimeZone;
use postrinch_model::{Answer, CalendarEvent, CalendarInfo, EmailAddress, EventWhen, Timestamp};

use crate::calendar_times::{between, span};
use crate::{CommandError, CommandResult, Commands};

/// The longest stretch the calendar is asked about at once.
const LONGEST_DAYS: i64 = 400;

/// An event at one of its times, in one of an account's calendars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Happening {
    pub account: EmailAddress,
    pub event: CalendarEvent,
}

/// The working day free time is looked for in, in the person's zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hours {
    pub from: Time,
    pub to: Time,
}

impl Commands {
    /// Each account's calendars, as last synced.
    pub fn calendars(&self) -> CommandResult<Vec<(EmailAddress, CalendarInfo)>> {
        let mut out = Vec::new();
        for account in self.visible_accounts()? {
            for info in self.store.calendars(&account.address)? {
                out.push((account.address.clone(), info));
            }
        }
        Ok(out)
    }

    /// The events of `accounts` (every account seen, when empty) that
    /// overlap `from`..`to`, soonest first, each repeating one at each of
    /// its times.
    pub fn events_between(
        &self,
        accounts: &[EmailAddress],
        from: Timestamp,
        to: Timestamp,
    ) -> CommandResult<Vec<Happening>> {
        check_range(from, to)?;
        let tz = TimeZone::system();
        let mut out = Vec::new();
        for account in self.asked(accounts)? {
            let records: Vec<_> = self
                .store
                .events(&account)?
                .into_iter()
                .map(|(_, r)| r)
                .collect();
            out.extend(
                between(&records, from, to, &tz)
                    .into_iter()
                    .map(|event| Happening {
                        account: account.clone(),
                        event,
                    }),
            );
        }
        out.sort_by_key(|h| span(&h.event.when, &tz).0);
        Ok(out)
    }

    /// The stretches of at least `minutes` in `from`..`to` that no event of
    /// `accounts`' own calendars makes busy (as Google's free/busy counts:
    /// the account's main calendar, events that make it busy, not those it
    /// declined, not whole days), within `hours` of each day when given.
    pub fn free_time(
        &self,
        accounts: &[EmailAddress],
        from: Timestamp,
        to: Timestamp,
        minutes: i64,
        hours: Option<Hours>,
    ) -> CommandResult<Vec<(Timestamp, Timestamp)>> {
        let tz = TimeZone::system();
        let mut busy: Vec<(i64, i64)> = Vec::new();
        for happening in self.events_between(accounts, from, to)? {
            let e = &happening.event;
            let own = e.calendar == happening.account.to_string();
            let counts = own
                && e.busy
                && !e.cancelled
                && e.answer != Some(Answer::No)
                && matches!(e.when, EventWhen::Times { .. });
            if counts {
                let (s, end) = span(&e.when, &tz);
                busy.push((s.as_millis(), end.as_millis()));
            }
        }
        let windows = match hours {
            Some(hours) => working_windows(from, to, hours, &tz),
            None => vec![(from.as_millis(), to.as_millis())],
        };
        Ok(free_in(windows, busy, minutes.max(1) * 60_000)
            .into_iter()
            .map(|(s, e)| (Timestamp::from_millis(s), Timestamp::from_millis(e)))
            .collect())
    }

    /// The accounts asked about: those given, each seen, or every one.
    fn asked(&self, accounts: &[EmailAddress]) -> CommandResult<Vec<EmailAddress>> {
        if accounts.is_empty() {
            return Ok(self
                .visible_accounts()?
                .into_iter()
                .map(|a| a.address)
                .collect());
        }
        for account in accounts {
            self.check(account)?;
        }
        Ok(accounts.to_vec())
    }
}

fn check_range(from: Timestamp, to: Timestamp) -> CommandResult<()> {
    let days = (to.as_millis() - from.as_millis()) / 86_400_000;
    if to <= from {
        return Err(CommandError::Invalid(
            "The end comes before the start.".into(),
        ));
    }
    if days > LONGEST_DAYS {
        return Err(CommandError::Invalid(format!(
            "Ask about at most {LONGEST_DAYS} days at a time."
        )));
    }
    Ok(())
}

/// Each day's working hours between `from` and `to`, in `tz`.
fn working_windows(from: Timestamp, to: Timestamp, hours: Hours, tz: &TimeZone) -> Vec<(i64, i64)> {
    let Ok(start) = jiff::Timestamp::from_millisecond(from.as_millis()) else {
        return Vec::new();
    };
    let mut day = start.to_zoned(tz.clone()).date();
    let mut out = Vec::new();
    loop {
        let at = |t: Time| {
            day.to_datetime(t)
                .to_zoned(tz.clone())
                .map(|z| z.timestamp().as_millisecond())
        };
        let (Ok(open), Ok(close)) = (at(hours.from), at(hours.to)) else {
            break;
        };
        if open >= to.as_millis() {
            break;
        }
        let (s, e) = (open.max(from.as_millis()), close.min(to.as_millis()));
        if s < e {
            out.push((s, e));
        }
        match day.tomorrow() {
            Ok(next) => day = next,
            Err(_) => break,
        }
    }
    out
}

/// What of `windows` no busy stretch covers, in pieces of at least `least`.
fn free_in(windows: Vec<(i64, i64)>, mut busy: Vec<(i64, i64)>, least: i64) -> Vec<(i64, i64)> {
    busy.sort();
    let mut out = Vec::new();
    for (start, end) in windows {
        let mut at = start;
        for &(b, e) in &busy {
            if e <= at || b >= end {
                continue;
            }
            if b > at && b - at >= least {
                out.push((at, b));
            }
            at = at.max(e);
            if at >= end {
                break;
            }
        }
        if end - at >= least {
            out.push((at, end));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_time_is_what_busy_leaves() {
        let h = 3_600_000;
        let free = free_in(
            vec![(9 * h, 17 * h)],
            vec![
                (10 * h, 11 * h),
                (10 * h + h / 2, 12 * h),
                (16 * h + h / 2, 18 * h),
            ],
            30 * 60_000,
        );
        assert_eq!(free, [(9 * h, 10 * h), (12 * h, 16 * h + h / 2)]);
        // Too short a gap is not offered.
        let free = free_in(vec![(9 * h, 10 * h)], vec![(9 * h + h / 4, 10 * h)], h / 2);
        assert!(free.is_empty());
    }

    #[test]
    fn working_hours_each_day() {
        let at = |s: &str| {
            Timestamp::from_millis(s.parse::<jiff::Timestamp>().unwrap().as_millisecond())
        };
        let hours = Hours {
            from: Time::constant(9, 0, 0, 0),
            to: Time::constant(17, 0, 0, 0),
        };
        let windows = working_windows(
            at("2026-11-24T12:00:00Z"),
            at("2026-11-26T10:00:00Z"),
            hours,
            &TimeZone::UTC,
        );
        let h = 3_600_000;
        let day = |d: i64| at("2026-11-24T00:00:00Z").as_millis() + d * 24 * h;
        assert_eq!(
            windows,
            [
                (day(0) + 12 * h, day(0) + 17 * h),
                (day(1) + 9 * h, day(1) + 17 * h),
                (day(2) + 9 * h, day(2) + 10 * h),
            ]
        );
    }
}
