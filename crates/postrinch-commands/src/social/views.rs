//! Social views: the person's own selections of places and groups, with
//! filters, and the built-in ones ([`super::views_built_in`]). A group in a
//! view means all its channels, those made later included. A view's
//! places are its tabs; its page merges them ([`super::pages`]).

use std::collections::HashSet;

use postrinch_store::{
    GroupRef, PlaceKind, PlaceRef, SocialCount, SocialPlace, SocialView, social_account_key,
};
use serde::{Deserialize, Serialize};

use super::filters::Filter;
use super::new_id;
use super::places::SocialPlaceInfo;
use super::views_built_in::BuiltInView;
use crate::{CommandError, CommandResult, Commands};

/// A view as the sidebar lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialViewEntry {
    /// The view as saved (a built-in view's own, until saved over).
    pub view: SocialView,
    /// What a built-in view shows; `None` for the person's own.
    pub built_in: Option<BuiltInView>,
    /// A built-in view as it first was, to put back; `None` for the
    /// person's own, which can be removed instead.
    pub original: Option<SocialView>,
}

/// How many unread messages a view's places have, as its filters show
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocialViewTally {
    pub id: String,
    pub unread: usize,
    /// Those that mention the person.
    pub mentions: usize,
}

impl SocialViewTally {
    /// What the sidebar shows beside the view, as it asks.
    pub fn shown(&self, count: SocialCount) -> Option<usize> {
        match count {
            SocialCount::Unread => Some(self.unread),
            SocialCount::Mentions => Some(self.mentions),
            SocialCount::None => None,
        }
    }
}

impl Commands {
    /// The views this caller sees, in the sidebar's order: Home, Mentions,
    /// Messages and Drafts, the person's own in their order, then All
    /// channels.
    pub fn social_views(&self) -> CommandResult<Vec<SocialViewEntry>> {
        let saved = self.store.social_views()?;
        let built_in = |kind: BuiltInView| {
            let original = kind.original();
            let view = saved
                .iter()
                .find(|v| v.id == original.id)
                .cloned()
                .unwrap_or_else(|| original.clone());
            SocialViewEntry {
                view,
                built_in: Some(kind),
                original: Some(original),
            }
        };
        let mut out: Vec<SocialViewEntry> = BuiltInView::FIRST.into_iter().map(built_in).collect();
        out.extend(
            saved
                .iter()
                .filter(|v| BuiltInView::of(&v.id).is_none() && self.sees_social_view(v))
                .map(|v| SocialViewEntry {
                    view: v.clone(),
                    built_in: None,
                    original: None,
                }),
        );
        out.push(built_in(BuiltInView::AllChannels));
        Ok(out)
    }

    /// The view `id`.
    pub fn social_view(&self, id: &str) -> CommandResult<SocialViewEntry> {
        self.social_views()?
            .into_iter()
            .find(|e| e.view.id == id)
            .ok_or_else(|| CommandError::Invalid(format!("There is no view {id:?}.")))
    }

    /// Save a view: a new one of the person's when its id is empty (put
    /// last), or else the view with its id. A built-in view's places are
    /// fixed. Returns the view as saved.
    pub fn save_social_view(&self, view: &SocialView) -> CommandResult<SocialView> {
        let mut view = view.clone();
        view.name = view.name.trim().to_owned();
        if view.name.is_empty() {
            return Err(CommandError::Invalid("A view needs a name.".into()));
        }
        Filter::of_view(&view.filters)?;
        view.places = self.checked_places(&view.places)?;
        view.groups = self.checked_groups(&view.groups)?;
        if !self.may_change_social_view(&view) {
            return Err(limited());
        }
        if view.id.is_empty() {
            view.id = new_id();
            let saved = self.store.social_views()?;
            view.order = saved.iter().map(|v| v.order + 1).max().unwrap_or(0);
        } else if let Some(kind) = BuiltInView::of(&view.id) {
            check_built_in(kind, &view)?;
        } else {
            let before = self.social_view(&view.id)?;
            if !self.may_change_social_view(&before.view) {
                return Err(limited());
            }
        }
        self.store.put_social_view(&view)?;
        Ok(view)
    }

    /// Remove one of the person's views, or put a built-in one back as it
    /// first was.
    pub fn remove_social_view(&self, id: &str) -> CommandResult<()> {
        let entry = self.social_view(id)?;
        if !self.may_change_social_view(&entry.view) {
            return Err(limited());
        }
        self.store.remove_social_view(id)?;
        Ok(())
    }

    /// Put the person's views in the order of `ids`; those not named keep
    /// their order, after them. An agent orders only the views it sees,
    /// among the places they had: the others stay where they are.
    pub fn order_social_views(&self, ids: &[String]) -> CommandResult<()> {
        let mut mine: Vec<SocialView> = self
            .store
            .social_views()?
            .into_iter()
            .filter(|v| BuiltInView::of(&v.id).is_none() && self.sees_social_view(v))
            .collect();
        mine.sort_by_key(|v| v.order);
        let slots: Vec<u64> = if self.sees_all() {
            (0..mine.len() as u64).collect()
        } else {
            mine.iter().map(|v| v.order).collect()
        };
        let mut ordered = Vec::with_capacity(mine.len());
        for id in ids {
            let view = mine
                .iter()
                .find(|v| &v.id == id && self.sees_social_view(v))
                .ok_or_else(|| {
                    CommandError::Invalid(format!("There is no view {id:?} of yours."))
                })?;
            if !self.may_change_social_view(view) {
                return Err(limited());
            }
            ordered.push(view.clone());
        }
        let named: HashSet<&String> = ids.iter().collect();
        ordered.extend(mine.iter().filter(|v| !named.contains(&v.id)).cloned());
        for (order, mut view) in slots.into_iter().zip(ordered) {
            if view.order != order {
                view.order = order;
                self.store.put_social_view(&view)?;
            }
        }
        Ok(())
    }

    /// A view's places: its tabs.
    pub fn social_view_places(&self, id: &str) -> CommandResult<Vec<SocialPlaceInfo>> {
        let entry = self.social_view(id)?;
        let mut out = Vec::new();
        for place in self.view_places(&entry)? {
            out.push(self.place_info(place)?);
        }
        Ok(out)
    }

    /// How many unread messages a view shows.
    pub fn social_view_tally(&self, id: &str) -> CommandResult<SocialViewTally> {
        let entry = self.social_view(id)?;
        let mut tally = SocialViewTally {
            id: entry.view.id.clone(),
            unread: 0,
            mentions: 0,
        };
        if entry.built_in.is_some_and(|k| !k.shows_messages()) {
            return Ok(tally);
        }
        let filter = Filter::of_view(&entry.view.filters)?;
        let only_mentions = entry.built_in == Some(BuiltInView::Mentions);
        for place in self.view_places(&entry)? {
            for message in self
                .store
                .social_unread_messages(&place.account, &place.id)?
            {
                if filter.keeps(&message) && (message.mentions_me || !only_mentions) {
                    tally.unread += 1;
                    tally.mentions += usize::from(message.mentions_me);
                }
            }
        }
        Ok(tally)
    }

    /// The places a view selects, as kept, of the accounts this caller
    /// sees.
    pub(crate) fn view_places(&self, entry: &SocialViewEntry) -> CommandResult<Vec<SocialPlace>> {
        let mut out = Vec::new();
        match entry.built_in {
            Some(BuiltInView::Drafts | BuiltInView::AllChannels) => {}
            Some(kind) => {
                for account in self.store.social_accounts()? {
                    if !self.sees_social(&account.actor) {
                        continue;
                    }
                    for place in self.places_by_name(&account.actor)? {
                        let keep = match kind {
                            BuiltInView::Home if place.kind == PlaceKind::Channel => {
                                let mut shown = place.clone();
                                self.place_outbox(&place.account, &place.id)?
                                    .lay_over_place(&mut shown);
                                shown.followed
                            }
                            BuiltInView::Home => false,
                            BuiltInView::Messages => place.kind == PlaceKind::Conversation,
                            _ => true,
                        };
                        if keep {
                            out.push(place);
                        }
                    }
                }
            }
            None => {
                let view = &entry.view;
                let mut seen = HashSet::new();
                for r in &view.places {
                    if self.sees_social(&r.account)
                        && let Some(place) = self.store.social_place(&r.account, &r.id)?
                        && seen.insert((social_account_key(&r.account), r.id.clone()))
                    {
                        out.push(place);
                    }
                }
                for g in &view.groups {
                    if !self.sees_social(&g.account) {
                        continue;
                    }
                    for place in self.store.social_places_of_group(&g.account, &g.uri)? {
                        if seen.insert((social_account_key(&g.account), place.id.clone())) {
                            out.push(place);
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    /// Whether this caller sees `view`: one whose places and groups are all
    /// of accounts it sees.
    fn sees_social_view(&self, view: &SocialView) -> bool {
        view.places.iter().all(|p| self.sees_social(&p.account))
            && view.groups.iter().all(|g| self.sees_social(&g.account))
    }

    /// Whether this caller may make or change `view`. The window may
    /// change any; an agent limited to some accounts only views of theirs
    /// alone, as a built-in view shows the person every account.
    fn may_change_social_view(&self, view: &SocialView) -> bool {
        self.sees_all()
            || (BuiltInView::of(&view.id).is_none()
                && !(view.places.is_empty() && view.groups.is_empty())
                && self.sees_social_view(view))
    }

    /// `places`, each one its account knows, once each.
    fn checked_places(&self, places: &[PlaceRef]) -> CommandResult<Vec<PlaceRef>> {
        let mut out: Vec<PlaceRef> = Vec::new();
        for r in places {
            let place = self.check_place(&r.account, &r.id)?;
            let r = PlaceRef {
                account: social_account_key(&place.account),
                id: place.id,
            };
            if !out.contains(&r) {
                out.push(r);
            }
        }
        Ok(out)
    }

    /// `groups`, each one its account knows, once each.
    fn checked_groups(&self, groups: &[GroupRef]) -> CommandResult<Vec<GroupRef>> {
        let mut out: Vec<GroupRef> = Vec::new();
        for g in groups {
            let account = self.check_social(&g.account)?;
            if self.store.social_group(&account.actor, &g.uri)?.is_none() {
                return Err(CommandError::Invalid(format!(
                    "{} knows no group {}.",
                    account.actor, g.uri
                )));
            }
            let g = GroupRef {
                account: social_account_key(&account.actor),
                uri: g.uri.clone(),
            };
            if !out.contains(&g) {
                out.push(g);
            }
        }
        Ok(out)
    }
}

/// An error unless `view` saves over the built-in `kind` as it may.
fn check_built_in(kind: BuiltInView, view: &SocialView) -> CommandResult<()> {
    let original = kind.original();
    if !view.places.is_empty() || !view.groups.is_empty() {
        return Err(CommandError::Invalid(format!(
            "{} is built in: what it shows is fixed. Make a view of your own to choose places.",
            original.name
        )));
    }
    if !kind.shows_messages() && view.filters != original.filters {
        return Err(CommandError::Invalid(format!(
            "{} lists {}, which filters don't narrow.",
            original.name,
            if kind == BuiltInView::Drafts {
                "drafts"
            } else {
                "places"
            }
        )));
    }
    Ok(())
}

/// An agent's view change that would reach accounts it does not see.
fn limited() -> CommandError {
    CommandError::Invalid(
        "You can change only views of your own accounts alone: this one shows the person places you don't see.".into(),
    )
}
