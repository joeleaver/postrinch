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
    /// When to remind the person, in minutes before it starts; `None` for
    /// its calendar's [`CalendarInfo::reminders`].
    #[serde(default)]
    pub reminders: Option<Vec<i64>>,
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
    /// When its events remind the person by default, in minutes before
    /// they start.
    #[serde(default)]
    pub reminders: Vec<i64>,
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

/// An event being made or changed: what the editor and agents give.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventDraft {
    /// The calendar it goes in.
    pub calendar: String,
    pub summary: String,
    pub location: String,
    pub description: String,
    pub when: EventWhen,
    /// The zone its times are written in (the calendar's, by default).
    pub time_zone: Option<String>,
    /// Who is invited, beside the account.
    pub guests: Vec<crate::Mailbox>,
}

impl EventDraft {
    /// The event as it is, to change.
    pub fn of(event: &CalendarEvent, account: &crate::EmailAddress) -> Self {
        Self {
            calendar: event.calendar.clone(),
            summary: event.summary.clone().unwrap_or_default(),
            location: event.location.clone().unwrap_or_default(),
            description: event.description.clone().unwrap_or_default(),
            when: event.when,
            time_zone: None,
            guests: event
                .attendees
                .iter()
                .map(|a| a.mailbox.clone())
                .filter(|m| m.address.key() != account.key())
                .collect(),
        }
    }
}

/// A change to the person's calendars an agent asks for that would tell
/// other people (invite, update or cancel guests, or answer an organizer),
/// waiting for the person to make it or not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventProposal {
    /// Its id among the account's proposals.
    pub id: u64,
    pub change: EventChange,
    /// The agent's name.
    pub agent: String,
    pub reason: Option<String>,
    pub made: Timestamp,
}

/// What an agent asks to change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventChange {
    /// Make an event (`event` none) or change one.
    Save {
        event: Option<String>,
        draft: EventDraft,
    },
    Delete {
        calendar: String,
        event: String,
        summary: Option<String>,
        when: EventWhen,
    },
    Answer {
        calendar: String,
        event: String,
        summary: Option<String>,
        when: EventWhen,
        answer: Answer,
    },
}

impl EventChange {
    /// When the event is, to show it.
    pub fn when(&self) -> EventWhen {
        match self {
            EventChange::Save { draft, .. } => draft.when,
            EventChange::Delete { when, .. } | EventChange::Answer { when, .. } => *when,
        }
    }

    /// The event's id, when it names one that exists.
    pub fn event(&self) -> Option<&str> {
        match self {
            EventChange::Save { event, .. } => event.as_deref(),
            EventChange::Delete { event, .. } | EventChange::Answer { event, .. } => Some(event),
        }
    }

    pub fn summary(&self) -> Option<&str> {
        match self {
            EventChange::Save { draft, .. } => Some(draft.summary.as_str()),
            EventChange::Delete { summary, .. } | EventChange::Answer { summary, .. } => {
                summary.as_deref()
            }
        }
    }
}
