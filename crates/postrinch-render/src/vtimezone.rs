//! A time zone as an invitation defines it (`VTIMEZONE`): its standard and
//! summer offsets and the yearly rules for changing between them. Outlook
//! names zones its own way ("Eastern Standard Time"), which the system does
//! not know, so the invitation's own definition says what its times mean.

use jiff::civil::{Date, DateTime, Weekday};

use crate::ics::Component;

/// The offset east of UTC, in seconds, that `zone` gives at wall time `at`:
/// that of the latest change to standard or summer time at or before it.
pub(crate) fn offset_at(zone: &Component, at: DateTime) -> Option<i32> {
    let mut latest: Option<(DateTime, i32)> = None;
    let mut earliest: Option<(DateTime, i32)> = None;
    for part in zone
        .children
        .iter()
        .filter(|c| c.name == "STANDARD" || c.name == "DAYLIGHT")
    {
        let start = DateTime::strptime("%Y%m%dT%H%M%S", part.get("DTSTART")?.value.trim()).ok()?;
        let to = offset(&part.get("TZOFFSETTO")?.value)?;
        let from = part
            .get("TZOFFSETFROM")
            .and_then(|f| offset(&f.value))
            .unwrap_or(to);
        if earliest.is_none_or(|(e, _)| start < e) {
            earliest = Some((start, from));
        }
        let rule = part.get("RRULE").map(|r| r.value.to_ascii_uppercase());
        let onsets: Vec<DateTime> = match rule.as_deref().and_then(yearly) {
            Some((month, nth, weekday)) => [at.year() - 1, at.year()]
                .into_iter()
                .filter_map(|y| nth_weekday(y, month, nth, weekday))
                .map(|d| d.to_datetime(start.time()))
                .filter(|d| *d >= start)
                .collect(),
            None => vec![start],
        };
        for onset in onsets.into_iter().filter(|d| *d <= at) {
            if latest.is_none_or(|(l, _)| onset > l) {
                latest = Some((onset, to));
            }
        }
    }
    latest.or(earliest).map(|(_, o)| o)
}

/// `-0500`, `+0530`, `+023000` as seconds east of UTC.
fn offset(text: &str) -> Option<i32> {
    let text = text.trim();
    let (sign, digits) = match text.as_bytes().first()? {
        b'-' => (-1, &text[1..]),
        b'+' => (1, &text[1..]),
        _ => (1, text),
    };
    if !(digits.len() == 4 || digits.len() == 6) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let h: i32 = digits[..2].parse().ok()?;
    let m: i32 = digits[2..4].parse().ok()?;
    let s: i32 = digits.get(4..6).map_or(Some(0), |s| s.parse().ok())?;
    Some(sign * (h * 3600 + m * 60 + s))
}

/// A yearly rule's month and day: `FREQ=YEARLY;BYMONTH=3;BYDAY=2SU` is the
/// second Sunday of March; `-1SU` the last.
fn yearly(rule: &str) -> Option<(i8, i8, Weekday)> {
    let mut month = None;
    let mut day = None;
    let mut yearly = false;
    for part in rule.split(';') {
        match part.split_once('=')? {
            ("FREQ", f) => yearly = f == "YEARLY",
            ("BYMONTH", m) => month = m.parse::<i8>().ok(),
            ("BYDAY", d) => {
                let (n, wd) = d.split_at(d.len().checked_sub(2)?);
                let n: i8 = if n.is_empty() { 1 } else { n.parse().ok()? };
                let wd = match wd {
                    "MO" => Weekday::Monday,
                    "TU" => Weekday::Tuesday,
                    "WE" => Weekday::Wednesday,
                    "TH" => Weekday::Thursday,
                    "FR" => Weekday::Friday,
                    "SA" => Weekday::Saturday,
                    "SU" => Weekday::Sunday,
                    _ => return None,
                };
                day = Some((n, wd));
            }
            _ => {}
        }
    }
    let (n, wd) = day?;
    yearly.then_some((month?, n, wd))
}

/// The `nth` `weekday` of a month (from the end when negative).
fn nth_weekday(year: i16, month: i8, nth: i8, weekday: Weekday) -> Option<Date> {
    Date::new(year, month, 1)
        .ok()?
        .nth_weekday_of_month(nth, weekday)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ics;

    /// As Outlook writes New York's zone.
    const EASTERN: &str = "BEGIN:VTIMEZONE\r\nTZID:Eastern Standard Time\r\n\
        BEGIN:STANDARD\r\nDTSTART:16010101T020000\r\nTZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\n\
        RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=1SU;BYMONTH=11\r\nEND:STANDARD\r\n\
        BEGIN:DAYLIGHT\r\nDTSTART:16010101T020000\r\nTZOFFSETFROM:-0500\r\nTZOFFSETTO:-0400\r\n\
        RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=2SU;BYMONTH=3\r\nEND:DAYLIGHT\r\nEND:VTIMEZONE\r\n";

    fn at(s: &str) -> DateTime {
        s.parse().unwrap()
    }

    #[test]
    fn summer_and_winter_by_the_zone_s_own_rules() {
        let zone = &ics::parse(EASTERN)[0];
        // 2026: summer time from 8 March to 1 November.
        assert_eq!(offset_at(zone, at("2026-11-24T10:00")), Some(-5 * 3600));
        assert_eq!(offset_at(zone, at("2026-07-01T10:00")), Some(-4 * 3600));
        assert_eq!(offset_at(zone, at("2026-03-08T01:59")), Some(-5 * 3600));
        assert_eq!(offset_at(zone, at("2026-03-08T02:00")), Some(-4 * 3600));
        assert_eq!(offset_at(zone, at("2026-01-10T10:00")), Some(-5 * 3600));
    }

    #[test]
    fn offsets_and_last_weekdays() {
        assert_eq!(offset("+0530"), Some(5 * 3600 + 30 * 60));
        assert_eq!(offset("-0000"), Some(0));
        assert_eq!(offset("0500x"), None);
        // The last Sunday of October 2026 is the 25th.
        assert_eq!(
            nth_weekday(2026, 10, -1, Weekday::Sunday),
            Date::new(2026, 10, 25).ok()
        );
        // The last Saturday of October 2026 is the 31st, the month's last.
        assert_eq!(
            nth_weekday(2026, 10, -1, Weekday::Saturday),
            Date::new(2026, 10, 31).ok()
        );
    }
}
