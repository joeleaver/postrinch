//! The sidebar: unified folders, each account's inbox, labels, and sync.

use postrinch_commands::FolderEntry;
use postrinch_model::EmailAddress;
use rinch::prelude::*;

use crate::state::State;
use crate::sync::thousands;
use crate::ui::account_color;
use crate::ui::labels::labels_section;

#[component]
pub fn sidebar(state: State) -> NodeHandle {
    rsx! {
        nav {
            class: "sidebar",
            style: {move || format!("width: {}px", state.sidebar_width.get())},
            {crate::ui::outgoing::outgoing_lines(__scope, state)}
            div { class: "folders",
                div { class: "folder-group",
                    // Keyed by what a row is, not its count: rows are built
                    // once and their counts follow the sidebar in place.
                    for slot in folder_slots(state) {
                        div { key: format!("{slot:?}"),
                            {folder_row(__scope, state, slot.clone())}
                        }
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
                // Keyed by account alone: a row is built once and its
                // line and bar follow the account's sync in place.
                for account in sync_accounts(state) {
                    div { key: account.key(),
                        {sync_row(__scope, state, account.clone())}
                    }
                }
            }
        }
    }
}

/// A row of the sidebar's folders, without what changes as mail arrives.
#[derive(Debug, Clone, PartialEq)]
struct FolderSlot {
    entry: FolderEntry,
    dot: Option<&'static str>,
    indented: bool,
}

/// All inboxes with each account's inbox under it, then Sent and Drafts,
/// then the labels the person pinned.
fn folder_slots(state: State) -> Vec<FolderSlot> {
    state.sidebar.with(|sidebar| {
        let Some(sidebar) = sidebar else {
            return Vec::new();
        };
        let slot = |entry: &FolderEntry, dot, indented| FolderSlot {
            entry: FolderEntry {
                unread: 0,
                ..entry.clone()
            },
            dot,
            indented,
        };
        let mut slots = Vec::new();
        let mut everywhere = sidebar.everywhere.iter();
        if let Some(all) = everywhere.next() {
            slots.push(slot(all, None, false));
            for (i, account) in sidebar.accounts.iter().enumerate() {
                slots.push(slot(&account.inbox, Some(account_color(i)), true));
            }
        }
        for entry in everywhere {
            slots.push(slot(entry, None, false));
        }
        for account in &sidebar.accounts {
            for entry in &account.labels.pinned {
                slots.push(slot(entry, None, false));
            }
        }
        slots
    })
}

#[component]
fn folder_row(state: State, slot: FolderSlot) -> NodeHandle {
    let FolderSlot {
        entry,
        dot,
        indented,
    } = slot;
    let folder = entry.folder.clone();
    let chosen = entry.folder.clone();
    let counted = entry.folder.clone();
    let unread = Memo::new(move || {
        state.sidebar.with(|s| {
            s.as_ref()
                .and_then(|s| s.entry(&counted))
                .map_or(0, |e| e.unread)
        })
    });
    let dot =
        dot.map(|color| rsx! { span { class: "dot", style: {format!("background: {color}")} } });
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
            span {
                class: "count",
                style: {move || if unread.get() > 0 { "" } else { "display: none" }},
                {move || thousands(unread.get())}
            }
        }
    }
}

fn sync_accounts(state: State) -> Vec<EmailAddress> {
    state
        .sync
        .with(|all| all.iter().map(|s| s.account.clone()).collect())
}

#[component]
fn sync_row(state: State, account: EmailAddress) -> NodeHandle {
    let address = account.to_string();
    let now = Memo::new(move || {
        state
            .sync
            .with(|all| all.iter().find(|s| s.account == account).cloned())
    });
    let progress = move || now.get().and_then(|s| s.progress);
    rsx! {
        div { class: "sync-account",
            div { class: "sync-address", {address} }
            div {
                class: {move || match now.get() {
                    Some(s) if s.trouble => "sync-line trouble",
                    _ => "sync-line",
                }},
                {move || now.get().map(|s| s.line).unwrap_or_default()}
            }
            div {
                class: "progress",
                style: {move || if progress().is_some() { "" } else { "display: none" }},
                div {
                    class: "progress-fill",
                    style: {move || {
                        let percent = progress()
                            .map_or(0, |(done, total)| (done * 100).checked_div(total).unwrap_or(100));
                        format!("width: {percent}%")
                    }},
                }
            }
        }
    }
}
