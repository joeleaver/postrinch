//! Each account's own views, listed under Inbox: with several accounts,
//! each account's inbox, and under it the account's Gmail categories
//! (Updates, Promotions, Social, Forums); with one, its categories under
//! Inbox itself. There is no Promotions of every account: a category is
//! an account's. An account's inbox follows Inbox, narrowed to the
//! account; a category is Gmail's, as it is. Each one's name, count,
//! order and place in the sidebar can be saved; its search and accounts
//! cannot.

use postrinch_model::{Account, EmailAddress, Provider, SavedView};

use crate::views::{Place, ViewEntry, built_in};

/// Gmail's categories but Primary (the inbox), by their built-in keys.
pub(crate) const TABS: [&str; 4] = ["updates", "promotions", "social", "forums"];

/// The view of every account that an account's own view follows, by its
/// key: `inbox:ann@example.org` follows `inbox`, `promotions:…` Gmail's
/// Promotions.
pub fn followed(key: &str) -> Option<&'static str> {
    let (parent, _) = key.split_once(':')?;
    std::iter::once("inbox").chain(TABS).find(|f| *f == parent)
}

/// An account's own view of `parent`, as it first was: `parent`, in that
/// account alone.
fn of_account(parent: &SavedView, account: &EmailAddress, name: String) -> SavedView {
    SavedView {
        key: format!("{}:{}", parent.key, account.key()),
        name,
        accounts: vec![account.clone()],
        ..parent.clone()
    }
}

/// Each account's views, in the sidebar's order, under `inbox`, from what
/// is `saved`.
pub(crate) fn account_views(
    inbox: &SavedView,
    accounts: &[Account],
    saved: impl Fn(&str) -> Option<SavedView>,
) -> Vec<ViewEntry> {
    let own = |parent: &SavedView, account: &EmailAddress, name: String, under: &str| {
        let original = of_account(parent, account, name);
        let mut view = saved(&original.key).unwrap_or_else(|| original.clone());
        view.query = parent.query.clone();
        view.accounts = original.accounts.clone();
        ViewEntry {
            view,
            place: Place::Top,
            original: Some(original),
            under: Some(under.to_owned()),
        }
    };
    let several = accounts.len() > 1;
    let mut entries = Vec::new();
    for a in accounts {
        let under = if several {
            let account_inbox = own(inbox, &a.address, a.address.to_string(), "inbox");
            let key = account_inbox.view.key.clone();
            entries.push(account_inbox);
            key
        } else {
            "inbox".to_owned()
        };
        // Only Gmail sorts mail into categories.
        if a.provider != Provider::Google {
            continue;
        }
        for (tab, _) in TABS.iter().filter_map(|t| built_in(t)) {
            entries.push(own(&tab, &a.address, tab.name.clone(), &under));
        }
    }
    entries
}
