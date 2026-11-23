//! Who may see a calendar and how: its sharing rules, as Google keeps them.

use serde::{Deserialize, Serialize};

/// What someone may do with a calendar.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub enum CalendarRole {
    /// See only when it is busy.
    FreeBusy,
    /// See every event and what it says.
    #[default]
    Reader,
    /// Make and change events.
    Writer,
    /// Change events and who it is shared with.
    Owner,
}

impl CalendarRole {
    pub const ALL: [CalendarRole; 4] = [
        CalendarRole::FreeBusy,
        CalendarRole::Reader,
        CalendarRole::Writer,
        CalendarRole::Owner,
    ];

    /// Google's word for it.
    pub fn word(self) -> &'static str {
        match self {
            CalendarRole::FreeBusy => "freeBusyReader",
            CalendarRole::Reader => "reader",
            CalendarRole::Writer => "writer",
            CalendarRole::Owner => "owner",
        }
    }

    pub fn from_word(word: &str) -> Option<CalendarRole> {
        CalendarRole::ALL.into_iter().find(|r| r.word() == word)
    }

    /// What it lets them do, as the person reads it.
    pub fn says(self) -> &'static str {
        match self {
            CalendarRole::FreeBusy => "See only free/busy",
            CalendarRole::Reader => "See all event details",
            CalendarRole::Writer => "Make changes to events",
            CalendarRole::Owner => "Make changes and manage sharing",
        }
    }

    /// The account may change events.
    pub fn writes(self) -> bool {
        self >= CalendarRole::Writer
    }
}

/// Who a sharing rule is for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ShareWho {
    /// One person, by address.
    Person(String),
    /// A Google group, by address.
    Group(String),
    /// Everyone at a domain.
    Domain(String),
    /// Anyone at all: the calendar is public.
    Anyone,
}

impl ShareWho {
    /// The person, group or domain as the person reads it.
    pub fn says(&self) -> String {
        match self {
            ShareWho::Person(a) | ShareWho::Group(a) => a.clone(),
            ShareWho::Domain(d) => format!("Everyone at {d}"),
            ShareWho::Anyone => "Anyone (public)".to_owned(),
        }
    }
}

/// One sharing rule of a calendar.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CalendarShare {
    /// Google's id for the rule, to change or remove it by.
    pub rule: String,
    pub who: ShareWho,
    pub role: CalendarRole,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_go_both_ways() {
        for role in CalendarRole::ALL {
            assert_eq!(CalendarRole::from_word(role.word()), Some(role));
        }
        assert!(CalendarRole::Writer.writes() && !CalendarRole::Reader.writes());
    }
}
