//! The sidebar: unified folders, each account's inbox, labels, and sync.

use postrinch_commands::{FolderEntry, Sidebar};
use rinch::prelude::*;

use crate::state::State;
use crate::sync::{AccountSync, thousands};
use crate::ui::account_color;
use crate::ui::labels::labels_section;

#[component]
pub fn sidebar(state: State) -> NodeHandle {
    rsx! {
        nav { class: "sidebar",
            div { class: "folders",
                // A keyed loop over the Option, not `if let`, so a changed
                // sidebar is drawn again (rinch#2105).
                for sidebar in state.sidebar.get() {
                    div { key: format!("{sidebar:?}"),
                        {unified(__scope, state, sidebar.clone())}
                    }
                }
            }
            div { class: "labels",
                {labels_section(__scope, state)}
            }
            div { class: "sync",
                div { class: "section-title", "Sync" }
                for why in state.sync_off.get() {
                    div { key: why.clone(), class: "sync-line trouble", {why.clone()} }
                }
                for account in state.sync.get() {
                    div { key: account.account.key(),
                        {sync_row(__scope, account.clone())}
                    }
                }
            }
        }
    }
}

/// All inboxes with each account's inbox under it, then Sent and Drafts,
/// then the labels the person pinned.
#[component]
fn unified(state: State, sidebar: Sidebar) -> NodeHandle {
    let mut rows = Vec::new();
    let pinned: Vec<FolderEntry> = sidebar
        .accounts
        .iter()
        .flat_map(|a| a.labels.pinned.iter().cloned())
        .collect();
    let mut everywhere = sidebar.everywhere.into_iter();
    if let Some(all) = everywhere.next() {
        rows.push(folder_row(__scope, state, all, None, false));
        for (i, account) in sidebar.accounts.into_iter().enumerate() {
            rows.push(folder_row(
                __scope,
                state,
                account.inbox,
                Some(account_color(i)),
                true,
            ));
        }
    }
    for entry in everywhere {
        rows.push(folder_row(__scope, state, entry, None, false));
    }
    for entry in pinned {
        rows.push(folder_row(__scope, state, entry, None, false));
    }
    rsx! { div { class: "folder-group", {rows} } }
}

#[component]
fn folder_row(
    state: State,
    entry: FolderEntry,
    dot: Option<&'static str>,
    indented: bool,
) -> NodeHandle {
    let folder = entry.folder.clone();
    let chosen = entry.folder.clone();
    let unread = (entry.unread > 0).then(|| thousands(entry.unread));
    let dot =
        dot.map(|color| rsx! { span { class: "dot", style: {format!("background: {color}")} } });
    let unread = unread.map(|n| rsx! { span { class: "count", {n} } });
    rsx! {
        div {
            class: {move || {
                let mut class = String::from("folder");
                if indented { class.push_str(" indented"); }
                if state.folder.get() == chosen { class.push_str(" chosen"); }
                class
            }},
            onclick: move || state.open_folder(folder.clone()),
            span { class: "folder-name", {dot} span { class: "name-text", {entry.name.clone()} } }
            {unread}
        }
    }
}

#[component]
fn sync_row(account: AccountSync) -> NodeHandle {
    let bar = account.progress.map(|(done, total)| {
        let percent = (done * 100).checked_div(total).unwrap_or(100);
        rsx! {
            div { class: "progress",
                div { class: "progress-fill", style: {format!("width: {percent}%")} }
            }
        }
    });
    let line_class = if account.trouble {
        "sync-line trouble"
    } else {
        "sync-line"
    };
    rsx! {
        div { class: "sync-account",
            div { class: "sync-address", {account.account.to_string()} }
            div { class: line_class, {account.line.clone()} }
            {bar}
        }
    }
}
