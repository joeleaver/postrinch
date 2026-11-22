//! The times events happen between two moments: single events as they
//! are, and each time of a repeating series, worked out from its rule
//! (RFC 5545 `RRULE`, by the `rrule` crate) in the zone it repeats in, less
//! its `EXDATE`s, plus its `RDATE`s, with the times that were changed or
//! cancelled apart (they are records of their own).

use std::collections::HashSet;

use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use postrinch_model::{CalendarEvent, Day, EventRecord, EventStart, EventWhen, Timestamp};

/// A series yields at most this many times before the range ends.
const MOST: usize = 20_000;

/// Every time an event of `records` happens that overlaps `from`..`to`,
/// soonest first. Whole days count from midnight in `tz`.
pub(crate) fn between(
    records: &[EventRecord],
    from: Timestamp,
    to: Timestamp,
    tz: &TimeZone,
) -> Vec<CalendarEvent> {
    // The times of series that a record of their own stands for.
    let apart: HashSet<(&str, EventStart)> = records
        .iter()
        .filter_map(|r| Some((r.series.as_deref()?, r.original_start?)))
        .collect();
    let mut out = Vec::new();
    for record in records {
        if record.recurrence.is_empty() {
            if !record.event.cancelled && overlaps(&record.event.when, from, to, tz) {
                out.push(record.event.clone());
            }
            continue;
        }
        for when in times(record, to, tz) {
            if apart.contains(&(record.event.id.as_str(), when.start())) {
                continue;
            }
            if !overlaps(&when, from, to, tz) {
                continue;
            }
            let mut event = record.event.clone();
            event.id = instance_id(&record.event.id, &when);
            event.when = when;
            event.repeating = true;
            out.push(event);
        }
    }
    out.sort_by_key(|e| span(&e.when, tz).0);
    out
}

/// Whether `when` overlaps `from`..`to`. An event with no length counts
/// when it starts inside.
pub(crate) fn overlaps(when: &EventWhen, from: Timestamp, to: Timestamp, tz: &TimeZone) -> bool {
    let (start, end) = span(when, tz);
    if end <= start {
        return start >= from && start < to;
    }
    start < to && end > from
}

/// When an event starts and ends, as moments: whole days from midnight to
/// midnight in `tz`.
pub(crate) fn span(when: &EventWhen, tz: &TimeZone) -> (Timestamp, Timestamp) {
    match *when {
        EventWhen::Times { start, end } => (start, end),
        EventWhen::Days { first, last } => {
            let midnight = |d: Day| -> Timestamp {
                civil(d)
                    .and_then(|d| d.to_zoned(tz.clone()).ok())
                    .map_or(Timestamp::from_millis(0), |z| {
                        Timestamp::from_millis(z.timestamp().as_millisecond())
                    })
            };
            let after = civil(last)
                .and_then(|d| d.tomorrow().ok())
                .map_or(last, day);
            (midnight(first), midnight(after))
        }
    }
}

/// Google's id for one time of a series: the series' id, then the time
/// it starts, in UTC (`20261124T150000Z`) or as a day (`20261124`).
fn instance_id(series: &str, when: &EventWhen) -> String {
    let series = without_split(series);
    match *when {
        EventWhen::Days { first, .. } => format!(
            "{series}_{:04}{:02}{:02}",
            first.year, first.month, first.day
        ),
        EventWhen::Times { start, .. } => {
            let at = jiff::Timestamp::from_millisecond(start.as_millis())
                .unwrap_or(jiff::Timestamp::UNIX_EPOCH);
            format!("{series}_{}", at.strftime("%Y%m%dT%H%M%SZ"))
        }
    }
}

/// A series split at "this and following" is `<id>_R<when split>`; the
/// times of it are named after `<id>` alone.
fn without_split(series: &str) -> &str {
    match series.rsplit_once("_R") {
        Some((base, when))
            if !base.is_empty()
                && (when.len() == 8 || when.len() == 15)
                && when
                    .bytes()
                    .enumerate()
                    .all(|(i, b)| b.is_ascii_digit() || (i == 8 && b == b'T')) =>
        {
            base
        }
        _ => series,
    }
}

/// The times of a series that start before `to`.
fn times(record: &EventRecord, to: Timestamp, tz: &TimeZone) -> Vec<EventWhen> {
    let zone = record
        .time_zone
        .as_deref()
        .and_then(|z| TimeZone::get(z).ok())
        .unwrap_or_else(|| tz.clone());
    let rules: Vec<&str> = record
        .recurrence
        .iter()
        .map(String::as_str)
        .filter(|l| starts_with(l, "RRULE") || starts_with(l, "EXRULE"))
        .collect();
    let excluded: HashSet<EventStart> = dates(record, "EXDATE", &zone).into_iter().collect();
    let mut starts: Vec<EventStart> = dates(record, "RDATE", &zone);
    let first = record.event.when;
    starts.push(first.start());
    if !rules.is_empty() {
        starts.extend(by_rule(&first, &rules, &zone, to, record));
    }
    starts.sort();
    starts.dedup();
    starts
        .into_iter()
        .filter(|s| !excluded.contains(s))
        .map(|s| moved(&first, s))
        .collect()
}

/// The starts the series' rules give, until `to`.
fn by_rule(
    first: &EventWhen,
    rules: &[&str],
    zone: &TimeZone,
    to: Timestamp,
    record: &EventRecord,
) -> Vec<EventStart> {
    // Whole days repeat as midnights in UTC, read back as days.
    let (dtstart, day_series) = match *first {
        EventWhen::Days { first, .. } => (
            format!(
                "DTSTART:{:04}{:02}{:02}T000000Z",
                first.year, first.month, first.day
            ),
            true,
        ),
        EventWhen::Times { start, .. } => {
            let Some(local) = zoned(start, zone) else {
                return Vec::new();
            };
            let name = zone.iana_name().unwrap_or("UTC");
            (
                format!("DTSTART;TZID={name}:{}", local.strftime("%Y%m%dT%H%M%S")),
                false,
            )
        }
    };
    let rules: Vec<String> = rules.iter().map(|r| until_in_utc(r)).collect();
    let text = format!("{dtstart}\n{}", rules.join("\n"));
    let set: rrule::RRuleSet = match text.parse() {
        Ok(set) => set,
        Err(e) => {
            tracing::warn!(
                "event {}: its repeat rule does not read ({e}): {:?}",
                record.event.id,
                rules
            );
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for at in set.into_iter().take(MOST) {
        let millis = at.timestamp_millis();
        if millis >= to.as_millis() {
            break;
        }
        out.push(if day_series {
            let utc = jiff::Timestamp::from_millisecond(millis)
                .map(|t| t.to_zoned(TimeZone::UTC).date())
                .unwrap_or_default();
            EventStart::Day(day(utc))
        } else {
            EventStart::At(Timestamp::from_millis(millis))
        });
    }
    out
}

/// A rule whose `UNTIL` is a bare day (`UNTIL=20250603`, as Google writes
/// for whole-day series), with it as the end of that day in UTC: the rule
/// reader takes `UNTIL` only in UTC beside a start in a zone or in UTC.
fn until_in_utc(rule: &str) -> String {
    rule.split(';')
        .map(|part| match part.split_once('=') {
            Some((name, value))
                if name.eq_ignore_ascii_case("UNTIL")
                    && value.len() == 8
                    && value.bytes().all(|b| b.is_ascii_digit()) =>
            {
                format!("{name}={value}T235959Z")
            }
            _ => part.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// The dates on a series' `name` lines (`EXDATE`, `RDATE`): days, UTC
/// moments, or wall times in their `TZID` (or the series' zone).
fn dates(record: &EventRecord, name: &str, zone: &TimeZone) -> Vec<EventStart> {
    let mut out = Vec::new();
    for line in &record.recurrence {
        if !starts_with(line, name) {
            continue;
        }
        let Some((head, values)) = line.split_once(':') else {
            continue;
        };
        let line_zone = head
            .split(';')
            .find_map(|p| p.strip_prefix("TZID="))
            .and_then(|z| TimeZone::get(z).ok())
            .unwrap_or_else(|| zone.clone());
        for value in values.split(',').map(str::trim) {
            if let Some(start) = date_value(value, &line_zone) {
                out.push(start);
            }
        }
    }
    out
}

fn date_value(value: &str, zone: &TimeZone) -> Option<EventStart> {
    if value.len() == 8 {
        return Some(EventStart::Day(day(Date::strptime("%Y%m%d", value).ok()?)));
    }
    let (wall, z) = match value.strip_suffix('Z') {
        Some(utc) => (utc, TimeZone::UTC),
        None => (value, zone.clone()),
    };
    let at = DateTime::strptime("%Y%m%dT%H%M%S", wall).ok()?;
    let at = at.to_zoned(z).ok()?.timestamp();
    Some(EventStart::At(Timestamp::from_millis(at.as_millisecond())))
}

/// The series' first time, moved to start at `start`.
fn moved(first: &EventWhen, start: EventStart) -> EventWhen {
    match (*first, start) {
        (EventWhen::Times { start: s, end: e }, EventStart::At(at)) => EventWhen::Times {
            start: at,
            end: Timestamp::from_millis(at.as_millis() + (e.as_millis() - s.as_millis())),
        },
        (EventWhen::Days { first: f, last: l }, EventStart::Day(d)) => {
            let length = match (civil(f), civil(l)) {
                (Some(f), Some(l)) => f.until(l).map(|s| s.get_days()).unwrap_or(0),
                _ => 0,
            };
            let last = civil(d)
                .and_then(|c| c.checked_add(jiff::Span::new().days(length)).ok())
                .map_or(d, day);
            EventWhen::Days { first: d, last }
        }
        // A date of the other kind than the series: taken as it is.
        (_, EventStart::At(at)) => EventWhen::Times { start: at, end: at },
        (_, EventStart::Day(d)) => EventWhen::Days { first: d, last: d },
    }
}

fn starts_with(line: &str, name: &str) -> bool {
    line.get(..name.len())
        .is_some_and(|h| h.eq_ignore_ascii_case(name))
        && matches!(line.as_bytes().get(name.len()), Some(b':' | b';'))
}

fn zoned(at: Timestamp, tz: &TimeZone) -> Option<jiff::Zoned> {
    jiff::Timestamp::from_millisecond(at.as_millis())
        .ok()
        .map(|t| t.to_zoned(tz.clone()))
}

fn civil(d: Day) -> Option<Date> {
    Date::new(d.year, d.month, d.day).ok()
}

fn day(d: Date) -> Day {
    Day {
        year: d.year(),
        month: d.month(),
        day: d.day(),
    }
}

#[cfg(test)]
#[path = "calendar_times_tests.rs"]
mod tests;
