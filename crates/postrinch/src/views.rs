//! The open view, and keeping views. A view opens as saved; changing it
//! is the view panel's (view_edit.rs).

use std::collections::HashMap;

use postrinch_commands::{LabelName, Place, Query, ViewEntry, ViewTally, label_matches};
use postrinch_model::{EmailAddress, SavedView, SystemLabel, Timestamp};
use rinch::batch;

use crate::state::{PAGE, State, View};

/// The view the window opens on.
pub const FIRST: &str = "inbox";

impl State {
    /// The open view as saved, once the views are loaded.
    pub fn saved_view(self) -> Option<ViewEntry> {
        let key = self.open_view.get();
        self.views
            .with(|v| v.iter().find(|e| e.view.key == key).cloned())
    }

    /// The open view as shown: as the panel has it while it is changed
    /// (or made), else as saved.
    pub fn shown_view(self) -> Option<SavedView> {
        if let Some(draft) = self.view_edit.with(|e| e.as_ref().and_then(|e| e.listed())) {
            return Some(draft);
        }
        Some(self.saved_view()?.view)
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

    /// Open a view as saved, keeping the panel's changes first.
    pub fn open_view(self, key: &str) {
        self.finish_edit();
        batch(|| {
            self.view.set(View::Mail);
            self.search.set(None);
            self.shown.set(PAGE);
            self.selected.set(None);
            self.search_accounts.set(Vec::new());
            self.view_edit.set(None);
            self.open_view.set(key.to_owned());
        });
    }

    /// Save a view and show it in the sidebar. Why it could not be saved
    /// (a search that does not read), if it was not.
    pub fn store_view(self, view: SavedView) -> Result<(), String> {
        let commands = crate::actions::commands().ok_or("Views can't be saved now.")?;
        commands.save_view(&view).map_err(|e| e.to_string())?;
        self.revision.update(|n| *n += 1);
        let key = view.key.clone();
        self.views
            .update(|views| match views.iter_mut().find(|e| e.view.key == key) {
                Some(entry) => entry.view = view.clone(),
                None => {
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
                            nested: false,
                        },
                    );
                }
            });
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
