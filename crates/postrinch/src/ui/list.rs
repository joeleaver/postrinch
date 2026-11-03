//! The thread list: one row per thread in the chosen folder.

use postrinch_commands::{CATEGORIES, Folder, Sidebar, ThreadPage, ThreadRow};
use postrinch_model::{EmailAddress, ProviderThreadId, SystemLabel, ThreadSummary};
use rinch::prelude::*;

use crate::dates;
use crate::state::{PAGE, State};
use crate::sync::thousands;
use crate::ui::account_color;

#[component]
pub fn thread_list(state: State) -> NodeHandle {
    let searching = move || state.search.get().is_some();
    rsx! {
        section {
            class: "list",
            style: {move || format!("width: {}px", state.list_width.get())},
            if searching() {
                div { class: "list-head",
                    button {
                        class: "back",
                        title: "Close search",
                        onclick: move || state.close_search(),
                        "←"
                    }
                    div { class: "search-head",
                        h1 { "Search results" }
                        span { class: "list-count", {move || search_line(state)} }
                    }
                }
            } else {
                div { class: "list-head",
                    h1 { {move || title(&state.folder.get(), state.sidebar.get().as_ref())} }
                    span { class: "list-count", {move || unread_line(&state.folder.get(), state.sidebar.get().as_ref())} }
                }
            }
            if !searching() && is_inbox(&state.folder.get(), state.sidebar.get().as_ref()) {
                {tabs(__scope, state)}
            }
            div { class: "rows",
                // Keyed by thread alone: a row is built once and follows
                // its thread's changes in place.
                for thread in row_ids(state) {
                    div { key: format!("{}|{}", thread.0.key(), thread.1.0),
                        {row(__scope, state, thread.clone())}
                    }
                }
                if !searching() && state.page.get().is_some_and(|p| p.threads.is_empty()) {
                    div { class: "empty", "Nothing here." }
                }
                if searching() && state.results.get().is_some_and(|r| r.threads.is_empty()) {
                    div { class: "empty", "No mail matches." }
                }
                if !searching() && state.page.get().is_some_and(|p| p.total > p.threads.len()) {
                    button {
                        class: "more",
                        onclick: move || state.shown.update(|n| *n += PAGE),
                        {move || more_label(state.page.get().as_ref())}
                    }
                }
            }
        }
    }
}

/// Which thread a row shows.
pub(crate) type RowId = (EmailAddress, ProviderThreadId);

/// The rows to show: the search's, while there is one, or the folder's.
fn with_rows<R>(state: State, f: impl FnOnce(&[ThreadRow]) -> R) -> R {
    if state.search.get().is_some() {
        state
            .results
            .with(|r| f(r.as_ref().map_or(&[], |r| &r.threads)))
    } else {
        state
            .page
            .with(|p| f(p.as_ref().map_or(&[], |p| &p.threads)))
    }
}

/// The threads listed, less those a pending action takes out.
pub(crate) fn row_ids(state: State) -> Vec<RowId> {
    let ids: Vec<RowId> = with_rows(state, |rows| {
        rows.iter()
            .map(|r| (r.summary.account.clone(), r.summary.id.clone()))
            .collect()
    });
    ids.into_iter()
        .filter(|id| !state.pending_removal(id))
        .collect()
}

/// The row's thread as now loaded, with pending actions on it applied.
fn find_row(state: State, id: &RowId) -> Option<ThreadRow> {
    let found = with_rows(state, |rows| {
        rows.iter()
            .find(|r| r.summary.account == id.0 && r.summary.id == id.1)
            .cloned()
    });
    found.map(|row| state.patched(row))
}

/// "6 for “cover my shift”".
fn search_line(state: State) -> String {
    match state.results.get() {
        Some(r) => {
            let n = r.threads.len();
            let what = if n == 1 {
                "conversation"
            } else {
                "conversations"
            };
            let more = if r.more { "+" } else { "" };
            format!(
                "{}{more} {what} for \u{201c}{}\u{201d}",
                thousands(n),
                r.query
            )
        }
        None => String::new(),
    }
}

/// "Show more (100 of 1,799)".
fn more_label(page: Option<&ThreadPage>) -> String {
    page.map(|p| {
        format!(
            "Show more ({} of {})",
            thousands(p.threads.len()),
            thousands(p.total)
        )
    })
    .unwrap_or_default()
}

/// Gmail's inbox tabs: everything, or one category.
#[component]
fn tabs(state: State) -> NodeHandle {
    let all = std::iter::once(None).chain(CATEGORIES.into_iter().map(Some));
    let buttons: Vec<NodeHandle> = all
        .map(|category| {
            let name = match category {
                None => "All",
                Some(SystemLabel::CategoryPersonal) => "Primary",
                Some(SystemLabel::CategoryUpdates) => "Updates",
                Some(SystemLabel::CategoryPromotions) => "Promotions",
                Some(SystemLabel::CategorySocial) => "Social",
                Some(_) => "Forums",
            };
            rsx! {
                button {
                    class: {move || if state.category.get() == category { "tab chosen" } else { "tab" }},
                    onclick: move || state.open_category(category),
                    {name}
                }
            }
        })
        .collect();
    rsx! { div { class: "tabs", role: "tablist", {buttons} } }
}

#[component]
fn row(state: State, id: RowId) -> NodeHandle {
    let first = find_row(state, &id);
    let Some(first) = first else {
        return rsx! { div {} };
    };
    let thread = {
        let id = id.clone();
        let last = std::cell::RefCell::new(first);
        Memo::new(move || {
            if let Some(found) = find_row(state, &id) {
                *last.borrow_mut() = found;
            }
            last.borrow().clone()
        })
    };
    let summary = move || thread.get().summary;
    let chosen = id.clone();
    let color = move || account_index(state.sidebar.get().as_ref(), &summary()).map(account_color);
    rsx! {
        div {
            class: {move || {
                let mut class = String::from("row");
                if summary().unread_count > 0 { class.push_str(" unread"); }
                if state.selected.get().as_ref() == Some(&chosen) { class.push_str(" chosen"); }
                class
            }},
            onclick: move || state.selected.set(Some(id.clone())),
            div { class: "row-top",
                span {
                    class: "dot",
                    style: {move || match color() {
                        Some(c) => format!("background: {c}"),
                        None => "display: none".to_owned(),
                    }},
                }
                span { class: "who", {move || people(&summary())} }
                span {
                    class: "thread-count",
                    style: {move || shown(summary().message_count > 1)},
                    {move || summary().message_count.to_string()}
                }
                span {
                    class: "attachment",
                    title: "Has attachments",
                    style: {move || shown(summary().has_attachments)},
                    "📎"
                }
                span { class: "when", {move || dates::short(summary().latest, &dates::now())} }
            }
            div { class: "subject", {move || or_none(&summary().subject, "(no subject)")} }
            div { class: "row-bottom",
                for chip in thread.get().chips {
                    span {
                        key: format!("{chip:?}"),
                        class: "label-chip",
                        style: {match &chip.color {
                            Some(c) => format!("border-color: {c}; color: {c}"),
                            None => String::new(),
                        }},
                        {chip.name.clone()}
                    }
                }
                span { class: "snippet",
                    {move || match thread.get().excerpt {
                        Some(e) => e.before,
                        None => summary().snippet,
                    }}
                    mark {
                        style: {move || shown(thread.get().excerpt.is_some())},
                        {move || thread.get().excerpt.map(|e| e.hit).unwrap_or_default()}
                    }
                    {move || thread.get().excerpt.map(|e| e.after).unwrap_or_default()}
                }
            }
        }
    }
}

/// A style that hides an element unless `show`.
fn shown(show: bool) -> &'static str {
    if show { "" } else { "display: none" }
}

/// Who a thread is between, by first name where there is one, leaving out
/// the account's own address unless it is all there is.
fn people(summary: &ThreadSummary) -> String {
    let me = summary.account.key();
    let others: Vec<String> = summary
        .participants
        .iter()
        .filter(|m| m.address.key() != me)
        .map(|m| m.display_name())
        .collect();
    match others.len() {
        0 => "me".to_owned(),
        1..=3 => others.join(", "),
        n => format!("{}, {} and {} more", others[0], others[1], n - 2),
    }
}

fn or_none(text: &str, none: &str) -> String {
    if text.trim().is_empty() {
        none.to_owned()
    } else {
        text.to_owned()
    }
}

fn account_index(sidebar: Option<&Sidebar>, summary: &ThreadSummary) -> Option<usize> {
    let accounts = &sidebar?.accounts;
    if accounts.len() < 2 {
        return None;
    }
    accounts
        .iter()
        .position(|a| a.account.address == summary.account)
}

/// Whether a folder is an inbox, which gets Gmail's category tabs.
pub(crate) fn is_inbox(folder: &Folder, sidebar: Option<&Sidebar>) -> bool {
    match folder {
        Folder::Everywhere(system) => *system == SystemLabel::Inbox,
        Folder::Label { .. } => {
            sidebar.is_some_and(|s| s.accounts.iter().any(|a| &a.inbox.folder == folder))
        }
    }
}

fn title(folder: &Folder, sidebar: Option<&Sidebar>) -> String {
    match folder {
        Folder::Everywhere(SystemLabel::Inbox) => "All inboxes".to_owned(),
        _ => sidebar
            .and_then(|s| s.entry(folder))
            .map(|e| e.name.clone())
            .unwrap_or_default(),
    }
}

fn unread_line(folder: &Folder, sidebar: Option<&Sidebar>) -> String {
    match sidebar
        .and_then(|s| s.entry(folder))
        .map_or(0, |e| e.unread)
    {
        0 => String::new(),
        unread => format!("{} unread", thousands(unread)),
    }
}
