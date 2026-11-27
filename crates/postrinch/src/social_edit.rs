//! The social view panel's draft (the "Edit a view" state of the Social
//! mockup): a view's name, its places and groups, the kinds it shows, its
//! filter, how it reads and its count, changed in the panel and saved by
//! Save. A plain value, so its rules are tested without a window.

use postrinch_commands::{BuiltInView, SocialViewEntry};
use postrinch_store::{ContentKind, GroupRef, PlaceRef, SocialView};

/// The kinds the panel's Show offers, in its order.
pub const KINDS: [ContentKind; 3] = [ContentKind::Chat, ContentKind::Memo, ContentKind::Article];

/// A view being changed or made in the panel.
#[derive(Debug, Clone, PartialEq)]
pub struct SocialEdit {
    /// The view as it was when the panel opened (empty for a new one).
    pub before: SocialView,
    /// The view as changed so far.
    pub draft: SocialView,
    /// What a built-in view shows; `None` for one of the person's own.
    pub built_in: Option<BuiltInView>,
    /// A built-in view as it first was, to put back.
    pub original: Option<SocialView>,
    /// Why the last Save or Delete did not happen.
    pub error: Option<String>,
}

impl SocialEdit {
    /// The panel on a view in the sidebar.
    pub fn of(entry: &SocialViewEntry) -> Self {
        Self {
            before: entry.view.clone(),
            draft: entry.view.clone(),
            built_in: entry.built_in,
            original: entry.original.clone(),
            error: None,
        }
    }

    /// The panel on a view not made yet.
    pub fn new_view() -> Self {
        let view = SocialView {
            id: String::new(),
            order: 0,
            name: String::new(),
            places: Vec::new(),
            groups: Vec::new(),
            filters: Default::default(),
            read_as: Default::default(),
            count: Default::default(),
        };
        Self {
            before: view.clone(),
            draft: view,
            built_in: None,
            original: None,
            error: None,
        }
    }

    /// Whether this is a view not made yet.
    pub fn is_new(&self) -> bool {
        self.before.id.is_empty()
    }

    /// Whether the view is the one with `id`.
    pub fn is_of(&self, id: &str) -> bool {
        !self.is_new() && self.before.id == id
    }

    /// Whether Save would change anything.
    pub fn changed(&self) -> bool {
        self.draft != self.before
    }

    /// Whether Save may be pressed: the view has a name.
    pub fn can_save(&self) -> bool {
        !self.draft.name.trim().is_empty()
    }

    /// What Save does with the panel as it is: refuse a view with no
    /// name, close a panel with nothing changed, else write the view.
    pub fn save_step(&self) -> SaveStep {
        if !self.can_save() {
            SaveStep::NeedsName
        } else if !self.changed() {
            SaveStep::Close
        } else {
            SaveStep::Write
        }
    }

    /// The panel once its built-in view is put back as it first was: open
    /// on the original, nothing changed. `None` for a view of the
    /// person's own, whose panel closes once it is deleted.
    pub fn put_back(&self) -> Option<SocialEdit> {
        let original = self.original.as_ref()?;
        Some(SocialEdit {
            before: original.clone(),
            draft: original.clone(),
            error: None,
            ..self.clone()
        })
    }

    /// What the left button of the panel's foot does, if anything: delete
    /// one of the person's views, or put a built-in one back as it first
    /// was (nothing when it is as it first was, or not made yet).
    pub fn removal(&self) -> Option<Removal> {
        if self.is_new() {
            return None;
        }
        match &self.original {
            None => Some(Removal::Delete),
            Some(original) if *original != self.before => Some(Removal::PutBack),
            Some(_) => None,
        }
    }

    /// Whether the view's filters narrow what it shows: not for Drafts or
    /// All channels, which list drafts and places.
    pub fn filters_apply(&self) -> bool {
        self.built_in.is_none_or(BuiltInView::shows_messages)
    }

    /// Whether Show has `kind` ticked: every kind is when none is named.
    pub fn shows(&self, kind: ContentKind) -> bool {
        let kinds = &self.draft.filters.kinds;
        kinds.is_empty() || kinds.contains(&kind)
    }

    /// Tick or untick `kind` in Show. The last kind ticked stays: a view
    /// shows something. All three ticked is saved as none named, so kinds
    /// added to the protocol later show too.
    pub fn toggle_kind(&mut self, kind: ContentKind) {
        let mut on: Vec<ContentKind> = KINDS.into_iter().filter(|&k| self.shows(k)).collect();
        if let Some(at) = on.iter().position(|&k| k == kind) {
            if on.len() == 1 {
                return;
            }
            on.remove(at);
        } else {
            on.push(kind);
        }
        let on: Vec<ContentKind> = KINDS.into_iter().filter(|k| on.contains(k)).collect();
        self.draft.filters.kinds = if on.len() == KINDS.len() {
            Vec::new()
        } else {
            on
        };
    }

    /// Add a place to the view, once.
    pub fn add_place(&mut self, place: PlaceRef) {
        if !self.has_place(&place) {
            self.draft.places.push(place);
        }
    }

    /// Add a group, all of its channels, to the view, once.
    pub fn add_group(&mut self, group: GroupRef) {
        if !self.has_group(&group) {
            self.draft.groups.push(group);
        }
    }

    pub fn remove_place(&mut self, place: &PlaceRef) {
        self.draft.places.retain(|p| !same_place(p, place));
    }

    pub fn remove_group(&mut self, group: &GroupRef) {
        self.draft.groups.retain(|g| !same_group(g, group));
    }

    pub fn has_place(&self, place: &PlaceRef) -> bool {
        self.draft.places.iter().any(|p| same_place(p, place))
    }

    pub fn has_group(&self, group: &GroupRef) -> bool {
        self.draft.groups.iter().any(|g| same_group(g, group))
    }
}

/// What the left button of the panel's foot does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// Delete one of the person's views (asked first: it can't be undone).
    Delete,
    /// Put a built-in view back as it first was.
    PutBack,
}

impl Removal {
    pub fn label(self) -> &'static str {
        match self {
            Self::Delete => "Delete view",
            Self::PutBack => "Back to the original",
        }
    }
}

/// Accounts are keyed lower-cased; a place is one whatever the spelling.
fn same_place(a: &PlaceRef, b: &PlaceRef) -> bool {
    a.id == b.id && a.account.eq_ignore_ascii_case(&b.account)
}

fn same_group(a: &GroupRef, b: &GroupRef) -> bool {
    a.uri == b.uri && a.account.eq_ignore_ascii_case(&b.account)
}

#[cfg(test)]
#[path = "social_edit_tests.rs"]
mod tests;

/// What Save does (see [`SocialEdit::save_step`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveStep {
    /// The view has no name: say so, and keep the panel open.
    NeedsName,
    /// Nothing changed: close the panel, writing nothing.
    Close,
    /// Write the view.
    Write,
}
