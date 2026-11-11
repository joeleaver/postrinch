//! How the person reads: when a conversation is marked read, and what
//! opens after the open one is archived or trashed. Kept in Postrinch's
//! settings file (see appearance.rs).

use std::cell::RefCell;
use std::thread;
use std::time::Instant;

use postrinch_commands::ThreadAction;
use rinch::{Effect, untracked};
use serde::{Deserialize, Serialize};

use crate::appearance::Settings;
use crate::state::State;
use crate::ui::list::RowId;

thread_local! {
    /// The opened row, and when it is due to be marked read.
    static DUE: RefCell<Option<(RowId, Instant)>> = const { RefCell::new(None) };
}

/// When an opened conversation is marked read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkRead {
    /// The moment it shows in the reading pane.
    #[default]
    OnOpen,
    /// Once it has been open for [`MarkRead::DELAY`], so stepping through
    /// the list leaves mail unread.
    AfterDelay,
    /// Only when the person says so.
    Manually,
}

impl MarkRead {
    pub const DELAY: std::time::Duration = std::time::Duration::from_secs(3);
}

/// What opens after the open conversation is archived or trashed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AfterMove {
    /// The one below it in the list.
    #[default]
    OpenNext,
    /// Nothing: back to the list.
    BackToList,
}

impl State {
    /// Mark each conversation the person opens read, when their setting
    /// says: at once, after a while, or never. Opening it again marks it
    /// again; it staying open does not, so marking it unread sticks.
    pub fn mark_read_when_opened(self) {
        Effect::new(move || {
            let opened = self.selected.get();
            let due = match (opened, untracked(|| self.mark_read.get())) {
                (Some(thread), MarkRead::OnOpen) => Some((thread, Instant::now())),
                (Some(thread), MarkRead::AfterDelay) => {
                    let tick = self.read_due;
                    let timer = thread::Builder::new()
                        .name("mark-read".into())
                        .spawn(move || {
                            thread::sleep(MarkRead::DELAY);
                            tick.update_send(|n| *n += 1);
                        });
                    if let Err(e) = timer {
                        tracing::warn!("cannot time marking read: {e}");
                    }
                    Some((thread, Instant::now() + MarkRead::DELAY))
                }
                _ => None,
            };
            DUE.with(|d| *d.borrow_mut() = due);
        });
        Effect::new(move || {
            self.read_due.get();
            let Some(thread) = self.thread.get() else {
                return;
            };
            let ready = DUE.with(|d| {
                let mut d = d.borrow_mut();
                let ready = match &*d {
                    Some((row, at)) => {
                        row.0 == thread.summary.account
                            && row.1 == thread.summary.id
                            && *at <= Instant::now()
                    }
                    None => false,
                };
                if ready {
                    d.take().map(|(row, _)| row)
                } else {
                    None
                }
            });
            let Some(row) = ready else {
                return;
            };
            // One message at a time marks that message; else the conversation.
            let unread = match &row.2 {
                Some(id) => thread
                    .messages
                    .iter()
                    .any(|m| &m.id == id && m.flags.unread),
                None => thread.summary.unread_count > 0,
            };
            if unread {
                untracked(|| self.act(vec![row], ThreadAction::MarkRead));
            }
        });
    }

    pub fn set_mark_read(self, choice: MarkRead) {
        self.mark_read.set(choice);
        self.save_settings(|s| s.mark_read = choice);
    }

    pub fn set_after_move(self, choice: AfterMove) {
        self.after_move.set(choice);
        self.save_settings(|s| s.after_move = choice);
    }

    /// Change and save the settings file, saying so when it cannot be
    /// saved.
    pub fn save_settings(self, change: impl FnOnce(&mut Settings)) {
        let mut settings = Settings::load();
        change(&mut settings);
        if let Err(e) = settings.save() {
            self.error
                .set(Some(format!("The setting was not saved: {e}")));
        }
    }
}
