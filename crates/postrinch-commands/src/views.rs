//! Views: saved searches the sidebar lists in place of labels. Gmail's
//! inbox, its tabs, Sent, Drafts and All mail are views built in; the
//! person's own come between them. A built-in view can be saved over and
//! put back; the person's can be removed.

use std::collections::{HashMap, HashSet};

use postrinch_model::{EmailAddress, LabelKind, ListOrder, SavedView, Timestamp, ViewCount};
use postrinch_store::ThreadEntry;
use serde::{Deserialize, Serialize};

use crate::query::{LabelName, Query, label_matches};
use crate::query_or::Either;
use crate::{CommandError, CommandResult, Commands, ThreadPage};

/// The built-in view of mail in the inbox and in no other view.
pub const EVERYTHING_ELSE: &str = "everything-else";

/// Searches already run, by query and accounts.
type Found = HashMap<(String, Vec<EmailAddress>), Vec<ThreadEntry>>;

/// Where in the sidebar a view is listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    /// The inbox and its tabs, first.
    Top,
    /// The person's own views.
    Yours,
    /// Everything else, All mail, Sent and Drafts, last.
    Bottom,
}

/// A view as the sidebar lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewEntry {
    /// The view as saved (a built-in view's own, until saved over).
    pub view: SavedView,
    pub place: Place,
    /// A built-in view as it first was, to put back; `None` for the
    /// person's own, which can be removed instead.
    pub original: Option<SavedView>,
}

/// The built-in views: key, name, search, count, place.
const BUILT_IN: [(&str, &str, &str, ViewCount, Place); 10] = [
    ("inbox", "Inbox", "in:inbox", ViewCount::Unread, Place::Top),
    (
        "primary",
        "Primary",
        "in:inbox category:primary",
        ViewCount::Unread,
        Place::Top,
    ),
    (
        "updates",
        "Updates",
        "in:inbox category:updates",
        ViewCount::Unread,
        Place::Top,
    ),
    (
        "promotions",
        "Promotions",
        "in:inbox category:promotions",
        ViewCount::None,
        Place::Top,
    ),
    (
        "social",
        "Social",
        "in:inbox category:social",
        ViewCount::None,
        Place::Top,
    ),
    (
        "forums",
        "Forums",
        "in:inbox category:forums",
        ViewCount::None,
        Place::Top,
    ),
    (
        EVERYTHING_ELSE,
        "Everything else",
        "in:inbox",
        ViewCount::Unread,
        Place::Bottom,
    ),
    ("all-mail", "All mail", "", ViewCount::None, Place::Bottom),
    ("sent", "Sent", "in:sent", ViewCount::None, Place::Bottom),
    (
        "drafts",
        "Drafts",
        "in:drafts",
        ViewCount::All,
        Place::Bottom,
    ),
];

/// A built-in view as it first was.
fn built_in(key: &str) -> Option<(SavedView, Place)> {
    BUILT_IN
        .iter()
        .find(|(k, ..)| *k == key)
        .map(|&(key, name, query, count, place)| {
            let view = SavedView {
                key: key.to_owned(),
                name: name.to_owned(),
                query: query.to_owned(),
                accounts: Vec::new(),
                count,
                hide_when_empty: false,
                hidden: false,
                order: ListOrder::default(),
            };
            (view, place)
        })
}

impl Commands {
    /// Every view, in the sidebar's order: the inbox and its tabs, the
    /// person's own in the order made, then Everything else, All mail,
    /// Sent and Drafts.
    pub fn views(&self) -> CommandResult<Vec<ViewEntry>> {
        let stored = self.store.views()?;
        let saved = |key: &str| stored.iter().find(|v| v.key == key).cloned();
        let built = |place: Place| {
            BUILT_IN
                .iter()
                .filter(move |b| b.4 == place)
                .filter_map(|b| built_in(b.0))
                .map(|(original, place)| ViewEntry {
                    view: saved(&original.key).unwrap_or_else(|| original.clone()),
                    place,
                    original: Some(original),
                })
        };
        let mut entries: Vec<ViewEntry> = built(Place::Top).collect();
        entries.extend(
            stored
                .iter()
                .filter(|v| built_in(&v.key).is_none())
                .map(|v| ViewEntry {
                    view: v.clone(),
                    place: Place::Yours,
                    original: None,
                }),
        );
        entries.extend(built(Place::Bottom));
        Ok(entries)
    }

    /// Save a view, new or changed. Its search must read and its accounts
    /// must be ones Postrinch has.
    pub fn save_view(&self, view: &SavedView) -> CommandResult<()> {
        if view.name.trim().is_empty() {
            return Err(CommandError::Invalid("A view needs a name.".into()));
        }
        Either::parse(&view.query, now())?;
        let known: Vec<EmailAddress> = self
            .visible_accounts()?
            .into_iter()
            .map(|a| a.address)
            .collect();
        if let Some(stray) = view.accounts.iter().find(|a| !known.contains(a)) {
            return Err(CommandError::UnknownAccount(stray.clone()));
        }
        Ok(self.store.put_view(view)?)
    }

    /// Remove one of the person's views, or put a built-in one back as it
    /// first was.
    pub fn remove_view(&self, key: &str) -> CommandResult<()> {
        Ok(self.store.remove_view(key)?)
    }

    /// A new view of the person's, named and searching as given.
    pub fn new_view(&self, name: &str, query: &str) -> CommandResult<SavedView> {
        SavedView::new(name, query, now()).map_err(|e| CommandError::Invalid(e.to_string()))
    }

    /// A page of what a view shows (or a view changed but not saved), in
    /// `order`.
    pub fn list_view(
        &self,
        view: &SavedView,
        order: ListOrder,
        offset: usize,
        limit: usize,
    ) -> CommandResult<ThreadPage> {
        let entries = self.view_threads(view, &mut Found::new())?;
        self.thread_page(entries, order, offset, limit, None)
    }

    /// What the sidebar counts beside each view, by key: its unread
    /// conversations or all of them, as the view asks; views that count
    /// nothing and show while empty are left out. A search several views
    /// share (Everything else reads them all) is run once.
    pub fn view_counts(&self, views: &[SavedView]) -> CommandResult<Vec<(String, usize)>> {
        let mut found = Found::new();
        let mut counts = Vec::new();
        for view in views {
            if view.count == ViewCount::None && !view.hide_when_empty {
                continue;
            }
            let threads = self.view_threads(view, &mut found)?;
            let count = match view.count {
                ViewCount::Unread => threads.iter().filter(|t| t.unread).count(),
                ViewCount::All | ViewCount::None => threads.len(),
            };
            counts.push((view.key.clone(), count));
        }
        Ok(counts)
    }

    /// Every conversation a view shows, each once, latest activity first.
    fn view_threads(&self, view: &SavedView, found: &mut Found) -> CommandResult<Vec<ThreadEntry>> {
        let mut threads = self.query_threads(&view.query, &view.accounts, found)?;
        if view.key == EVERYTHING_ELSE {
            let mut elsewhere: HashSet<(String, String)> = HashSet::new();
            for other in self.views()? {
                let skip = matches!(other.view.key.as_str(), "inbox" | EVERYTHING_ELSE)
                    || other.place == Place::Bottom
                    || other.view.hidden;
                if skip {
                    continue;
                }
                for t in self.query_threads(&other.view.query, &other.view.accounts, found)? {
                    elsewhere.insert((t.account.key(), t.thread.0));
                }
            }
            threads.retain(|t| !elsewhere.contains(&(t.account.key(), t.thread.0.clone())));
        }
        Ok(threads)
    }

    /// The conversations a search finds in these accounts (every account
    /// for none), from `found` when it ran already.
    fn query_threads(
        &self,
        query: &str,
        accounts: &[EmailAddress],
        found: &mut Found,
    ) -> CommandResult<Vec<ThreadEntry>> {
        let key = (query.to_owned(), accounts.to_vec());
        if let Some(threads) = found.get(&key) {
            return Ok(threads.clone());
        }
        let threads = self.run_query(query, accounts)?;
        found.insert(key, threads.clone());
        Ok(threads)
    }

    /// [`Commands::query_threads`], run.
    fn run_query(&self, query: &str, accounts: &[EmailAddress]) -> CommandResult<Vec<ThreadEntry>> {
        let either = Either::parse(query, now())?;
        if let Some(threads) = self.labelled_threads(&either, accounts)? {
            return Ok(threads);
        }
        let (found, _) = self.found_in(&either, true, accounts)?;
        Ok(self.store.found_threads(&found)?)
    }

    /// The conversations a search finds when it only names a label or two
    /// (`in:inbox category:primary`, the tabs and most views), read through
    /// the labels' links alone, which is many times quicker than reading
    /// every message the labels have. `None` for any other search.
    fn labelled_threads(
        &self,
        either: &Either,
        only: &[EmailAddress],
    ) -> CommandResult<Option<Vec<ThreadEntry>>> {
        let labels_only = |q: &Query| {
            (1..=2).contains(&q.labels.len())
                && Query {
                    labels: Vec::new(),
                    accounts: Vec::new(),
                    gmail: String::new(),
                    ..q.clone()
                } == Query::default()
        };
        if !either.queries.iter().all(labels_only) {
            return Ok(None);
        }
        let mut threads: HashMap<(String, String), ThreadEntry> = HashMap::new();
        for q in &either.queries {
            for account in self.search_accounts(q)? {
                if !only.is_empty() && !only.contains(&account) {
                    continue;
                }
                let labels = self.store.labels(&account)?;
                let id = |name: &LabelName| {
                    labels
                        .iter()
                        .find(|l| match (name, &l.kind) {
                            (LabelName::System(s), LabelKind::System(k)) => s == k,
                            (LabelName::Named(n), LabelKind::User) => label_matches(&l.name, n),
                            _ => false,
                        })
                        .map(|l| l.provider_id.clone())
                };
                let ids: Option<Vec<String>> = q.labels.iter().map(id).collect();
                let Some(ids) = ids else { continue };
                let within = ids.get(1).map(String::as_str);
                for t in self
                    .store
                    .label_threads_sorting(&account, &ids[0], within, true)?
                {
                    let key = (t.account.key(), t.thread.0.clone());
                    let unread = t.unread;
                    threads
                        .entry(key)
                        .and_modify(|had| had.unread |= unread)
                        .or_insert(t);
                }
            }
        }
        let mut threads: Vec<ThreadEntry> = threads.into_values().collect();
        threads.sort_by(|a, b| {
            b.latest
                .cmp(&a.latest)
                .then_with(|| a.thread.cmp(&b.thread))
        });
        Ok(Some(threads))
    }
}

fn now() -> Timestamp {
    Timestamp::from_millis(jiff::Timestamp::now().as_millisecond())
}

#[cfg(test)]
#[path = "views_tests.rs"]
mod tests;
