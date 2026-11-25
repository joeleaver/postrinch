//! A calendar opened in the calendar's right pane (DESIGN.md "Calendar"):
//! a click on its name in the sidebar. One of the account's own lists who
//! it is shared with, each with what they may do, to change or stop, and
//! Share adds someone (Google emails them). One shared with the account
//! offers Remove from my calendars.

use postrinch_model::{CalendarRole, CalendarShare, EmailAddress, ShareWho};
use rinch::prelude::*;
use rinch::{Effect, untracked};

use crate::actions::sync;
use crate::state::State;
use crate::sync::SyncHandle;

/// Who the open calendar is shared with, as far as Google has said.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Sharing {
    #[default]
    Loading,
    Shares(Vec<CalendarShare>),
    /// Only its owners may see who it is shared with.
    NotOwner,
    Failed(String),
}

/// The calendar pane's signals. `Copy`, like the window's state.
#[derive(Clone, Copy)]
pub struct Panel {
    /// The calendar open in the pane: its account and id.
    pub open: Signal<Option<(EmailAddress, String)>>,
    pub sharing: Signal<Sharing>,
    /// A change on its way.
    pub busy: Signal<bool>,
    /// Why the last change failed, in a sentence.
    pub error: Signal<Option<String>>,
    /// Who to share with next, as typed, and how.
    pub who: Signal<String>,
    pub role: Signal<CalendarRole>,
    /// Counts the changes made, to ask Google again after each.
    pub reload: Signal<u32>,
}

impl Panel {
    pub fn new() -> Self {
        Self {
            open: Signal::new(None),
            sharing: Signal::new(Sharing::Loading),
            busy: Signal::new(false),
            error: Signal::new(None),
            who: Signal::new(String::new()),
            role: Signal::new(CalendarRole::Reader),
            reload: Signal::new(0),
        }
    }
}

/// A sync call for the open calendar, from a thread of its own.
fn run<T: Send + 'static>(
    handle: SyncHandle,
    work: impl FnOnce(&SyncHandle) -> postrinch_sync::SyncResult<T> + Send + 'static,
    done: impl FnOnce(Result<T, String>) + Send + 'static,
) {
    std::thread::spawn(move || {
        done(work(&handle).map_err(|e| crate::invitations::say(&e)));
    });
}

impl State {
    /// Open a calendar in the pane, closing any event, and ask who it is
    /// shared with.
    pub fn open_calendar_panel(self, account: &EmailAddress, calendar: &str) {
        self.calendar.open.set(None);
        self.calendar.editing.set(None);
        let panel = self.panel;
        panel.error.set(None);
        panel.who.set(String::new());
        panel.open.set(Some((account.clone(), calendar.to_owned())));
        self.load_shares();
    }

    /// Ask Google again who the open calendar is shared with.
    fn load_shares(self) {
        let panel = self.panel;
        let Some((account, calendar)) = panel.open.get() else {
            return;
        };
        let owner = self
            .calendar
            .calendars
            .get()
            .iter()
            .any(|(a, c)| *a == account && c.id == calendar && c.role == CalendarRole::Owner);
        if !owner {
            panel.sharing.set(Sharing::NotOwner);
            return;
        }
        let Some(handle) = sync() else {
            panel
                .sharing
                .set(Sharing::Failed(crate::invitations::OFF.into()));
            return;
        };
        panel.sharing.set(Sharing::Loading);
        let asked = (account.clone(), calendar.clone());
        run(
            handle,
            move |h| {
                h.on_calendar(&account, async |sync| {
                    sync.calendar_shares(&account, &calendar).await
                })
            },
            move |got| {
                let got = match got {
                    Ok(shares) => Sharing::Shares(shares),
                    Err(why) => Sharing::Failed(why),
                };
                // Run on the window's thread, which alone reads signals:
                // only for the calendar still open.
                panel.sharing.update_send(move |s| {
                    if untracked(|| panel.open.get()).as_ref() == Some(&asked) {
                        *s = got;
                    }
                });
            },
        );
    }

    /// Share the open calendar with whoever is typed, as chosen.
    pub fn share_open_calendar(self) {
        let panel = self.panel;
        let typed = panel.who.get().trim().to_owned();
        let who = match postrinch_model::EmailAddress::parse(&typed) {
            Ok(address) => ShareWho::Person(address.to_string()),
            Err(_) => {
                panel
                    .error
                    .set(Some(format!("\u{201c}{typed}\u{201d} is not an address.")));
                return;
            }
        };
        let role = panel.role.get();
        self.change_sharing(
            move |h, account, calendar| {
                h.on_calendar(account, async |sync| {
                    sync.share_calendar(account, calendar, &who, role).await
                })
                .map(|_| ())
            },
            true,
        );
    }

    /// Change what one share lets them do.
    pub fn change_share(self, share: CalendarShare, role: CalendarRole) {
        if share.role == role {
            return;
        }
        self.change_sharing(
            move |h, account, calendar| {
                h.on_calendar(account, async |sync| {
                    sync.share_calendar(account, calendar, &share.who, role)
                        .await
                })
                .map(|_| ())
            },
            false,
        );
    }

    /// Stop sharing the open calendar by one rule.
    pub fn stop_share(self, share: CalendarShare) {
        self.change_sharing(
            move |h, account, calendar| {
                h.on_calendar(account, async |sync| {
                    sync.unshare_calendar(account, calendar, &share.rule).await
                })
            },
            false,
        );
    }

    fn change_sharing(
        self,
        work: impl FnOnce(&SyncHandle, &EmailAddress, &str) -> postrinch_sync::SyncResult<()>
        + Send
        + 'static,
        clear_who: bool,
    ) {
        let panel = self.panel;
        let Some((account, calendar)) = panel.open.get() else {
            return;
        };
        let Some(handle) = sync() else {
            panel.error.set(Some(crate::invitations::OFF.into()));
            return;
        };
        panel.busy.set(true);
        panel.error.set(None);
        run(
            handle,
            move |h| work(h, &account, &calendar),
            move |done| {
                panel.busy.send(false);
                match done {
                    Ok(()) => {
                        if clear_who {
                            panel.who.send(String::new());
                        }
                        // Read back, as Google has it now.
                        panel.reload.update_send(|n| *n += 1);
                    }
                    Err(why) => panel.error.send(Some(why)),
                }
            },
        );
    }

    /// Take the open calendar off its account's calendars.
    pub fn remove_open_calendar(self) {
        let panel = self.panel;
        let Some((account, calendar)) = panel.open.get() else {
            return;
        };
        let Some(handle) = sync() else {
            panel.error.set(Some(crate::invitations::OFF.into()));
            return;
        };
        panel.busy.set(true);
        let revision = self.revision;
        run(
            handle,
            move |h| {
                h.on_calendar(&account, async |sync| {
                    sync.remove_shared_calendar(&account, &calendar).await
                })
            },
            move |done| {
                panel.busy.send(false);
                match done {
                    Ok(()) => {
                        panel.open.send(None);
                        revision.update_send(|n| *n += 1);
                    }
                    Err(why) => panel.error.send(Some(why)),
                }
            },
        );
    }

    /// Close the pane's calendar when an event opens or one is made, and
    /// ask again after a change.
    pub fn watch_calendar_panel(self) {
        Effect::new(move || {
            let event = self.calendar.open.with(|o| o.is_some())
                || self.calendar.editing.with(|e| e.is_some());
            if event {
                untracked(|| self.panel.open.set(None));
            }
        });
        Effect::new(move || {
            self.panel.reload.get();
            untracked(|| self.load_shares());
        });
    }
}
