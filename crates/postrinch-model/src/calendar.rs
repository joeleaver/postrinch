//! Calendar events: an invitation as mail carries it, and an event as the
//! person's calendar holds it.

use serde::{Deserialize, Serialize};

use crate::{Mailbox, Timestamp};

/// A day on the calendar, without a time or a time zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Day {
    pub year: i16,
    pub month: i8,
    pub day: i8,
}

/// When an event happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventWhen {
    /// Whole days, `first` to `last` inclusive, wherever the person is.
    Days { first: Day, last: Day },
    /// From `start` until `end`.
    Times { start: Timestamp, end: Timestamp },
}

/// How someone answered an invitation. Google calls these accepted,
/// tentative and declined; iCalendar calls them `PARTSTAT`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Answer {
    Yes,
    Maybe,
    No,
}

impl Answer {
    pub const ALL: [Answer; 3] = [Answer::Yes, Answer::Maybe, Answer::No];

    /// The word an agent uses for it.
    pub fn word(self) -> &'static str {
        match self {
            Answer::Yes => "yes",
            Answer::Maybe => "maybe",
            Answer::No => "no",
        }
    }

    pub fn from_word(word: &str) -> Option<Answer> {
        Answer::ALL
            .into_iter()
            .find(|a| a.word().eq_ignore_ascii_case(word.trim()))
    }
}

/// Someone invited, and their answer if they have given one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attendee {
    pub mailbox: Mailbox,
    pub answer: Option<Answer>,
}

/// What an invitation in mail asks (its iCalendar `METHOD`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InvitationKind {
    /// An invitation, new or changed: it asks for an answer.
    Request,
    /// The event is called off.
    Cancel,
    /// Someone's answer to an invitation the person sent.
    Reply,
    /// An event to add to a calendar, with nobody to answer to.
    Publish,
}

/// An event as an invitation in mail describes it. Every word of it is the
/// sender's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invitation {
    pub kind: InvitationKind,
    /// The event's `UID`, which every calendar keeps it under.
    pub uid: String,
    /// For one time of a repeating event, which time (its `RECURRENCE-ID`,
    /// as written).
    pub recurrence: Option<String>,
    pub summary: Option<String>,
    pub location: Option<String>,
    pub when: Option<EventWhen>,
    pub repeats: bool,
    pub organizer: Option<Mailbox>,
    pub attendees: Vec<Attendee>,
}

/// An event in one of the person's calendars: a single event, or one time
/// of a repeating one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvent {
    /// The calendar's id for it.
    pub id: String,
    /// The id of the calendar it is in.
    #[serde(default)]
    pub calendar: String,
    pub summary: Option<String>,
    pub location: Option<String>,
    /// What its description says. Whoever made the event wrote it.
    #[serde(default)]
    pub description: Option<String>,
    /// Whether it makes the person busy (Google's "transparency").
    #[serde(default = "yes")]
    pub busy: bool,
    pub when: EventWhen,
    pub organizer: Option<Mailbox>,
    pub attendees: Vec<Attendee>,
    /// Whether the account is among its guests, so can answer it.
    pub guest: bool,
    /// The account's own answer, when it is a guest and has given one.
    pub answer: Option<Answer>,
    pub cancelled: bool,
    /// Whether it is one time of a repeating event.
    pub repeating: bool,
    /// The event in Google Calendar on the web.
    pub link: Option<String>,
}

fn yes() -> bool {
    true
}

/// One of an account's calendars.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarInfo {
    pub id: String,
    /// What the person calls it (their own name for it, or its own).
    pub name: String,
    /// Its colour, `#rrggbb`.
    pub color: Option<String>,
    /// The account's own calendar.
    pub primary: bool,
    /// Whether the account may change its events.
    pub writable: bool,
    /// Whether the person shows it in Google Calendar.
    pub shown: bool,
    pub time_zone: Option<String>,
}

/// Where an event starts: a day, or a moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EventStart {
    Day(Day),
    At(Timestamp),
}

impl EventWhen {
    pub fn start(&self) -> EventStart {
        match *self {
            EventWhen::Days { first, .. } => EventStart::Day(first),
            EventWhen::Times { start, .. } => EventStart::At(start),
        }
    }
}

/// An event as a calendar keeps it: a single event, a repeating series
/// (its first time, and the rule it repeats by), or one time of a series
/// that was changed or cancelled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRecord {
    /// For a series, its first time.
    pub event: CalendarEvent,
    /// For a series, its iCalendar `RRULE`, `EXRULE`, `RDATE` and `EXDATE`
    /// lines.
    pub recurrence: Vec<String>,
    /// The time zone a series repeats in.
    pub time_zone: Option<String>,
    /// For one time of a series: the series' id, and which time it was.
    pub series: Option<String>,
    pub original_start: Option<EventStart>,
}

/// An agent's answer to an invitation in a message, waiting for the person:
/// answering tells the organizer, so only the person gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedAnswer {
    pub message: crate::ProviderMessageId,
    pub thread: crate::ProviderThreadId,
    pub answer: Answer,
    /// The agent's name.
    pub agent: String,
    pub reason: Option<String>,
    /// The event's summary, as the invitation gives it.
    pub summary: Option<String>,
    pub made: Timestamp,
}
