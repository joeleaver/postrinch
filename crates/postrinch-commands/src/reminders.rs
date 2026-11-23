//! Reminders before events: the moments they fall due, by each event's own
//! reminders as Google Calendar keeps them, else its calendar's.

use std::collections::{HashMap, HashSet};

use jiff::tz::TimeZone;
use postrinch_model::{Answer, EmailAddress, Timestamp};

use crate::calendar_times::span;
use crate::{CommandResult, Commands, Happening};

/// How far ahead of an event a reminder may be: Google allows four weeks.
const FURTHEST_DAYS: i64 = 28;

/// What the person asked of reminders. When they fall due is Google
/// Calendar's to say (each event's own, or its calendar's); the person only
/// chooses which calendars remind them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReminderRules {
    /// Calendars that never remind, as `<account key>\u{1f}<calendar id>`.
    pub quiet_calendars: HashSet<String>,
}

/// A reminder that has fallen due.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Due {
    pub happening: Happening,
    /// Minutes before it starts (negative for later on its day).
    pub minutes: i64,
    pub at: Timestamp,
}

impl Commands {
    /// The reminders that fall due after `after` and at or before `until`,
    /// soonest first; an event in two accounts' calendars reminds once.
    pub fn reminders_due(
        &self,
        after: Timestamp,
        until: Timestamp,
        rules: &ReminderRules,
    ) -> CommandResult<Vec<Due>> {
        let tz = TimeZone::system();
        let defaults: HashMap<(EmailAddress, String), Vec<i64>> = self
            .calendars()?
            .into_iter()
            .map(|(a, c)| ((a, c.id), c.reminders))
            .collect();
        let ahead = Timestamp::from_millis(until.as_millis() + FURTHEST_DAYS * 86_400_000);
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for h in self.events_between(&[], after, ahead)? {
            let e = &h.event;
            let key = format!("{}\u{1f}{}", h.account.key(), e.calendar);
            if e.cancelled
                || e.answer == Some(Answer::No)
                || rules.quiet_calendars.contains(&key)
                || !seen.insert(e.id.clone())
            {
                continue;
            }
            let start = span(&e.when, &tz).0;
            // A whole-day event's are counted back from its midnight, as
            // Google counts them.
            let minutes = e.reminders.clone().unwrap_or_else(|| {
                defaults
                    .get(&(h.account.clone(), e.calendar.clone()))
                    .cloned()
                    .unwrap_or_default()
            });
            for m in minutes {
                let at = Timestamp::from_millis(start.as_millis() - m * 60_000);
                if at > after && at <= until {
                    out.push(Due {
                        happening: h.clone(),
                        minutes: m,
                        at,
                    });
                }
            }
        }
        out.sort_by_key(|d| d.at);
        Ok(out)
    }
}

#[cfg(test)]
#[path = "reminders_tests.rs"]
mod tests;
