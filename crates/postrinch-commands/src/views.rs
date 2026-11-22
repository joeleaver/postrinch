//! Views: saved searches the sidebar lists in place of labels. Gmail's
//! inbox (with each account's, and each account's categories, under it),
//! Sent, Drafts and All mail are views built in; the
//! person's own come between them. What mail a built-in view shows is
//! fixed; how it shows it can be saved over and put back. The person's
//! own can be removed.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use postrinch_model::{EmailAddress, LabelKind, ListOrder, SavedView, Timestamp, ViewCount};
use postrinch_store::ThreadEntry;
use serde::{Deserialize, Serialize};

use crate::labels::Chip;
use crate::messages::message_row;
use crate::query::{LabelName, Query, label_matches};
use crate::query_or::Either;
use crate::views_accounts::{TABS, account_views, followed};
use crate::{CommandError, CommandResult, Commands, ThreadPage};

/// The built-in view of mail in the inbox and in no other view.
pub const EVERYTHING_ELSE: &str = "everything-else";

/// Built-in views of mail outside the inbox, which Everything else does
/// not leave out.
const OUTSIDE_INBOX: [&str; 3] = ["all-mail", "sent", "drafts"];

/// Built-in views there are no more, whose saved settings are passed
/// over: Primary is the inbox now.
const RETIRED: [&str; 1] = ["primary"];

/// Searches already run, by query and accounts.
type Found = HashMap<(String, Vec<EmailAddress>), Vec<ThreadEntry>>;

/// Where in the sidebar a view is listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    /// The built-in views, first: Inbox with each account's tree under
    /// it, Everything else, All mail, Sent and Drafts.
    BuiltIn,
    /// The person's own views.
    Yours,
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
    /// The view it is listed under, by key: each account's inbox under
    /// Inbox, and its categories under its inbox (views_accounts.rs).
    #[serde(default)]
    pub under: Option<String>,
}

/// How many conversations a view has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewTally {
    pub key: String,
    pub all: usize,
    /// Those with unread mail.
    pub unread: usize,
}

impl ViewTally {
    /// What the sidebar shows beside the view, as it asks.
    pub fn shown(&self, count: ViewCount) -> Option<usize> {
        match count {
            ViewCount::Unread => Some(self.unread),
            ViewCount::All => Some(self.all),
            ViewCount::None => None,
        }
    }
}

/// The built-in views, in the sidebar's order: key, name, search, count.
const BUILT_IN: [(&str, &str, &str, ViewCount); 9] = [
    // Gmail's inbox as Gmail shows it with its tabs: Primary. The other
    // tabs are each account's, listed under it (views_accounts.rs).
    (
        "inbox",
        "Inbox",
        "in:inbox category:primary",
        ViewCount::Unread,
    ),
    (
        "updates",
        "Updates",
        "in:inbox category:updates",
        ViewCount::Unread,
    ),
    (
        "promotions",
        "Promotions",
        "in:inbox category:promotions",
        ViewCount::None,
    ),
    (
        "social",
        "Social",
        "in:inbox category:social",
        ViewCount::None,
    ),
    (
        "forums",
        "Forums",
        "in:inbox category:forums",
        ViewCount::None,
    ),
    (
        EVERYTHING_ELSE,
        "Everything else",
        "in:inbox",
        ViewCount::Unread,
    ),
    ("all-mail", "All mail", "", ViewCount::None),
    ("sent", "Sent", "in:sent", ViewCount::None),
    ("drafts", "Drafts", "in:drafts", ViewCount::All),
];

/// A built-in view as it first was.
pub(crate) fn built_in(key: &str) -> Option<SavedView> {
    BUILT_IN
        .iter()
        .find(|(k, ..)| *k == key)
        .map(|&(key, name, query, count)| SavedView {
            key: key.to_owned(),
            name: name.to_owned(),
            query: query.to_owned(),
            accounts: Vec::new(),
            count,
            hide_when_empty: false,
            hidden: false,
            order: ListOrder::default(),
            rows: None,
        })
}

/// Why a caller limited to some accounts may not change a view.
fn limited() -> CommandError {
    CommandError::Invalid(
        "Only views of this agent's own accounts can be made or changed by it: a view of every account, or of others, shows the person mail it does not see. Give the view `accounts`.".into(),
    )
}

impl Commands {
    /// Every view, in the sidebar's order: the built-in ones (Inbox with
    /// each account's tree under it, Everything else, All mail, Sent and
    /// Drafts), then the person's own in the order made.
    pub fn views(&self) -> CommandResult<Vec<ViewEntry>> {
        let stored = self.store.views()?;
        let saved = |key: &str| stored.iter().find(|v| v.key == key).cloned();
        let built = || {
            BUILT_IN
                .iter()
                .filter_map(|b| built_in(b.0))
                .map(|original| {
                    // What mail a built-in view shows is fixed; how it
                    // shows it is the person's.
                    let mut view = saved(&original.key).unwrap_or_else(|| original.clone());
                    view.query = original.query.clone();
                    view.accounts = original.accounts.clone();
                    ViewEntry {
                        view,
                        place: Place::BuiltIn,
                        original: Some(original),
                        under: None,
                    }
                })
        };
        let accounts = self.visible_accounts()?;
        let mut entries = Vec::new();
        for entry in built().filter(|e| !TABS.contains(&e.view.key.as_str())) {
            let inbox = (entry.view.key == "inbox").then(|| entry.view.clone());
            entries.push(entry);
            if let Some(inbox) = inbox {
                entries.extend(account_views(&inbox, &accounts, saved));
            }
        }
        entries.extend(
            stored
                .iter()
                .filter(|v| {
                    built_in(&v.key).is_none()
                        && followed(&v.key).is_none()
                        && !RETIRED.contains(&v.key.as_str())
                })
                .map(|v| ViewEntry {
                    view: v.clone(),
                    place: Place::Yours,
                    original: None,
                    under: None,
                }),
        );
        entries.retain(|e| self.sees_view(&e.view));
        Ok(entries)
    }

    /// Save a view, new or changed. Its search must read and its accounts
    /// must be ones Postrinch has; a built-in view's are its own.
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
        let before = self.store.views()?.into_iter().find(|v| v.key == view.key);
        let before = before.or_else(|| built_in(&view.key));
        if !self.may_change_view(view) || before.is_some_and(|v| !self.may_change_view(&v)) {
            return Err(limited());
        }
        let listed = self.views()?.into_iter().find(|e| e.view.key == view.key);
        if let Some(original) = listed.and_then(|e| e.original)
            && (original.query != view.query.trim() || original.accounts != view.accounts)
        {
            return Err(CommandError::Invalid(format!(
                "{} is built in: what mail it shows is fixed, and only how it shows it can change. Make a view of your own to show other mail.",
                original.name
            )));
        }
        Ok(self.store.put_view(view)?)
    }

    /// Remove one of the person's views, or put a built-in one back as it
    /// first was.
    pub fn remove_view(&self, key: &str) -> CommandResult<()> {
        let entry = self.views()?.into_iter().find(|e| e.view.key == key);
        let Some(entry) = entry else {
            return Err(CommandError::Invalid(format!("There is no view {key:?}.")));
        };
        if !self.may_change_view(&entry.view) {
            return Err(limited());
        }
        Ok(self.store.remove_view(key)?)
    }

    /// Why `query` does not read as a search, if it does not.
    pub fn check_query(query: &str) -> CommandResult<()> {
        Either::parse(query, now()).map(|_| ())
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

    /// [`Commands::list_view`] one message at a time: the messages the
    /// view's search finds, each a row of its own.
    pub fn list_view_messages(
        &self,
        view: &SavedView,
        order: ListOrder,
        offset: usize,
        limit: usize,
    ) -> CommandResult<ThreadPage> {
        let either = Either::parse(&view.query, now())?;
        let (mut found, _) = self.found_in(&either, true, &view.accounts)?;
        if view.key == EVERYTHING_ELSE {
            let keep: HashSet<(String, String)> = self
                .view_threads(view, &mut Found::new())?
                .into_iter()
                .map(|t| (t.account.key(), t.thread.0))
                .collect();
            let mut kept = Vec::new();
            for f in found {
                if let Some(thread) = self.store.found_thread(&f)?
                    && keep.contains(&(f.account.key(), thread.0))
                {
                    kept.push(f);
                }
            }
            found = kept;
        }
        found.sort_by(|a, b| {
            let tie = a
                .account
                .key()
                .cmp(&b.account.key())
                .then_with(|| a.id.cmp(&b.id));
            order.compare((a.unread, a.received), (b.unread, b.received), tie)
        });
        let total = found.len();
        let mut rows = Vec::new();
        let mut chips: HashMap<String, HashMap<String, Chip>> = HashMap::new();
        for f in found.iter().skip(offset).take(limit) {
            let Some(message) = self.store.message(&f.account, &f.id)? else {
                continue;
            };
            let known = match chips.entry(f.account.key()) {
                Entry::Occupied(known) => known.into_mut(),
                Entry::Vacant(slot) => slot.insert(self.account_chips(&f.account)?),
            };
            rows.extend(message_row(&message, known, None, None));
        }
        Ok(ThreadPage {
            total,
            offset,
            threads: rows,
        })
    }

    /// What the sidebar needs to know of each view: how many
    /// conversations are in it and how many have unread mail. Views that
    /// count nothing and show while empty are left out. A search several
    /// views share (Everything else reads them all) is run once.
    pub fn view_counts(&self, views: &[SavedView]) -> CommandResult<Vec<ViewTally>> {
        let mut found = Found::new();
        let mut counts = Vec::new();
        for view in views {
            if view.count == ViewCount::None && !view.hide_when_empty {
                continue;
            }
            let threads = self.view_threads(view, &mut found)?;
            counts.push(ViewTally {
                key: view.key.clone(),
                all: threads.len(),
                unread: threads.iter().filter(|t| t.unread).count(),
            });
        }
        Ok(counts)
    }

    /// Every conversation a view shows, each once, latest activity first.
    fn view_threads(&self, view: &SavedView, found: &mut Found) -> CommandResult<Vec<ThreadEntry>> {
        let mut threads = self.query_threads(&view.query, &view.accounts, found)?;
        if view.key == EVERYTHING_ELSE {
            let mut elsewhere: HashSet<(String, String)> = HashSet::new();
            for other in self.views()? {
                // Inbox counts (it is Primary unless changed), and each
                // account's categories; an account's inbox is part of
                // Inbox.
                let skip = other.view.key == EVERYTHING_ELSE
                    || followed(&other.view.key) == Some("inbox")
                    || OUTSIDE_INBOX.contains(&other.view.key.as_str())
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

pub(crate) fn now() -> Timestamp {
    Timestamp::from_millis(jiff::Timestamp::now().as_millisecond())
}

#[cfg(test)]
#[path = "views_tests.rs"]
mod tests;
