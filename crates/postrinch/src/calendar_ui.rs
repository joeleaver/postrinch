//! The calendar view's state (DESIGN.md "Calendar", the calendar mockup):
//! the day in view and how much of it shows, the events of that stretch
//! and the calendars they are in (loaded by the loader), which calendars
//! the person hid, the event open in the pane, and the editor's draft.
//! Changes go to Google on a thread of their own, and the calendar is
//! synced straight after.

use std::collections::HashSet;

use jiff::civil::Date;
use jiff::tz::TimeZone;
use postrinch_commands::Happening;
use postrinch_model::{Answer, CalendarInfo, EmailAddress, Timestamp};
use postrinch_sync::google::Answered;
use rinch::prelude::*;
use rinch::{Effect, untracked};
use serde::{Deserialize, Serialize};

use crate::actions::sync;
use crate::loader::Request;
use crate::state::{State, View};

/// How much of the calendar shows at once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalSpan {
    Day,
    #[default]
    Week,
    Month,
}

/// The calendar view's signals. `Copy`, like the window's state.
#[derive(Clone, Copy)]
pub struct CalendarUi {
    /// A day in the stretch shown.
    pub day: Signal<Date>,
    pub span: Signal<CalSpan>,
    /// The events of the stretch shown, soonest first.
    pub happenings: Signal<Vec<Happening>>,
    pub calendars: Signal<Vec<(EmailAddress, CalendarInfo)>>,
    /// Hidden calendars, as [`hide_key`] gives them.
    pub hidden: Signal<HashSet<String>>,
    /// The event open in the pane.
    pub open: Signal<Option<Happening>>,
    /// An answer on its way, to the open event.
    pub answering: Signal<Option<Answer>>,
    /// The event being made or changed, in the pane.
    pub editing: Signal<Option<crate::calendar_edit::Form>>,
}

/// A calendar as the hidden set names it.
pub fn hide_key(account: &EmailAddress, calendar: &str) -> String {
    format!("{}\u{1f}{calendar}", account.key())
}

pub fn today() -> Date {
    jiff::Zoned::now().date()
}

impl CalendarUi {
    pub fn new(settings: &crate::appearance::Settings) -> Self {
        Self {
            day: Signal::new(today()),
            span: Signal::new(settings.calendar_span),
            happenings: Signal::new(Vec::new()),
            calendars: Signal::new(Vec::new()),
            hidden: Signal::new(settings.hidden_calendars.iter().cloned().collect()),
            open: Signal::new(None),
            answering: Signal::new(None),
            editing: Signal::new(None),
        }
    }
}

/// The first and last days of the stretch `span` shows around `day`:
/// a day; Monday to Sunday; or the six weeks holding the month.
pub fn stretch(day: Date, span: CalSpan) -> (Date, Date) {
    let monday = |d: Date| {
        let back = d.weekday().to_monday_zero_offset();
        d.checked_sub(jiff::Span::new().days(back)).unwrap_or(d)
    };
    match span {
        CalSpan::Day => (day, day),
        CalSpan::Week => {
            let first = monday(day);
            (
                first,
                first
                    .checked_add(jiff::Span::new().days(6))
                    .unwrap_or(first),
            )
        }
        CalSpan::Month => {
            let first = monday(day.first_of_month());
            (
                first,
                first
                    .checked_add(jiff::Span::new().days(41))
                    .unwrap_or(first),
            )
        }
    }
}

/// Midnight starting `day`, in the person's zone.
pub fn midnight(day: Date) -> Timestamp {
    let at = day
        .to_zoned(TimeZone::system())
        .map(|z| z.timestamp().as_millisecond())
        .unwrap_or(0);
    Timestamp::from_millis(at)
}

impl State {
    /// Keep the stretch shown loaded while the calendar is open, again
    /// whenever sync changes the store.
    pub fn load_calendar(self, ask: impl Fn(Request) + 'static) {
        Effect::new(move || {
            self.revision.get();
            if self.view.get() != View::Calendar {
                return;
            }
            let (first, last) = stretch(self.calendar.day.get(), self.calendar.span.get());
            let after = last.tomorrow().unwrap_or(last);
            ask(Request::Calendar {
                from: midnight(first),
                to: midnight(after),
            });
        });
        // The open event follows what is loaded: changed, or gone.
        Effect::new(move || {
            let loaded = self.calendar.happenings.get();
            let Some(open) = untracked(|| self.calendar.open.get()) else {
                return;
            };
            let now = loaded
                .iter()
                .find(|h| h.account == open.account && h.event.id == open.event.id)
                .cloned();
            if now.as_ref() != Some(&open) {
                // A time of a series moved out of the stretch stays open.
                if let Some(now) = now {
                    self.calendar.open.set(Some(now));
                }
            }
        });
    }

    pub fn show_calendar(self, on: bool) {
        self.view.set(if on { View::Calendar } else { View::Mail });
    }

    /// Move the stretch shown by one step of its span, back or on.
    pub fn calendar_step(self, on: bool) {
        let span = self.calendar.span.get();
        let step = match span {
            CalSpan::Day => jiff::Span::new().days(1),
            CalSpan::Week => jiff::Span::new().weeks(1),
            CalSpan::Month => jiff::Span::new().months(1),
        };
        let day = self.calendar.day.get();
        let moved = if on {
            day.checked_add(step)
        } else {
            day.checked_sub(step)
        };
        if let Ok(moved) = moved {
            self.calendar.day.set(moved);
        }
    }

    pub fn set_calendar_span(self, span: CalSpan) {
        self.calendar.span.set(span);
        self.save_settings(|s| s.calendar_span = span);
    }

    /// Show a day: the day itself, from the month.
    pub fn calendar_go(self, day: Date, span: Option<CalSpan>) {
        batch(|| {
            self.calendar.day.set(day);
            if let Some(span) = span {
                self.calendar.span.set(span);
            }
        });
    }

    pub fn toggle_calendar(self, account: &EmailAddress, calendar: &str) {
        let key = hide_key(account, calendar);
        self.calendar.hidden.update(|h| {
            if !h.remove(&key) {
                h.insert(key);
            }
        });
        let hidden: Vec<String> = self.calendar.hidden.get().into_iter().collect();
        self.save_settings(|s| s.hidden_calendars = hidden);
    }

    pub fn open_event(self, happening: Option<Happening>) {
        batch(|| {
            self.calendar.editing.set(None);
            self.calendar.answering.set(None);
            self.calendar.open.set(happening);
        });
    }

    /// Answer the open event as its account, telling the organizer.
    pub fn answer_event(self, answer: Answer) {
        let Some(open) = self.calendar.open.get() else {
            return;
        };
        if open.event.answer == Some(answer) {
            return;
        }
        let Some(handle) = sync() else {
            self.error.set(Some(crate::invitations::OFF.into()));
            return;
        };
        self.calendar.answering.set(Some(answer));
        let (answering, error, revision) = (self.calendar.answering, self.error, self.revision);
        let (happenings, opened) = (self.calendar.happenings, self.calendar.open);
        std::thread::spawn(move || {
            let sync = handle.sync_engine();
            let done = handle.runtime.block_on(sync.answer_event(
                &open.account,
                &open.event.calendar,
                &open.event.id,
                answer,
            ));
            let trouble = match done {
                Ok(Answered::Done(_)) => {
                    // Shown at once, before the stretch loads again.
                    let (who, id) = (open.account.clone(), open.event.id.clone());
                    happenings.update_send(move |all| {
                        for h in all
                            .iter_mut()
                            .filter(|h| h.account == who && h.event.id == id)
                        {
                            h.event.answer = Some(answer);
                        }
                    });
                    let mut shown = open.clone();
                    shown.event.answer = Some(answer);
                    opened.send(Some(shown));
                    None
                }
                Ok(Answered::NotInCalendar) => {
                    Some("That event is no longer in the calendar.".to_owned())
                }
                Ok(Answered::NotAGuest) => Some(format!(
                    "{} isn\u{2019}t among this event\u{2019}s guests.",
                    open.account
                )),
                Err(e) => Some(crate::invitations::say(&e)),
            };
            answering.send(None);
            revision.update_send(|n| *n += 1);
            if let Some(trouble) = trouble {
                error.send(Some(trouble));
            }
        });
    }
}
