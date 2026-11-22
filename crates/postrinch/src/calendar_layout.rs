//! Where events go in the calendar's day columns: whole-day events in the
//! band over the hours; timed ones at their hours, an event that crosses
//! midnight in each day it covers, side by side where they overlap.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use postrinch_commands::Happening;
use postrinch_model::EventWhen;

/// The height of an hour, in px.
pub const HOUR: f64 = 48.0;
/// The shortest an event is drawn, in px, so a short one can be read.
const LEAST: f64 = 20.0;

/// An event placed in a day's column.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub happening: Happening,
    pub top: f64,
    pub height: f64,
    /// Which of `lanes` side-by-side places it takes.
    pub lane: usize,
    pub lanes: usize,
}

/// One day's column.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub day: Date,
    pub all_day: Vec<Happening>,
    pub timed: Vec<Placed>,
}

/// The columns of `days` from `first`, with what of `shown` falls in each.
pub fn columns(shown: &[Happening], first: Date, days: i64, tz: &TimeZone) -> Vec<Column> {
    (0..days)
        .filter_map(|i| first.checked_add(jiff::Span::new().days(i)).ok())
        .map(|day| column(shown, day, tz))
        .collect()
}

fn column(shown: &[Happening], day: Date, tz: &TimeZone) -> Column {
    let start = at(day, tz);
    let end = at(day.tomorrow().unwrap_or(day), tz);
    let mut all_day = Vec::new();
    let mut spans: Vec<(i64, i64, Happening)> = Vec::new();
    for h in shown {
        match h.event.when {
            EventWhen::Days { first, last } => {
                let covers =
                    date(first).is_some_and(|f| f <= day) && date(last).is_some_and(|l| l >= day);
                if covers {
                    all_day.push(h.clone());
                }
            }
            EventWhen::Times { start: s, end: e } => {
                let (s, e) = (s.as_millis(), e.as_millis().max(s.as_millis()));
                let inside = if e == s {
                    s >= start && s < end
                } else {
                    s < end && e > start
                };
                if inside {
                    spans.push((s.max(start), e.min(end), h.clone()));
                }
            }
        }
    }
    spans.sort_by_key(|(s, e, _)| (*s, -*e));
    Column {
        day,
        all_day,
        timed: lanes(spans, start),
    }
}

/// Side-by-side places for overlapping spans: each cluster of spans that
/// overlap one another shares its width between as many lanes as it needs.
fn lanes(spans: Vec<(i64, i64, Happening)>, midnight: i64) -> Vec<Placed> {
    let px = |ms: i64| (ms - midnight) as f64 / 3_600_000.0 * HOUR;
    let mut out: Vec<Placed> = Vec::new();
    let mut cluster: Vec<usize> = Vec::new();
    let mut lane_ends: Vec<i64> = Vec::new();
    let mut cluster_end = i64::MIN;
    for (s, e, h) in spans {
        // Drawn at least LEAST tall: it overlaps what that covers.
        let drawn_end = e.max(s + (LEAST / HOUR * 3_600_000.0) as i64);
        if s >= cluster_end {
            close(&mut out, &cluster, lane_ends.len());
            cluster.clear();
            lane_ends.clear();
        }
        let lane = match lane_ends.iter().position(|end| *end <= s) {
            Some(free) => {
                lane_ends[free] = drawn_end;
                free
            }
            None => {
                lane_ends.push(drawn_end);
                lane_ends.len() - 1
            }
        };
        cluster_end = cluster_end.max(drawn_end);
        cluster.push(out.len());
        out.push(Placed {
            happening: h,
            top: px(s),
            height: (px(e) - px(s)).max(LEAST),
            lane,
            lanes: 1,
        });
    }
    close(&mut out, &cluster, lane_ends.len());
    out
}

fn close(out: &mut [Placed], cluster: &[usize], lanes: usize) {
    for &i in cluster {
        out[i].lanes = lanes.max(1);
    }
}

fn at(day: Date, tz: &TimeZone) -> i64 {
    day.to_zoned(tz.clone())
        .map(|z| z.timestamp().as_millisecond())
        .unwrap_or(0)
}

fn date(d: postrinch_model::Day) -> Option<Date> {
    Date::new(d.year, d.month, d.day).ok()
}

/// The day an event starts on, in `tz`.
pub fn span_day(when: &EventWhen, tz: &TimeZone) -> Option<Date> {
    match *when {
        EventWhen::Days { first, .. } => date(first),
        EventWhen::Times { start, .. } => jiff::Timestamp::from_millisecond(start.as_millis())
            .ok()
            .map(|t| t.to_zoned(tz.clone()).date()),
    }
}

/// Where now is in a day's column, in px from its top.
pub fn now_top(tz: &TimeZone) -> f64 {
    let now = jiff::Zoned::now().with_time_zone(tz.clone());
    (f64::from(now.hour()) + f64::from(now.minute()) / 60.0) * HOUR
}

#[cfg(test)]
mod tests {
    use postrinch_model::{CalendarEvent, Day, EmailAddress, Timestamp};

    use super::*;

    fn at(s: &str) -> Timestamp {
        Timestamp::from_millis(s.parse::<jiff::Timestamp>().unwrap().as_millisecond())
    }

    fn h(id: &str, when: EventWhen) -> Happening {
        Happening {
            account: EmailAddress::parse("me@example.org").unwrap(),
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
            },
        }
    }

    fn timed(id: &str, s: &str, e: &str) -> Happening {
        h(
            id,
            EventWhen::Times {
                start: at(s),
                end: at(e),
            },
        )
    }

    #[test]
    fn overlapping_events_share_the_width() {
        let shown = [
            timed("a", "2026-11-24T09:00:00Z", "2026-11-24T10:00:00Z"),
            timed("b", "2026-11-24T09:30:00Z", "2026-11-24T11:00:00Z"),
            timed("c", "2026-11-24T10:00:00Z", "2026-11-24T10:30:00Z"),
            timed("d", "2026-11-24T13:00:00Z", "2026-11-24T14:00:00Z"),
        ];
        let day: Date = "2026-11-24".parse().unwrap();
        let cols = columns(&shown, day, 1, &TimeZone::UTC);
        let placed: Vec<_> = cols[0]
            .timed
            .iter()
            .map(|p| (p.happening.event.id.as_str(), p.lane, p.lanes, p.top))
            .collect();
        assert_eq!(
            placed,
            [
                ("a", 0, 2, 9.0 * HOUR),
                ("b", 1, 2, 9.5 * HOUR),
                // C starts as A ends, so takes A's lane.
                ("c", 0, 2, 10.0 * HOUR),
                ("d", 0, 1, 13.0 * HOUR),
            ]
        );
    }

    #[test]
    fn midnight_splits_and_whole_days_band() {
        let d = |day| Day {
            year: 2026,
            month: 11,
            day,
        };
        let shown = [
            timed("late", "2026-11-24T22:00:00Z", "2026-11-25T02:00:00Z"),
            h(
                "trip",
                EventWhen::Days {
                    first: d(24),
                    last: d(26),
                },
            ),
        ];
        let day: Date = "2026-11-24".parse().unwrap();
        let cols = columns(&shown, day, 3, &TimeZone::UTC);
        assert_eq!(cols[0].timed[0].height, 2.0 * HOUR);
        assert_eq!(cols[1].timed[0].top, 0.0);
        assert_eq!(cols[1].timed[0].height, 2.0 * HOUR);
        assert!(cols[2].timed.is_empty());
        assert!(cols.iter().all(|c| c.all_day.len() == 1));
    }
}
