//! The last few lists shown, so going back to a view
//! shows its threads at once while the loader reads them again, and which
//! list the window wants now, so the loader drops an answer for one the
//! person has already left.

use std::sync::Mutex;

use postrinch_commands::{Grouping, ThreadPage};
use postrinch_model::SavedView;

/// How many lists are kept.
const KEPT: usize = 8;

/// What a list is of. Not how many of its threads are shown: showing more
/// of a list is still that list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListKey {
    /// The view as shown, its order and any unsaved changes included.
    pub view: SavedView,
    pub grouping: Grouping,
}

struct Recent {
    /// The list the window shows now.
    wanted: Option<ListKey>,
    /// Lists as last loaded, the latest last.
    pages: Vec<(ListKey, ThreadPage)>,
}

static RECENT: Mutex<Recent> = Mutex::new(Recent {
    wanted: None,
    pages: Vec::new(),
});

fn with<R>(f: impl FnOnce(&mut Recent) -> R) -> R {
    f(&mut RECENT.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The window now shows `key`. Returns its list as last loaded when it is
/// another list than before, to show until the loader's answer comes; on
/// the same list (sync changed the store, or more is shown) returns `None`.
pub fn want(key: ListKey) -> Option<Option<ThreadPage>> {
    with(|r| {
        if r.wanted.as_ref() == Some(&key) {
            return None;
        }
        let page = r
            .pages
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, p)| p.clone());
        r.wanted = Some(key);
        Some(page)
    })
}

/// Keep a loaded list. Returns whether it is the one the window shows now,
/// so a list left while it loaded is not shown.
pub fn loaded(key: ListKey, page: &ThreadPage) -> bool {
    with(|r| {
        r.pages.retain(|(k, _)| *k != key);
        if r.pages.len() == KEPT {
            r.pages.remove(0);
        }
        r.pages.push((key.clone(), page.clone()));
        r.wanted.as_ref() == Some(&key)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> ListKey {
        let made = postrinch_model::Timestamp::from_millis(name.len() as i64);
        ListKey {
            view: SavedView::new(name, &format!("label:{name}"), made).unwrap(),
            grouping: Grouping::default(),
        }
    }

    fn page(total: usize) -> ThreadPage {
        ThreadPage {
            total,
            offset: 0,
            threads: Vec::new(),
        }
    }

    // One test: they share the one cache.
    #[test]
    fn a_list_comes_back_at_once_and_a_left_one_is_not_shown() {
        assert_eq!(want(key("Inbox")), Some(None));
        assert!(loaded(key("Inbox"), &page(1)));
        // The same list again (sync, or more shown): nothing to swap in.
        assert_eq!(want(key("Inbox")), None);
        // Another list, then back: the first comes back as it was.
        assert_eq!(want(key("Sent")), Some(None));
        assert_eq!(want(key("Inbox")), Some(Some(page(1))));
        // Sent's answer arrives after the person went back to the inbox.
        assert!(!loaded(key("Sent"), &page(2)));
        // Only KEPT lists are kept, the oldest going first.
        for n in 0..KEPT {
            loaded(key(&format!("other {n}")), &page(n));
        }
        assert_eq!(want(key("Sent")), Some(None));
    }
}
