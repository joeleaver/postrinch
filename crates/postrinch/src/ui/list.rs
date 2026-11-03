//! The thread list: one row per thread in the chosen folder.

use postrinch_commands::{CATEGORIES, Folder, Sidebar, ThreadPage, ThreadRow};
use postrinch_model::{SystemLabel, ThreadSummary};
use rinch::prelude::*;

use crate::dates;
use crate::state::{PAGE, State};
use crate::sync::thousands;
use crate::ui::account_color;

#[component]
pub fn thread_list(state: State) -> NodeHandle {
    rsx! {
        section { class: "list",
            div { class: "list-head",
                h1 { {move || title(&state.folder.get(), state.sidebar.get().as_ref())} }
                span { class: "list-count", {move || unread_line(&state.folder.get(), state.sidebar.get().as_ref())} }
            }
            if is_inbox(&state.folder.get(), state.sidebar.get().as_ref()) {
                {tabs(__scope, state)}
            }
            div { class: "rows",
                for thread in state.page.get().map(|p| p.threads).unwrap_or_default() {
                    div { key: row_key(&thread),
                        {row(__scope, state, thread.clone())}
                    }
                }
                if state.page.get().is_some_and(|p| p.threads.is_empty()) {
                    div { class: "empty", "Nothing here." }
                }
                if state.page.get().is_some_and(|p| p.total > p.threads.len()) {
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

/// Changes whenever anything the row shows changes, so the row is drawn
/// again.
fn row_key(thread: &ThreadRow) -> String {
    let s = &thread.summary;
    format!(
        "{}|{}|{}|{}|{}|{:?}",
        s.account.key(),
        s.id.0,
        s.latest.as_millis(),
        s.unread_count,
        s.message_count,
        thread.chips
    )
}

#[component]
fn row(state: State, thread: ThreadRow) -> NodeHandle {
    let summary = thread.summary;
    let chips: Vec<NodeHandle> = thread
        .chips
        .iter()
        .map(|chip| {
            let style = match &chip.color {
                Some(c) => format!("border-color: {c}; color: {c}"),
                None => String::new(),
            };
            let name = chip.name.clone();
            rsx! { span { class: "label-chip", style: {style.clone()}, {name} } }
        })
        .collect();
    let key = (summary.account.clone(), summary.id.clone());
    let chosen = key.clone();
    let now = dates::now();
    let color = account_index(state.sidebar.get().as_ref(), &summary).map(account_color);
    let dot = color.map(|c| rsx! { span { class: "dot", style: {format!("background: {c}")} } });
    let count = (summary.message_count > 1).then(|| {
        let n = summary.message_count.to_string();
        rsx! { span { class: "thread-count", {n} } }
    });
    let attachment = summary
        .has_attachments
        .then(|| rsx! { span { class: "attachment", title: "Has attachments", "📎" } });
    let unread = summary.unread_count > 0;
    rsx! {
        div {
            class: {move || {
                let mut class = String::from("row");
                if unread { class.push_str(" unread"); }
                if state.selected.get().as_ref() == Some(&chosen) { class.push_str(" chosen"); }
                class
            }},
            onclick: move || state.selected.set(Some(key.clone())),
            div { class: "row-top",
                {dot}
                span { class: "who", {people(&summary)} }
                {count}
                {attachment}
                span { class: "when", {dates::short(summary.latest, &now)} }
            }
            div { class: "subject", {or_none(&summary.subject, "(no subject)")} }
            div { class: "row-bottom",
                {chips}
                span { class: "snippet", {summary.snippet.clone()} }
            }
        }
    }
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
fn is_inbox(folder: &Folder, sidebar: Option<&Sidebar>) -> bool {
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
