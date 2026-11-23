//! What the person wants to be told about, and how (Settings ›
//! Notifications, after its mockup): new mail by view, reminders by
//! calendar (when they fall due is Google Calendar's to say), invitations, agents waiting, refused mail; what a notice
//! shows, several at once, quiet hours, and not while the window is in
//! front. Kept in the settings file.

use std::collections::BTreeMap;

use postrinch_commands::{ReminderRules, followed};
use postrinch_model::{CalendarInfo, EmailAddress};
use serde::{Deserialize, Serialize};

/// What a mail notice shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shows {
    /// Who, the subject and how it starts.
    #[default]
    Everything,
    WhoAndSubject,
    /// Only that mail came, for a screen others see.
    Nothing,
}

/// Several messages at once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Several {
    /// A notice each, up to three; more become one.
    #[default]
    EachUpToThree,
    /// One notice for them all.
    OneForAll,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NoticePrefs {
    pub mail: bool,
    /// Views by key, on or off; those not here take [`Self::view_on`]'s
    /// default.
    pub views: BTreeMap<String, bool>,
    /// Only mail from people the person has written to.
    pub known_only: bool,
    pub shows: Shows,
    pub several: Several,
    pub reminders: bool,
    /// Calendars by `calendar_ui::hide_key`, on or off. When they remind
    /// is Google Calendar's to say.
    pub calendars: BTreeMap<String, bool>,
    pub invitations: bool,
    pub agents: bool,
    pub refused: bool,
    pub quiet: bool,
    /// `HH:MM`.
    pub quiet_from: String,
    pub quiet_to: String,
    /// Monday first.
    pub quiet_days: [bool; 7],
    /// Reminders come through in quiet hours.
    pub quiet_reminders: bool,
    /// No mail notices while the window is in front.
    pub not_in_front: bool,
}

impl Default for NoticePrefs {
    fn default() -> Self {
        Self {
            mail: true,
            views: BTreeMap::new(),
            known_only: false,
            shows: Shows::default(),
            several: Several::default(),
            reminders: true,
            calendars: BTreeMap::new(),
            invitations: true,
            agents: true,
            refused: true,
            quiet: false,
            quiet_from: "22:00".into(),
            quiet_to: "07:00".into(),
            quiet_days: [true; 7],
            quiet_reminders: true,
            not_in_front: true,
        }
    }
}

impl NoticePrefs {
    /// Whether new mail in the view `key` is told: as set, else the Inbox
    /// alone, and each account's inbox as the Inbox.
    pub fn view_on(&self, key: &str) -> bool {
        if let Some(on) = self.views.get(key) {
            return *on;
        }
        match followed(key) {
            Some("inbox") => self.view_on("inbox"),
            Some(_) => false,
            None => key == "inbox",
        }
    }

    /// Whether a calendar reminds: as set, else one the account may change
    /// (not the holidays and other calendars it only follows).
    pub fn calendar_on(&self, account: &EmailAddress, info: &CalendarInfo) -> bool {
        let key = crate::calendar_ui::hide_key(account, &info.id);
        self.calendars.get(&key).copied().unwrap_or(info.writable)
    }

    /// The reminder rules for these calendars.
    pub fn rules(&self, calendars: &[(EmailAddress, CalendarInfo)]) -> ReminderRules {
        ReminderRules {
            quiet_calendars: calendars
                .iter()
                .filter(|(a, c)| !self.calendar_on(a, c))
                .map(|(a, c)| crate::calendar_ui::hide_key(a, &c.id))
                .collect(),
        }
    }

    /// Whether `now` (a weekday from Monday, 0, and a time) is in quiet
    /// hours; hours that pass midnight count from the day they start.
    pub fn quiet_at(&self, weekday: usize, time: jiff::civil::Time) -> bool {
        if !self.quiet {
            return false;
        }
        let (Ok(from), Ok(to)) = (
            self.quiet_from.trim().parse::<jiff::civil::Time>(),
            self.quiet_to.trim().parse::<jiff::civil::Time>(),
        ) else {
            return false;
        };
        let day = |d: usize| self.quiet_days[d % 7];
        if from <= to {
            day(weekday) && time >= from && time < to
        } else {
            (day(weekday) && time >= from) || (day(weekday + 6) && time < to)
        }
    }
}

impl crate::state::State {
    /// Change what notices tell, and save it.
    pub fn set_notices(self, change: impl FnOnce(&mut NoticePrefs)) {
        self.notices.prefs.update(change);
        let prefs = self.notices.prefs.get();
        self.save_settings(|s| s.notices = prefs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::Time;

    #[test]
    fn views_default_to_the_inbox_and_its_accounts() {
        let mut p = NoticePrefs::default();
        assert!(p.view_on("inbox"));
        assert!(p.view_on("inbox:me@example.org"));
        assert!(!p.view_on("updates:me@example.org"));
        assert!(!p.view_on("some-view-of-mine"));
        p.views.insert("inbox".into(), false);
        assert!(!p.view_on("inbox:me@example.org"));
        p.views.insert("inbox:me@example.org".into(), true);
        assert!(p.view_on("inbox:me@example.org"));
    }

    #[test]
    fn quiet_hours_across_midnight() {
        let mut p = NoticePrefs {
            quiet: true,
            ..Default::default()
        };
        p.quiet_days = [true, true, true, true, true, false, false];
        let t = |h, m| Time::constant(h, m, 0, 0);
        // Friday 23:00: quiet; Saturday 06:00: still Friday's night.
        assert!(p.quiet_at(4, t(23, 0)));
        assert!(p.quiet_at(5, t(6, 0)));
        // Saturday 23:00: Saturday is not a quiet day.
        assert!(!p.quiet_at(5, t(23, 0)));
        // Sunday 06:00: Saturday's night, so not quiet.
        assert!(!p.quiet_at(6, t(6, 0)));
        assert!(!p.quiet_at(0, t(12, 0)));
    }
}
