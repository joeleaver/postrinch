//! Social mode's state (docs/ofscp.md "Organizing: views, not providers",
//! the Social mockup): the views in the sidebar with their counts and
//! marks, the open view and its places (its tabs), the tab chosen, and the
//! view panel's draft. The loader fills it while Social is shown
//! (social_load.rs); the person's changes go through the `social/`
//! commands (social_actions.rs).

use std::collections::HashMap;

use postrinch_commands::{
    HOME, SocialGroupInfo, SocialPlaceInfo, SocialViewEntry, SocialViewTally,
};
use postrinch_store::PlaceRef;
use rinch::prelude::*;
use rinch::{Effect, untracked};

use crate::loader::Request;
use crate::social_edit::SocialEdit;
use crate::social_load::Ask;
use crate::social_shown::{GroupMark, chosen_tab};
use crate::state::{State, View};

/// Social mode's signals. `Copy`, like the window's state.
#[derive(Clone, Copy)]
pub struct SocialUi {
    /// Every view, in the sidebar's order.
    pub views: Signal<Vec<SocialViewEntry>>,
    /// Each view's unread count, by view id.
    pub tallies: Signal<HashMap<String, SocialViewTally>>,
    /// The groups each view's places are in, by view id, for its mark.
    pub groups: Signal<HashMap<String, Vec<GroupMark>>>,
    /// The open view's id.
    pub open: Signal<String>,
    /// The open view's places, its tabs, with the id of the view they are
    /// of (the view open when they were loaded).
    pub places: Signal<(String, Vec<SocialPlaceInfo>)>,
    /// The tab chosen: a place, or `None` for All.
    pub tab: Signal<Option<PlaceRef>>,
    /// The view being changed or made in the panel.
    pub edit: Signal<Option<SocialEdit>>,
    /// Every group and place known, for the panel's chips and to add.
    pub choices: Signal<Choices>,
    /// Bumped by each change made here, so what is shown loads again.
    pub changed: Signal<u64>,
}

/// What the panel offers to put in a view.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Choices {
    pub groups: Vec<SocialGroupInfo>,
    pub places: Vec<SocialPlaceInfo>,
}

impl SocialUi {
    pub fn new() -> Self {
        Self {
            views: Signal::new(Vec::new()),
            tallies: Signal::new(HashMap::new()),
            groups: Signal::new(HashMap::new()),
            open: Signal::new(HOME.to_owned()),
            places: Signal::new((String::new(), Vec::new())),
            tab: Signal::new(None),
            edit: Signal::new(None),
            choices: Signal::new(Choices::default()),
            changed: Signal::new(0),
        }
    }

    /// The open view, once the views are loaded.
    pub fn open_entry(self) -> Option<SocialViewEntry> {
        let open = self.open.get();
        self.views
            .with(|v| v.iter().find(|e| e.view.id == open).cloned())
    }

    /// The open view's places, once they are loaded for it: none while
    /// another view's are still shown.
    pub fn open_places(self) -> Vec<SocialPlaceInfo> {
        let open = self.open.get();
        self.places.with(|(of, places)| {
            if *of == open {
                places.clone()
            } else {
                Vec::new()
            }
        })
    }

    /// The open view's chosen tab, while it is still one of its tabs
    /// (`None` is "All"): a saved change, a sync or reading it as a list
    /// can take the place or the tabs away.
    pub fn open_tab(self) -> Option<PlaceRef> {
        let entry = self.open_entry()?;
        let places = self.open_places();
        self.tab.with(|t| chosen_tab(&entry, &places, t.as_ref()))
    }

    /// The place the header is about: the tab chosen, or a view's only
    /// place.
    pub fn shown_place(self) -> Option<SocialPlaceInfo> {
        let places = self.open_places();
        match self.open_tab() {
            Some(tab) => places.into_iter().find(|p| is_place(p, &tab)),
            None if places.len() == 1 => places.into_iter().next(),
            None => None,
        }
    }
}

/// Whether `info` is the place `r` names.
pub fn is_place(info: &SocialPlaceInfo, r: &PlaceRef) -> bool {
    info.place.id == r.id && info.place.account.eq_ignore_ascii_case(&r.account)
}

/// A place as a view names it.
pub fn place_ref(info: &SocialPlaceInfo) -> PlaceRef {
    PlaceRef {
        account: info.place.account.clone(),
        id: info.place.id.clone(),
    }
}

impl State {
    /// Keep Social loaded while it is shown: again whenever sync changes
    /// the store, a change is made here, another view is opened, or the
    /// panel opens (for what it offers).
    pub fn load_social(self, ask: impl Fn(Request) + 'static) {
        let shown = Memo::new(move || self.view.get() == View::Social);
        let panel = Memo::new(move || self.social.edit.with(Option::is_some));
        Effect::new(move || {
            self.revision.get();
            self.social.changed.get();
            if !shown.get() {
                return;
            }
            let open = self.social.open.get();
            let panel = panel.get();
            untracked(|| ask(Request::Social(Ask { open, panel })));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postrinch_store::{PlaceCursors, PlaceKind, SocialPlace};

    fn info(account: &str, id: &str) -> SocialPlaceInfo {
        SocialPlaceInfo {
            place: SocialPlace {
                account: account.into(),
                id: id.into(),
                kind: PlaceKind::Channel,
                group: None,
                followed: false,
                last_activity: None,
                object: serde_json::json!({}),
                cursors: PlaceCursors::default(),
                read_marker: None,
                notices: None,
            },
            name: id.into(),
            group_name: None,
            unread: 0,
            mentions: 0,
            may_post: Vec::new(),
            may_react: true,
            pending: None,
        }
    }

    #[test]
    fn a_place_is_named_by_its_account_in_any_spelling_and_its_id() {
        let general = info("ada@groups.example", "https://groups.example/c/1");
        let r = PlaceRef {
            account: "Ada@Groups.example".into(),
            id: "https://groups.example/c/1".into(),
        };
        assert!(is_place(&general, &r));
        assert!(!is_place(
            &info("bob@groups.example", "https://groups.example/c/1"),
            &r
        ));
        assert_eq!(place_ref(&general).id, r.id);
    }
}
