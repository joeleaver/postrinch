//! Changing a view, making one, or refining a search, in the view panel:
//! one panel, opened by the pencil on a view's row (or + for a new view),
//! that holds everything about a view. While it is open the list shows
//! what the view would show, as changed. A search is a view not saved yet:
//! the panel changes its search as it goes, and saving it names it.

use postrinch_commands::Commands;
use postrinch_model::{ListOrder, SavedView, Timestamp, ViewCount};
use rinch::batch;

use crate::state::{PAGE, State, View};

/// What the panel is changing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editing {
    /// A view in the sidebar, the open one.
    View,
    /// A view not made yet.
    New,
    /// The search shown in place of a view.
    Search,
}

/// The panel's view, as changed so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewEdit {
    pub draft: SavedView,
    pub kind: Editing,
    /// Why its search does not read, or why it could not be saved.
    pub error: Option<String>,
}

impl ViewEdit {
    /// The view to list while the panel is open: the draft, when its
    /// search reads. A search lists through search instead.
    pub fn listed(&self) -> Option<SavedView> {
        (self.kind != Editing::Search && Commands::check_query(&self.draft.query).is_ok())
            .then(|| self.draft.clone())
    }
}

/// A view of the person's own, not made yet, showing what `query` finds.
fn blank(query: &str) -> SavedView {
    let now = Timestamp::from_millis(jiff::Timestamp::now().as_millisecond());
    SavedView {
        key: format!("v{}", now.as_millis()),
        name: String::new(),
        query: query.trim().to_owned(),
        accounts: Vec::new(),
        count: ViewCount::Unread,
        hide_when_empty: false,
        hidden: false,
        order: ListOrder::default(),
    }
}

impl State {
    /// Open the panel on a view, opening the view first.
    pub fn edit_view(self, key: &str) {
        if self.open_view.get() != key || self.search.get().is_some() {
            self.open_view(key);
        }
        if let Some(entry) = self.saved_view() {
            self.view_edit.set(Some(ViewEdit {
                draft: entry.view,
                kind: Editing::View,
                error: None,
            }));
        }
    }

    /// Open the panel on a new view, which shows all mail until changed.
    pub fn new_view(self) {
        batch(|| {
            self.close_search();
            self.view.set(View::Mail);
            self.selected.set(None);
            self.shown.set(PAGE);
            self.view_edit.set(Some(ViewEdit {
                draft: blank(""),
                kind: Editing::New,
                error: None,
            }));
        });
    }

    /// Open the panel on the search shown.
    pub fn edit_search(self) {
        let query = self
            .search
            .get()
            .unwrap_or_else(|| self.search_draft.get());
        let mut draft = blank(&query);
        draft.accounts = self.search_accounts.get();
        self.view_edit.set(Some(ViewEdit {
            draft,
            kind: Editing::Search,
            error: None,
        }));
    }

    /// Change the panel's view; a search runs again as changed.
    pub fn change_edit(self, change: impl FnOnce(&mut SavedView)) {
        let Some(mut edit) = self.view_edit.get() else {
            return;
        };
        change(&mut edit.draft);
        edit.error = Commands::check_query(&edit.draft.query)
            .err()
            .map(|e| e.to_string());
        let search = (edit.kind == Editing::Search && edit.error.is_none())
            .then(|| (edit.draft.query.clone(), edit.draft.accounts.clone()));
        batch(|| {
            self.view_edit.set(Some(edit));
            self.shown.set(PAGE);
            if let Some((query, accounts)) = search {
                self.selected.set(None);
                self.search_draft.set(query.clone());
                self.search_accounts.set(accounts);
                self.search.set(Some(query));
            }
        });
    }

    /// Close the panel, leaving the view as it was saved.
    pub fn cancel_edit(self) {
        batch(|| {
            self.view_edit.set(None);
            self.shown.set(PAGE);
        });
    }

    /// Save the panel's view: a new one (or a search) opens as a view.
    pub fn save_edit(self) {
        let Some(edit) = self.view_edit.get() else {
            return;
        };
        let mut view = edit.draft;
        view.name = view.name.trim().to_owned();
        view.query = view.query.trim().to_owned();
        let why = if view.name.is_empty() {
            Some(match edit.kind {
                Editing::Search => "Give it a name to keep it as a view.".to_owned(),
                _ => "Give the view a name.".to_owned(),
            })
        } else {
            self.put_view(view).err()
        };
        match why {
            Some(why) => self
                .view_edit
                .update(|e| e.iter_mut().for_each(|e| e.error = Some(why.clone()))),
            None => self.view_edit.set(None),
        }
    }

    /// Delete the panel's view, or put a built-in one back as it first
    /// was (and keep changing it).
    pub fn remove_edited(self) {
        let Some(edit) = self.view_edit.get() else {
            return;
        };
        let key = edit.draft.key;
        let original = self
            .views
            .with(|v| v.iter().find(|e| e.view.key == key).and_then(|e| e.original.clone()));
        self.remove_view(&key);
        match original {
            Some(original) => self.view_edit.set(Some(ViewEdit {
                draft: original,
                kind: Editing::View,
                error: None,
            })),
            None => self.view_edit.set(None),
        }
    }

    /// Narrow what is shown by a search term (a label's, from a row), in
    /// the panel, so it can be kept or not.
    pub fn edit_with(self, term: String) {
        if self.view_edit.get().is_none() {
            if self.search.get().is_some() {
                self.edit_search();
            } else {
                self.edit_view(&self.open_view.get());
            }
        }
        self.change_edit(|v| {
            if !v.query.split_whitespace().any(|t| t == term) {
                v.query = format!("{} {term}", v.query).trim().to_owned();
            }
        });
    }
}
