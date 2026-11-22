//! Making and changing events in the window: the editor's form as the
//! person types it, read into an event when saved; and deleting one.
//! Google is told on a thread of its own, the guests too when there are
//! any (the Save button says so), and the calendar synced straight after.

use jiff::civil::{Date, DateTime, Time};
use jiff::tz::TimeZone;
use postrinch_commands::Happening;
use postrinch_model::{
    Day, EmailAddress, EventDraft, EventWhen, Timestamp, format_mailboxes, parse_mailboxes,
};

use crate::actions::sync;
use crate::calendar_ui::today;
use crate::state::State;

/// The editor's fields, as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub account: EmailAddress,
    pub calendar: String,
    /// The event changed; none for a new one.
    pub event: Option<String>,
    pub summary: String,
    /// `2026-11-25`.
    pub day: String,
    /// For a whole-day event, its last day.
    pub last_day: String,
    /// `14:00`.
    pub from: String,
    pub to: String,
    pub all_day: bool,
    pub location: String,
    /// Addresses, as To: takes them.
    pub guests: String,
    pub notes: String,
    /// Why it can't be saved, or what went wrong saving it.
    pub error: Option<String>,
    pub busy: bool,
    /// The agent's proposal it came from, settled when it is saved.
    pub proposal: Option<postrinch_model::EventProposal>,
}

impl Form {
    /// A new event on `day`, at the next hour when that is today, else at
    /// nine, for an hour.
    pub fn new(account: EmailAddress, calendar: String, day: Date) -> Self {
        let hour = if day == today() {
            (jiff::Zoned::now().hour() + 1).min(23)
        } else {
            9
        };
        Self {
            account,
            calendar,
            event: None,
            summary: String::new(),
            day: day.to_string(),
            last_day: day.to_string(),
            from: format!("{hour:02}:00"),
            to: format!("{:02}:00", (hour + 1).min(23)),
            all_day: false,
            location: String::new(),
            guests: String::new(),
            notes: String::new(),
            error: None,
            busy: false,
            proposal: None,
        }
    }

    /// An event as it is, to change.
    pub fn of(h: &Happening, tz: &TimeZone) -> Self {
        let d = EventDraft::of(&h.event, &h.account);
        Self::of_draft(h.account.clone(), Some(h.event.id.clone()), d, tz)
    }

    /// An event as a draft says it: an agent's, to check.
    pub fn of_draft(
        account: EmailAddress,
        event: Option<String>,
        d: EventDraft,
        tz: &TimeZone,
    ) -> Self {
        let mut form = Self::new(account, d.calendar.clone(), today());
        form.event = event;
        form.summary = d.summary;
        form.location = d.location;
        form.notes = d.description;
        form.guests = format_mailboxes(&d.guests);
        match d.when {
            EventWhen::Days { first, last } => {
                form.all_day = true;
                form.day = date(first).map(|d| d.to_string()).unwrap_or_default();
                form.last_day = date(last).map(|d| d.to_string()).unwrap_or_default();
            }
            EventWhen::Times { start, end } => {
                let at = |t: Timestamp| {
                    jiff::Timestamp::from_millisecond(t.as_millis())
                        .map(|t| t.to_zoned(tz.clone()))
                        .ok()
                };
                if let (Some(s), Some(e)) = (at(start), at(end)) {
                    form.day = s.date().to_string();
                    form.last_day = form.day.clone();
                    form.from = s.strftime("%H:%M").to_string();
                    form.to = e.strftime("%H:%M").to_string();
                }
            }
        }
        form
    }

    /// The event the form says, read in `tz`; or why not, in a sentence.
    pub fn draft(&self, tz: &TimeZone) -> Result<EventDraft, String> {
        let summary = self.summary.trim();
        if summary.is_empty() {
            return Err("Give it a name.".into());
        }
        let day: Date = self.day.trim().parse().map_err(|_| {
            format!(
                "{:?} is not a day: write it as 2026-11-25.",
                self.day.trim()
            )
        })?;
        let when = if self.all_day {
            let last: Date = match self.last_day.trim() {
                "" => day,
                text => text
                    .parse()
                    .map_err(|_| format!("{text:?} is not a day: write it as 2026-11-25."))?,
            };
            if last < day {
                return Err("It ends before it starts.".into());
            }
            EventWhen::Days {
                first: of(day),
                last: of(last),
            }
        } else {
            let time = |text: &str| {
                text.trim()
                    .parse::<Time>()
                    .map_err(|_| format!("{:?} is not a time: write it as 14:00.", text.trim()))
            };
            let (from, to) = (time(&self.from)?, time(&self.to)?);
            let at = |t: Time| -> Result<Timestamp, String> {
                DateTime::from_parts(day, t)
                    .to_zoned(tz.clone())
                    .map(|z| Timestamp::from_millis(z.timestamp().as_millisecond()))
                    .map_err(|e| e.to_string())
            };
            // An end before the start is past midnight.
            let end_day = if to <= from {
                day.tomorrow().unwrap_or(day)
            } else {
                day
            };
            let end = DateTime::from_parts(end_day, to)
                .to_zoned(tz.clone())
                .map(|z| Timestamp::from_millis(z.timestamp().as_millisecond()))
                .map_err(|e| e.to_string())?;
            EventWhen::Times {
                start: at(from)?,
                end,
            }
        };
        let guests = parse_mailboxes(&self.guests).map_err(|e| e.to_string())?;
        Ok(EventDraft {
            calendar: self.calendar.clone(),
            summary: summary.to_owned(),
            location: self.location.trim().to_owned(),
            description: self.notes.trim().to_owned(),
            when,
            time_zone: tz.iana_name().map(str::to_owned),
            guests,
        })
    }

    /// Whether saving sends the guests word.
    pub fn tells(&self) -> bool {
        !self.guests.trim().is_empty()
    }

    /// What Save says: whether it invites or tells guests.
    pub fn save_label(&self) -> &'static str {
        match (self.tells(), self.event.is_some()) {
            (false, _) => "Save",
            (true, false) => "Save and send invitations",
            (true, true) => "Save and tell guests",
        }
    }
}

fn of(d: Date) -> Day {
    Day {
        year: d.year(),
        month: d.month(),
        day: d.day(),
    }
}

fn date(d: Day) -> Option<Date> {
    Date::new(d.year, d.month, d.day).ok()
}

impl State {
    /// Open the editor on a new event, in the first account's own
    /// calendar, on the day in view.
    pub fn new_event(self) {
        let calendars = self.calendar.calendars.get();
        let Some((account, info)) = calendars
            .iter()
            .find(|(_, c)| c.primary)
            .or_else(|| calendars.iter().find(|(_, c)| c.writable))
        else {
            self.error.set(Some(
                "There is no calendar to put it in yet: sync hasn\u{2019}t brought any.".into(),
            ));
            return;
        };
        let form = Form::new(account.clone(), info.id.clone(), self.calendar.day.get());
        self.calendar.open.set(None);
        self.calendar.editing.set(Some(form));
    }

    pub fn edit_event(self, h: &Happening) {
        self.calendar
            .editing
            .set(Some(Form::of(h, &TimeZone::system())));
    }

    /// Change the form as the person types.
    pub fn edit_form(self, change: impl FnOnce(&mut Form)) {
        self.calendar.editing.update(|e| {
            if let Some(form) = e {
                change(form);
                form.error = None;
            }
        });
    }

    pub fn cancel_edit(self) {
        self.calendar.editing.set(None);
    }

    /// Save the form: make the event or change it.
    pub fn save_event(self) {
        let Some(form) = self.calendar.editing.get() else {
            return;
        };
        let draft = match form.draft(&TimeZone::system()) {
            Ok(draft) => draft,
            Err(why) => {
                self.edit_form(|f| f.error = Some(why));
                return;
            }
        };
        let Some(handle) = sync() else {
            self.edit_form(|f| f.error = Some(crate::invitations::OFF.into()));
            return;
        };
        self.edit_form(|f| f.busy = true);
        let (editing, revision) = (self.calendar.editing, self.revision);
        let here = crate::actions::commands().map(|c| (*c).clone());
        std::thread::spawn(move || {
            let sync = handle.sync_engine();
            let done = match &form.event {
                Some(id) => handle
                    .runtime
                    .block_on(sync.change_event(&form.account, id, &draft)),
                None => handle
                    .runtime
                    .block_on(sync.create_event(&form.account, &draft))
                    .map(drop),
            };
            match done {
                Ok(()) => {
                    if let (Some(p), Some(here)) = (&form.proposal, &here)
                        && let Err(e) = here.settle_event_proposal(&form.account, p.id)
                    {
                        tracing::warn!("settling an agent's event: {e}");
                    }
                    editing.send(None);
                    revision.update_send(|n| *n += 1);
                }
                Err(e) => {
                    let why = crate::invitations::say(&e);
                    editing.update_send(move |f| {
                        if let Some(f) = f {
                            f.busy = false;
                            f.error = Some(why);
                        }
                    });
                }
            }
        });
    }

    /// Delete the open event (this time alone, for one of a series),
    /// telling its guests.
    pub fn delete_event(self) {
        let Some(open) = self.calendar.open.get() else {
            return;
        };
        let Some(handle) = sync() else {
            self.error.set(Some(crate::invitations::OFF.into()));
            return;
        };
        let (error, revision, opened) = (self.error, self.revision, self.calendar.open);
        let here = crate::actions::commands().map(|c| (*c).clone());
        // A guest taking it off its own calendar tells nobody.
        let organizes = open.event.organizer.as_ref().is_none_or(|o| {
            o.address.key() == open.account.key() || o.address.to_string() == open.event.calendar
        });
        let tell = organizes && !open.event.attendees.is_empty();
        std::thread::spawn(move || {
            let sync = handle.sync_engine();
            let done = handle.runtime.block_on(sync.delete_event(
                &open.account,
                &open.event.calendar,
                &open.event.id,
                tell,
            ));
            match done {
                Ok(()) => {
                    if let Some(here) = &here {
                        crate::calendar_proposals::settle_for(here, &open.account, &open.event.id);
                    }
                    opened.send(None);
                    revision.update_send(|n| *n += 1);
                }
                Err(e) => error.send(Some(crate::invitations::say(&e))),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> Form {
        let mut f = Form::new(
            EmailAddress::parse("me@example.org").unwrap(),
            "me@example.org".into(),
            "2026-11-25".parse().unwrap(),
        );
        f.summary = "Walkthrough".into();
        f
    }

    #[test]
    fn reads_times_days_and_guests() {
        let mut f = form();
        f.from = "14:00".into();
        f.to = "15:30".into();
        f.guests = "Ada <ada@example.org>, bob@example.org".into();
        let d = f.draft(&TimeZone::UTC).unwrap();
        let EventWhen::Times { start, end } = d.when else {
            panic!()
        };
        assert_eq!(end.as_millis() - start.as_millis(), 90 * 60_000);
        assert_eq!(d.guests.len(), 2);
        assert!(f.tells());

        // Past midnight.
        f.from = "23:00".into();
        f.to = "01:00".into();
        let EventWhen::Times { start, end } = f.draft(&TimeZone::UTC).unwrap().when else {
            panic!()
        };
        assert_eq!(end.as_millis() - start.as_millis(), 2 * 3_600_000);

        f.all_day = true;
        f.last_day = "2026-11-27".into();
        let EventWhen::Days { first, last } = f.draft(&TimeZone::UTC).unwrap().when else {
            panic!()
        };
        assert_eq!((first.day, last.day), (25, 27));
    }

    #[test]
    fn says_what_is_wrong() {
        let mut f = form();
        f.summary = " ".into();
        assert!(f.draft(&TimeZone::UTC).unwrap_err().contains("name"));
        f.summary = "x".into();
        f.day = "tomorrow".into();
        assert!(f.draft(&TimeZone::UTC).unwrap_err().contains("2026-11-25"));
        f.day = "2026-11-25".into();
        f.from = "2pm".into();
        assert!(f.draft(&TimeZone::UTC).unwrap_err().contains("14:00"));
        f.from = "14:00".into();
        f.all_day = true;
        f.last_day = "2026-11-20".into();
        assert!(f.draft(&TimeZone::UTC).unwrap_err().contains("before"));
    }
}
