//! The open view, and keeping views. A view opens as saved; changing it
//! is the view panel's (view_edit.rs).

use std::collections::HashMap;

use postrinch_commands::{LabelName, Place, Query, ViewEntry, ViewTally, label_matches};
use postrinch_model::{EmailAddress, ListOrder, SavedView, SystemLabel, Timestamp};
use rinch::batch;

use crate::appearance::Settings;
use crate::state::{PAGE, State, View};

/// The view the window opens on.
pub const FIRST: &str = "inbox";

/// `view` narrowed by the list's filter text: its search and the text,
/// both. The view's key stays, so Everything else is still itself.
pub fn filtered(mut view: SavedView, filter: &str) -> SavedView {
    let filter = filter.trim();
    if filter.is_empty() || postrinch_commands::Commands::check_query(filter).is_err() {
        return view;
    }
    view.query = if view.query.trim().is_empty() {
        filter.to_owned()
    } else {
        format!("({}) {filter}", view.query.trim())
    };
    view
}

impl State {
    /// Narrow the open view's list by `text`, from the list's filter box.
    pub fn filter_list(self, text: String) {
        batch(|| {
            self.shown.set(PAGE);
            self.list_filter.set(text);
        });
    }

    /// The open view's order, as the list shows it.
    pub fn shown_order(self) -> ListOrder {
        self.shown_view().map(|v| v.order).unwrap_or_default()
    }

    /// Change the open view's order, from the list's bar: through the
    /// panel when it is open, else saved at once.
    pub fn set_view_order(self, order: ListOrder) {
        if self.view_edit.with(|e| e.is_some()) {
            self.change_edit(|v| v.order = order);
            return;
        }
        let Some(mut view) = self.saved_view().map(|e| e.view) else {
            return;
        };
        if view.order == order {
            return;
        }
        view.order = order;
        if let Err(e) = self.store_view(view) {
            self.error
                .set(Some(format!("The order was not saved: {e}")));
        }
    }

    /// ✎ over the list: open the panel on what is shown, or close it.
    pub fn toggle_edit(self) {
        if self.view_edit.with(|e| e.is_some()) {
            self.finish_edit();
        } else if self.search.get().is_some() {
            self.edit_search();
        } else {
            self.edit_view(&self.open_view.get());
        }
    }

    /// Whether a sidebar row with rows under it shows them: as the person
    /// left it, else Inbox open and each account closed.
    pub fn row_open(self, key: &str) -> bool {
        self.view_tree
            .with(|t| t.get(key).copied())
            .unwrap_or(key == "inbox")
    }

    /// Open or close a row with rows under it, and keep that for next time.
    pub fn toggle_row(self, key: &str) {
        let open = !self.row_open(key);
        self.view_tree.update(|t| {
            t.insert(key.to_owned(), open);
        });
        let mut settings = Settings::load();
        settings.view_tree = self.view_tree.get();
        if let Err(e) = settings.save() {
            tracing::warn!("the sidebar's open rows were not saved: {e}");
        }
    }

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
            self.list_filter.set(String::new());
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
                    views.push(ViewEntry {
                        view: view.clone(),
                        place: Place::Yours,
                        original: None,
                        under: None,
                    });
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
