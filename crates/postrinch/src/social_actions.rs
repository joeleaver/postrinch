//! What the person does in Social mode, through the `social/` commands:
//! opening a view and choosing its tab, the view panel (open, change,
//! Save, Delete), following a channel, and a place's or group's notices.
//! Each change is written at once, then what is shown loads again.

use postrinch_commands::{HOME, SocialPlaceInfo};
use postrinch_store::{GroupRef, Notices, PlaceRef};
use rinch::batch;

use crate::social_edit::{Removal, SaveStep, SocialEdit};
use crate::state::State;

/// Said in the panel when Save is asked of a view with no name.
const NEEDS_NAME: &str = "Give the view a name to save it.";

impl State {
    /// Open a view, on All. An open panel closes without saving, as
    /// Cancel would.
    pub fn open_social_view(self, id: &str) {
        batch(|| {
            self.social.edit.set(None);
            self.social.tab.set(None);
            self.social.open.set(id.to_owned());
        });
    }

    /// Choose a tab: a place of the open view, or `None` for All.
    pub fn choose_social_tab(self, tab: Option<PlaceRef>) {
        self.social.tab.set(tab);
    }

    /// ✎ on a view's row: open the panel on it, or close it when it is
    /// open on it already. The view opens too, so the panel stands beside
    /// what it changes.
    pub fn toggle_social_edit(self, id: &str) {
        if self
            .social
            .edit
            .with(|e| e.as_ref().is_some_and(|e| e.is_of(id)))
        {
            self.social.edit.set(None);
            return;
        }
        let Some(entry) = self
            .social
            .views
            .with(|v| v.iter().find(|e| e.view.id == id).cloned())
        else {
            return;
        };
        batch(|| {
            if self.social.open.get() != id {
                self.social.tab.set(None);
                self.social.open.set(id.to_owned());
            }
            self.social.edit.set(Some(SocialEdit::of(&entry)));
        });
    }

    /// "New view…": the panel on a view not made yet.
    pub fn new_social_view(self) {
        self.social.edit.set(Some(SocialEdit::new_view()));
    }

    /// Change the panel's draft.
    pub fn change_social_edit(self, f: impl FnOnce(&mut SocialEdit)) {
        self.social.edit.update(|e| {
            if let Some(e) = e {
                f(e);
                e.error = None;
            }
        });
    }

    /// Cancel: close the panel, keeping nothing.
    pub fn cancel_social_edit(self) {
        self.social.edit.set(None);
    }

    /// Save: keep the draft, close the panel and open the view as saved.
    /// What the commands refuse is said in the panel, which stays open.
    pub fn save_social_edit(self) {
        let Some(edit) = self.social.edit.get() else {
            return;
        };
        match edit.save_step() {
            SaveStep::NeedsName => {
                self.change_error(NEEDS_NAME.to_owned());
                return;
            }
            SaveStep::Close => {
                self.social.edit.set(None);
                return;
            }
            SaveStep::Write => {}
        }
        let Some(commands) = crate::actions::commands() else {
            return;
        };
        match commands.save_social_view(&edit.draft) {
            Ok(saved) => batch(|| {
                self.social.edit.set(None);
                if self.social.open.get() != saved.id {
                    self.social.tab.set(None);
                    self.social.open.set(saved.id);
                }
                self.bump_social();
            }),
            Err(e) => self.change_error(e.to_string()),
        }
    }

    /// The foot's left button, once confirmed where it asks: delete the
    /// person's view (opening Home if it was open), or put a built-in view
    /// back as it first was, its panel staying open on it.
    pub fn remove_social_edit(self) {
        let Some(edit) = self.social.edit.get() else {
            return;
        };
        let (Some(removal), Some(commands)) = (edit.removal(), crate::actions::commands()) else {
            return;
        };
        let id = edit.before.id.clone();
        if let Err(e) = commands.remove_social_view(&id) {
            self.change_error(e.to_string());
            return;
        }
        batch(|| {
            match (removal, edit.put_back()) {
                (Removal::PutBack, Some(back)) => self.social.edit.set(Some(back)),
                _ => {
                    self.social.edit.set(None);
                    if self.social.open.get() == id {
                        self.social.tab.set(None);
                        self.social.open.set(HOME.to_owned());
                    }
                }
            }
            self.bump_social();
        });
    }

    /// Follow a channel, or stop following it.
    pub fn follow_social_place(self, place: &SocialPlaceInfo) {
        let Some(commands) = crate::actions::commands() else {
            return;
        };
        let on = !place.place.followed;
        match commands.follow_social_place(&place.place.account, &place.place.id, on) {
            Ok(_) => self.bump_social(),
            Err(e) => self.error.set(Some(e.to_string())),
        }
    }

    /// How a channel tells the person of new messages.
    pub fn set_place_notices(self, place: &PlaceRef, notices: Option<Notices>) {
        let Some(commands) = crate::actions::commands() else {
            return;
        };
        match commands.set_social_place_notices(&place.account, &place.id, notices) {
            Ok(_) => self.bump_social(),
            Err(e) => self.change_error(e.to_string()),
        }
    }

    /// How a group's channels tell the person of new messages.
    pub fn set_group_notices(self, group: &GroupRef, notices: Option<Notices>) {
        let Some(commands) = crate::actions::commands() else {
            return;
        };
        match commands.set_social_group_notices(&group.account, &group.uri, notices) {
            Ok(_) => self.bump_social(),
            Err(e) => self.change_error(e.to_string()),
        }
    }

    /// What went wrong, said in the panel.
    fn change_error(self, why: String) {
        self.social.edit.update(|e| {
            if let Some(e) = e {
                e.error = Some(why);
            }
        });
    }

    fn bump_social(self) {
        self.social.changed.update(|n| *n += 1);
    }
}
