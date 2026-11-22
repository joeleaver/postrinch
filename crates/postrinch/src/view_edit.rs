//! Changing a view, making one, or refining a search, in the view panel:
//! one panel, opened by the pencil on a view's row (or + for a new view),
//! that holds everything about a view. While it is open the list shows
//! what the view would show, as changed.
//!
//! A view's changes keep themselves: each is saved a moment after the
//! last (`SAVE_AFTER`), and at once when the panel closes or another view
//! opens. Undo changes puts the view back as it was when the panel
//! opened. A new view is made once it has a name. A search is a view not
//! made: the panel changes its search as it goes, and Keep as view makes
//! it one.

use std::thread;
use std::time::Duration;

use postrinch_commands::Commands;
use postrinch_model::{ListOrder, SavedView, Timestamp, ViewCount};
use rinch::{Effect, batch, untracked};

use crate::state::{PAGE, State, View};

/// How long after the last change a view's changes are saved.
const SAVE_AFTER: Duration = Duration::from_millis(600);

/// What the panel is changing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editing {
    /// A view in the sidebar, the open one.
    View,
    /// A view not made yet: made once it has a name.
    New,
    /// The search shown in place of a view.
    Search,
}

/// The panel's view, as changed so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewEdit {
    pub draft: SavedView,
    pub kind: Editing,
    /// The view as it was when the panel opened, for Undo changes.
    pub original: SavedView,
    /// Why its search does not read, or why it could not be kept.
    pub error: Option<String>,
    /// Changes made, and how many of them are saved.
    changes: u64,
    saved: u64,
}

impl ViewEdit {
    fn new(draft: SavedView, kind: Editing) -> Self {
        Self {
            original: draft.clone(),
            draft,
            kind,
            error: None,
            changes: 0,
            saved: 0,
        }
    }

    /// The view to list while the panel is open: the draft, when its
    /// search reads. A search lists through search instead.
    pub fn listed(&self) -> Option<SavedView> {
        (self.kind != Editing::Search && Commands::check_query(&self.draft.query).is_ok())
            .then(|| self.draft.clone())
    }

    /// Whether the view differs from when the panel opened.
    pub fn changed(&self) -> bool {
        self.kind == Editing::View && self.draft != self.original
    }

    /// Why the draft can't be kept as it is, if it can't.
    fn trouble(&self) -> Option<String> {
        if let Err(e) = Commands::check_query(&self.draft.query) {
            return Some(e.to_string());
        }
        match (self.draft.name.trim().is_empty(), self.kind) {
            (true, Editing::Search) => Some("Give it a name to keep it as a view.".into()),
            (true, Editing::New) => Some("Give it a name to make it.".into()),
            (true, Editing::View) => Some("A view needs a name.".into()),
            (false, _) => None,
        }
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
        rows: None,
    }
}

impl State {
    /// Save the panel's changes a moment after the last one.
    pub fn save_edits_as_they_come(self) {
        Effect::new(move || {
            let due = self.save_due.get();
            let waiting = untracked(|| {
                self.view_edit.with(|e| {
                    e.as_ref()
                        .is_some_and(|e| e.changes == due && e.saved != due)
                })
            });
            if waiting {
                untracked(|| self.keep_edit());
            }
        });
    }

    /// Save the panel's view now, if it changed and can be kept. A new
    /// view, once made, is changed as a view from then on.
    fn keep_edit(self) {
        let Some(mut edit) = self.view_edit.get() else {
            return;
        };
        if edit.kind == Editing::Search || edit.saved == edit.changes {
            return;
        }
        if let Some(why) = edit.trouble() {
            // A new view without a name yet waits quietly for one; the
            // panel says so.
            let quiet = edit.kind == Editing::New && edit.draft.name.trim().is_empty();
            if !quiet {
                edit.error = Some(why);
                self.view_edit.set(Some(edit));
            }
            return;
        }
        let mut view = edit.draft.clone();
        view.name = view.name.trim().to_owned();
        view.query = view.query.trim().to_owned();
        match self.store_view(view) {
            Ok(()) => {
                edit.saved = edit.changes;
                edit.error = None;
                let made = edit.kind == Editing::New;
                if made {
                    // The original is the view as first made, which Undo
                    // changes cannot take back to nothing.
                    edit.kind = Editing::View;
                    edit.original = edit.draft.clone();
                }
                let key = edit.draft.key.clone();
                batch(|| {
                    if made {
                        self.open_view.set(key);
                    }
                    self.view_edit.set(Some(edit));
                });
            }
            Err(why) => {
                edit.error = Some(why);
                self.view_edit.set(Some(edit));
            }
        }
    }

    /// Open the panel on a view, opening the view first.
    pub fn edit_view(self, key: &str) {
        if self.open_view.get() != key || self.search.get().is_some() {
            self.open_view(key);
        }
        if let Some(entry) = self.saved_view() {
            self.view_edit
                .set(Some(ViewEdit::new(entry.view, Editing::View)));
        }
    }

    /// Open the panel on a new view, which shows all mail until changed.
    pub fn new_view(self) {
        self.finish_edit();
        batch(|| {
            self.close_search();
            self.view.set(View::Mail);
            self.selected.set(None);
            self.shown.set(PAGE);
            self.view_edit
                .set(Some(ViewEdit::new(blank(""), Editing::New)));
        });
    }

    /// Open the panel on the search shown.
    pub fn edit_search(self) {
        let query = self.search.get().unwrap_or_else(|| self.search_draft.get());
        let mut draft = blank(&query);
        draft.accounts = self.search_accounts.get();
        self.view_edit
            .set(Some(ViewEdit::new(draft, Editing::Search)));
    }

    /// Change the panel's view; a search runs again as changed, and a
    /// view saves itself a moment later.
    pub fn change_edit(self, change: impl FnOnce(&mut SavedView)) {
        let Some(mut edit) = self.view_edit.get() else {
            return;
        };
        let rows = edit.draft.rows;
        change(&mut edit.draft);
        // A row is a conversation or a message: the open one is neither
        // once that changes.
        let regrouped = edit.draft.rows != rows;
        edit.changes += 1;
        edit.error = Commands::check_query(&edit.draft.query)
            .err()
            .map(|e| e.to_string());
        let search = (edit.kind == Editing::Search && edit.error.is_none())
            .then(|| (edit.draft.query.clone(), edit.draft.accounts.clone()));
        if edit.kind != Editing::Search {
            self.save_soon(edit.changes);
        }
        batch(|| {
            if regrouped {
                self.selected.set(None);
                self.whole_conversation.set(false);
            }
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

    /// Tell `save_edits_as_they_come` about change `n` after SAVE_AFTER.
    fn save_soon(self, n: u64) {
        let due = self.save_due;
        let timer = thread::Builder::new()
            .name("view-save".into())
            .spawn(move || {
                thread::sleep(SAVE_AFTER);
                due.update_send(move |d| *d = n);
            });
        if let Err(e) = timer {
            // Kept when the panel closes instead.
            tracing::warn!("cannot time saving a view: {e}");
        }
    }

    /// Close the panel, keeping what can be kept of its changes.
    pub fn finish_edit(self) {
        self.keep_edit();
        batch(|| {
            self.view_edit.set(None);
            self.shown.set(PAGE);
        });
    }

    /// Put the view back as it was when the panel opened, and save that.
    pub fn undo_edit(self) {
        let Some(original) = self
            .view_edit
            .with(|e| e.as_ref().map(|e| e.original.clone()))
        else {
            return;
        };
        self.change_edit(move |v| *v = original);
        self.keep_edit();
    }

    /// Make the search a view of the person's own, and open it.
    pub fn keep_search(self) {
        let Some(edit) = self.view_edit.get() else {
            return;
        };
        if let Some(why) = edit.trouble() {
            self.view_edit
                .update(|e| e.iter_mut().for_each(|e| e.error = Some(why.clone())));
            return;
        }
        let mut view = edit.draft;
        view.name = view.name.trim().to_owned();
        view.query = view.query.trim().to_owned();
        let key = view.key.clone();
        match self.store_view(view) {
            Ok(()) => self.open_view(&key),
            Err(why) => self
                .view_edit
                .update(|e| e.iter_mut().for_each(|e| e.error = Some(why.clone()))),
        }
    }

    /// Delete the panel's view, or put a built-in one back as it first
    /// was (and keep changing it).
    pub fn remove_edited(self) {
        let Some(edit) = self.view_edit.get() else {
            return;
        };
        let key = edit.draft.key;
        let original = self.views.with(|v| {
            v.iter()
                .find(|e| e.view.key == key)
                .and_then(|e| e.original.clone())
        });
        // Closed first, so nothing of it is kept again on the way out.
        self.view_edit.set(None);
        self.remove_view(&key);
        match original {
            Some(original) => self
                .view_edit
                .set(Some(ViewEdit::new(original, Editing::View))),
            None => self.view_edit.set(None),
        }
    }

    /// Narrow what is shown by a search term (a label's, from a row): a
    /// search of the view and the term, so the view itself stays as it is.
    pub fn edit_with(self, term: String) {
        let searching = self
            .view_edit
            .with(|e| e.as_ref().is_some_and(|e| e.kind == Editing::Search));
        if !searching {
            let (query, accounts) = match self.search.get() {
                Some(query) => (query, self.search_accounts.get()),
                None => self
                    .shown_view()
                    .map(|v| (v.query, v.accounts))
                    .unwrap_or_default(),
            };
            self.finish_edit();
            batch(|| {
                self.search_draft.set(query.clone());
                self.search_accounts.set(accounts);
                self.selected.set(None);
                self.search.set(Some(query));
            });
            self.edit_search();
        }
        self.change_edit(|v| {
            if !v.query.split_whitespace().any(|t| t == term) {
                v.query = format!("{} {term}", v.query).trim().to_owned();
            }
        });
    }
}
