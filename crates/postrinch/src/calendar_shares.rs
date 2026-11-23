//! Calendars shared with the person by mail (DESIGN.md "Calendar"): the
//! loader finds Google's signed "shared a calendar with you" mail in the
//! open thread, and Add calendar puts the calendar in its account's list.

use std::collections::HashMap;

use postrinch_commands::{Commands, SharedCalendar, Thread, may_share_calendar};
use postrinch_model::EmailAddress;

use crate::actions::sync;
use crate::state::State;

/// A calendar a message shares, and how far adding it has got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shared {
    pub calendar: SharedCalendar,
    pub state: ShareState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareState {
    Offered,
    Adding,
    /// In the account's calendars, by this name.
    Added(String),
    /// It could not be added, and why, in a sentence.
    Failed(String),
}

/// The calendars the thread's messages share, by message id. Run on the
/// loader thread.
pub fn read(commands: &Commands, thread: &Thread) -> HashMap<String, Shared> {
    let account = &thread.summary.account;
    let mut found = HashMap::new();
    let listed = commands.calendars().unwrap_or_default();
    for m in thread.messages.iter().filter(|m| may_share_calendar(m)) {
        let Some(calendar) = commands.calendar_share(account, &m.id).ok().flatten() else {
            continue;
        };
        let state = listed
            .iter()
            .find(|(a, c)| a.key() == account.key() && c.id == calendar.id)
            .map_or(ShareState::Offered, |(_, c)| {
                ShareState::Added(c.name.clone())
            });
        found.insert(m.id.0.clone(), Shared { calendar, state });
    }
    found
}

impl State {
    /// Add the calendar message `id` shares to `account`'s calendars.
    pub fn add_shared_calendar(self, account: &EmailAddress, id: &str) {
        let shares = self.invitations.shares;
        let Some(shared) = shares.get().get(id).cloned() else {
            return;
        };
        let set = move |id: String, state: ShareState| {
            shares.update_send(move |s| {
                if let Some(found) = s.get_mut(&id) {
                    found.state = state;
                }
            });
        };
        let Some(handle) = sync() else {
            set(
                id.to_owned(),
                ShareState::Failed(crate::invitations::OFF.into()),
            );
            return;
        };
        set(id.to_owned(), ShareState::Adding);
        let (account, id, revision) = (account.clone(), id.to_owned(), self.revision);
        std::thread::spawn(move || {
            let sync = handle.sync_engine();
            let done = handle
                .runtime
                .block_on(sync.add_shared_calendar(&account, &shared.calendar.id));
            match done {
                Ok(info) => {
                    set(id, ShareState::Added(info.name));
                    revision.update_send(|n| *n += 1);
                }
                Err(e) => set(id, ShareState::Failed(crate::invitations::say(&e))),
            }
        });
    }
}
