//! The open view and what the person changed about it without saving: the
//! accounts it shows and filters added to its search. Its order is saved
//! with the view as soon as it is chosen. Opening another view leaves the
//! changes behind, so a view always opens as saved.

use std::collections::HashMap;

use postrinch_commands::{LabelName, Place, Query, ViewEntry, ViewTally, label_matches};
use postrinch_model::{EmailAddress, ListOrder, SavedView, SystemLabel, Timestamp};
use rinch::batch;

use crate::state::{PAGE, State, View};

/// The view the window opens on.
pub const FIRST: &str = "primary";

/// What the person changed about the open view and not saved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewChanges {
    /// The accounts shown in place of the view's own.
    pub accounts: Option<Vec<EmailAddress>>,
    /// Search terms added to the view's, each as typed (`has:attachment`).
    pub added: Vec<String>,
}

impl ViewChanges {
    pub fn any(&self) -> bool {
        self.accounts.is_some() || !self.added.is_empty()
    }

    /// `view` with these changes made.
    pub fn applied(&self, view: &SavedView) -> SavedView {
        let mut view = view.clone();
        if let Some(accounts) = &self.accounts {
            view.accounts.clone_from(accounts);
        }
        for term in &self.added {
            view.query = format!("{} {term}", view.query).trim().to_owned();
        }
        view
    }
}

impl State {
    /// The open view as saved, once the views are loaded.
    pub fn saved_view(self) -> Option<ViewEntry> {
        let key = self.open_view.get();
        self.views
            .with(|v| v.iter().find(|e| e.view.key == key).cloned())
    }

    /// The open view as shown: as saved, with the changes made.
    pub fn shown_view(self) -> Option<SavedView> {
        let entry = self.saved_view()?;
        Some(self.view_changes.with(|c| c.applied(&entry.view)))
    }

    /// The open view's search, read (`None` while there is no view, or
    /// for a search that does not read).
    fn shown_query(self) -> Option<Query> {
        let now = Timestamp::from_millis(jiff::Timestamp::now().as_millisecond());
        Query::parse(&self.shown_view()?.query, now).ok()
    }

    /// Whether the open view shows mail with one of these system labels:
    /// the inbox, say, so archiving takes a conversation out of it.
    pub fn shows_label(self, system: SystemLabel) -> bool {
        self.shown_query()
            .is_some_and(|q| q.labels.contains(&LabelName::System(system)))
    }

    /// The provider's ids of the person's labels the open view names, in
    /// every account: taking one off takes a conversation out of it.
    pub fn shown_label_ids(self) -> Vec<String> {
        let Some(query) = self.shown_query() else {
            return Vec::new();
        };
        let Some(commands) = crate::actions::commands() else {
            return Vec::new();
        };
        let accounts: Vec<EmailAddress> = self.sidebar.with(|s| {
            s.iter()
                .flat_map(|s| s.accounts.iter().map(|a| a.account.address.clone()))
                .collect()
        });
        let mut ids = Vec::new();
        for name in &query.labels {
            let LabelName::Named(name) = name else {
                continue;
            };
            for account in &accounts {
                for label in commands.user_labels(account).unwrap_or_default() {
                    if label_matches(&label.name, name) {
                        ids.push(label.provider_id);
                    }
                }
            }
        }
        ids
    }

    /// The one account the open view shows, if it shows only one.
    pub fn shown_account(self) -> Option<EmailAddress> {
        match self.shown_view()?.accounts.as_slice() {
            [one] => Some(one.clone()),
            _ => None,
        }
    }

    /// Open a view as saved.
    pub fn open_view(self, key: &str) {
        batch(|| {
            self.view.set(View::Mail);
            self.search.set(None);
            self.shown.set(PAGE);
            self.selected.set(None);
            self.view_changes.set(ViewChanges::default());
            self.open_view.set(key.to_owned());
        });
    }

    /// Show the open view's mail in these accounts (every account for
    /// none), unsaved.
    pub fn show_accounts(self, accounts: Vec<EmailAddress>) {
        let saved = self.saved_view().map(|e| e.view.accounts);
        self.view_changes.update(move |c| {
            c.accounts = (saved.as_ref() != Some(&accounts)).then_some(accounts);
        });
    }

    /// Narrow the open view by a search term, unsaved.
    pub fn add_filter(self, term: String) {
        self.view_changes.update(move |c| {
            if !c.added.contains(&term) {
                c.added.push(term);
            }
        });
    }

    pub fn remove_filter(self, term: &str) {
        self.view_changes.update(|c| c.added.retain(|t| t != term));
    }

    pub fn undo_view_changes(self) {
        self.view_changes.set(ViewChanges::default());
    }

    /// The open view's order: saved with it at once, as a view keeps its
    /// own order whether or not anything else about it is saved.
    pub fn set_view_order(self, order: ListOrder) {
        let Some(mut view) = self.saved_view().map(|e| e.view) else {
            return;
        };
        view.order = order;
        self.shown.set(PAGE);
        self.keep_view(view);
    }

    /// Save the open view with the changes made.
    pub fn save_view_changes(self) {
        if let Some(view) = self.shown_view()
            && self.put_view(view).is_ok()
        {
            self.view_changes.set(ViewChanges::default());
        }
    }

    /// Make a new view, in its dialog.
    pub fn new_view(self) {
        let dialog = crate::ui::view_dialog::ViewDialog::new_view("");
        self.view_dialog.set(Some(dialog));
    }

    /// [`State::put_view`], saying what went wrong in the window.
    pub fn keep_view(self, view: SavedView) {
        if let Err(e) = self.put_view(view) {
            self.error.set(Some(e));
        }
    }

    /// Save a view and show it, opening it when it is new. Why it could
    /// not be saved (a search that does not read), if it was not.
    pub fn put_view(self, view: SavedView) -> Result<(), String> {
        let commands = crate::actions::commands().ok_or("Views can't be saved now.")?;
        commands.save_view(&view).map_err(|e| e.to_string())?;
        self.revision.update(|n| *n += 1);
        let key = view.key.clone();
        let mut is_new = false;
        self.views
            .update(|views| match views.iter_mut().find(|e| e.view.key == key) {
                Some(entry) => entry.view = view.clone(),
                None => {
                    is_new = true;
                    let at = views
                        .iter()
                        .position(|e| e.place == Place::Bottom)
                        .unwrap_or(views.len());
                    views.insert(
                        at,
                        ViewEntry {
                            view: view.clone(),
                            place: Place::Yours,
                            original: None,
                        },
                    );
                }
            });
        if is_new {
            self.open_view(&key);
        }
        Ok(())
    }

    /// Remove one of the person's views, or put a built-in one back as it
    /// first was. Removing the open one opens the first view.
    pub fn remove_view(self, key: &str) {
        let key = key.to_owned();
        self.views.update(|views| {
            if let Some(at) = views.iter().position(|e| e.view.key == key) {
                match views[at].original.clone() {
                    Some(original) => views[at].view = original,
                    None => {
                        views.remove(at);
                    }
                }
            }
        });
        if self.open_view.get() == key && self.saved_view().is_none() {
            self.open_view(FIRST);
        } else if self.open_view.get() == key {
            self.undo_view_changes();
        }
        if let Some(commands) = crate::actions::commands() {
            match commands.remove_view(&key) {
                Ok(()) => self.revision.update(|n| *n += 1),
                Err(e) => self.error.set(Some(e.to_string())),
            }
        }
    }
}

/// Whether the sidebar lists a view: not hidden (unless the hidden ones
/// are asked for), and not empty when it hides itself then (or not yet
/// counted).
pub fn listed(view: &SavedView, counts: &HashMap<String, ViewTally>, show_hidden: bool) -> bool {
    (show_hidden || !view.hidden)
        && (!view.hide_when_empty || counts.get(&view.key).is_none_or(|t| t.all > 0))
}
