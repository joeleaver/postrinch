//! The thread list: one row per thread in the chosen folder.

use postrinch_commands::{Folder, Sidebar, ThreadPage};
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
            div { class: "rows",
                if let Some(page) = state.page.get() {
                    {rows(__scope, state, page.clone())}
                }
            }
        }
    }
}

/// A page of rows, and "Show more" when the folder has more.
#[component]
fn rows(state: State, page: ThreadPage) -> NodeHandle {
    if page.threads.is_empty() {
        return rsx! { div { class: "empty", "Nothing here." } };
    }
    let more = (page.total > page.threads.len()).then(|| {
        let label = format!(
            "Show more ({} of {})",
            thousands(page.threads.len()),
            thousands(page.total)
        );
        rsx! {
            button {
                class: "more",
                onclick: move || state.shown.update(|n| *n += PAGE),
                {label}
            }
        }
    });
    rsx! {
        div {
            for summary in page.threads.clone() {
                div { key: format!("{}\u{1f}{}", summary.account.key(), summary.id.0),
                    {row(__scope, state, summary.clone())}
                }
            }
            {more}
        }
    }
}

#[component]
fn row(state: State, summary: ThreadSummary) -> NodeHandle {
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
            div { class: "snippet", {summary.snippet.clone()} }
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
