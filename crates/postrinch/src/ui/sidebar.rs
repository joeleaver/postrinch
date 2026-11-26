//! The sidebar, after the views mockup: the inbox (with each account's
//! under it, when there are several) and its tabs, a thin
//! line with + to make a view, the person's own views, another line, then
//! Everything else, All mail, Sent and Drafts; and sync below.

use postrinch_commands::Place;
use postrinch_model::{EmailAddress, SavedView};
use rinch::prelude::*;

use crate::state::State;
use crate::sync::thousands;
use crate::ui::account_color;
use crate::ui::icons::{Icon, icon};
use crate::view_edit::Editing;

#[component]
pub fn sidebar(state: State) -> NodeHandle {
    rsx! {
        nav {
            class: "sidebar",
            style: {move || format!("width: {}px", state.sidebar_width.get())},
            {crate::ui::outgoing::outgoing_lines(__scope, state)}
            div { class: "folders",
                div { class: "folder-group",
                    if state.search.get().is_some() {
                        {unsaved_row(__scope, state, Editing::Search)}
                        div { class: "view-line", span { class: "view-rule" } }
                    }
                    {views_in(__scope, state, Place::BuiltIn)}
                    div { class: "view-line",
                        span { class: "view-rule" }
                        button {
                            class: "view-add",
                            title: "New view",
                            aria-label: "New view",
                            onclick: move || state.new_view(),
                            "+"
                        }
                    }
                    if state.view_edit.get().is_some_and(|e| e.kind == Editing::New) {
                        {unsaved_row(__scope, state, Editing::New)}
                    }
                    {views_in(__scope, state, Place::Yours)}
                    div {
                        class: "show-all",
                        style: {move || if any_hidden(state) { "" } else { "display: none" }},
                        onclick: move || state.show_hidden.update(|on| *on = !*on),
                        {move || if state.show_hidden.get() { "Leave out hidden views" } else { "Show hidden views" }}
                    }
                }
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

fn any_hidden(state: State) -> bool {
    state.views.with(|v| v.iter().any(|e| e.view.hidden))
}

/// The views listed in one part of the sidebar, as a tree: each account
/// under Inbox, its categories under it, a row's rows shown while it is
/// open. Keyed by what a row shows but its count: a row is built once and
/// its count follows in place.
#[component]
fn views_in(state: State, place: Place) -> NodeHandle {
    let listed = move || {
        let counts = state.view_counts.get();
        let show_hidden = state.show_hidden.get();
        state.view_tree.get();
        state.views.with(|views| {
            let under = |key: &str| {
                views
                    .iter()
                    .find(|e| e.view.key == key)
                    .and_then(|e| e.under.clone())
            };
            views
                .iter()
                .filter(|e| e.place == place && crate::views::listed(&e.view, &counts, show_hidden))
                .filter_map(|e| {
                    // Shown while every row it is under is open.
                    let mut depth = 0;
                    let mut above = e.under.clone();
                    while let Some(key) = above {
                        if !state.row_open(&key) {
                            return None;
                        }
                        depth += 1;
                        above = under(&key);
                    }
                    let parent = views
                        .iter()
                        .any(|c| c.under.as_deref() == Some(e.view.key.as_str()));
                    Some((e.view.clone(), Tree { depth, parent }))
                })
                .collect::<Vec<_>>()
        })
    };
    rsx! {
        div { class: "view-group",
            for (view, tree) in listed() {
                div { key: format!("{}|{}|{:?}|{:?}|{}|{tree:?}", view.key, view.name, view.accounts, view.count, view.hidden),
                    {view_row(__scope, state, view.clone(), tree)}
                }
            }
        }
    }
}

/// Where a row stands in the sidebar's tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tree {
    /// How many rows it is under.
    depth: usize,
    /// Whether rows are under it, which it opens and closes.
    parent: bool,
}

#[component]
fn view_row(state: State, view: SavedView, tree: Tree) -> NodeHandle {
    let key = view.key.clone();
    let chosen = view.key.clone();
    let counted = view.clone();
    let hidden = view.hidden;
    let count = Memo::new(move || {
        state.view_counts.with(|c| {
            c.get(&counted.key)
                .and_then(|t| t.shown(counted.count))
                .unwrap_or(0)
        })
    });
    // A view of one account says which, by the account's colour (once
    // the accounts are loaded).
    // Not under an account's row, which says it already.
    let one = match view.accounts.as_slice() {
        [one] if tree.depth < 2 => Some(one.clone()),
        _ => None,
    };
    let dot_style = Memo::new(
        move || match one.as_ref().and_then(|a| account_dot(state, a)) {
            Some(color) => format!("background: {color}"),
            None => "display: none".to_owned(),
        },
    );
    rsx! {
        div {
            class: {move || {
                let mut class = String::from("folder view-row");
                let elsewhere = state.search.get().is_some()
                    || state.view_edit.with(|e| e.as_ref().is_some_and(|e| e.kind == Editing::New));
                if state.open_view.get() == chosen && !elsewhere {
                    class.push_str(" chosen");
                }
                if hidden {
                    class.push_str(" hidden-view");
                }
                class
            }},
            style: {format!("padding-left: {}px", 2 + tree.depth * 14)},
            {chevron(__scope, state, view.key.clone(), tree.parent)}
            div { class: "row-open", onclick: move || state.open_view(&key),
                span { class: "folder-name",
                    span { class: "name-text", {view.name.clone()} }
                    span { class: "dot", style: {move || dot_style.get()} }
                }
                span {
                    class: "count",
                    style: {move || if count.get() > 0 { "" } else { "display: none" }},
                    {move || thousands(count.get())}
                }
            }
            {row_edit(__scope, state, view.key.clone(), view.name.clone())}
        }
    }
}

/// ✎ on a view's row: the one way to change a view, opening the view
/// panel on it. Shown only while the row is hovered or it has the focus,
/// in place of the row's count.
fn row_edit(__scope: &mut RenderScope, state: State, key: String, name: String) -> NodeHandle {
    let editing = key.clone();
    let pressed = Memo::new(move || {
        state
            .view_edit
            .with(|e| e.as_ref().is_some_and(|e| e.is_of(&editing)))
    });
    let title = format!("Edit {name}");
    let label = title.clone();
    rsx! {
        button {
            class: {move || if pressed.get() { "row-edit pressed" } else { "row-edit" }},
            title: {title.clone()},
            aria-label: {label.clone()},
            aria-pressed: {move || if pressed.get() { "true" } else { "false" }},
            onclick: move || state.toggle_edit_of(&key),
            {icon(__scope, Icon::Write)}
        }
    }
}

/// What opens and closes a row's rows; room for one on a row without.
fn chevron(__scope: &mut RenderScope, state: State, key: String, parent: bool) -> NodeHandle {
    if !parent {
        return rsx! { span { class: "chevron" } };
    }
    let shown = key.clone();
    let open = Memo::new(move || state.row_open(&shown));
    rsx! {
        button {
            class: "chevron row-chevron",
            aria-label: {move || if open.get() { "Close" } else { "Open" }},
            onclick: move || state.toggle_row(&key),
            {move || if open.get() { "\u{25be}" } else { "\u{25b8}" }}
        }
    }
}

/// A row for what is shown but not in the sidebar yet: a search, or a
/// view being made, with × to let it go. A search's row has ✎, as a
/// view's has, to refine it in the panel; a view being made has the
/// panel open already.
#[component]
fn unsaved_row(state: State, kind: Editing) -> NodeHandle {
    let (name, close_title) = match kind {
        Editing::Search => ("Search", "Close search"),
        _ => ("New view", "Don't make it"),
    };
    let close = move || match kind {
        Editing::Search => state.close_search(),
        // A view not made yet is let go of, name or not.
        _ => state.view_edit.set(None),
    };
    rsx! {
        div { class: "folder view-row chosen unsaved",
            div { class: "row-open",
                span { class: "folder-name", span { class: "name-text", {name} } }
            }
            if kind == Editing::Search {
                {search_edit(__scope, state)}
            }
            button { class: "row-close", title: close_title, aria-label: close_title, onclick: close, "×" }
        }
    }
}

/// ✎ on the search's row: refine the search in the view panel.
fn search_edit(__scope: &mut RenderScope, state: State) -> NodeHandle {
    let pressed = Memo::new(move || {
        state
            .view_edit
            .with(|e| e.as_ref().is_some_and(|e| e.kind == Editing::Search))
    });
    rsx! {
        button {
            class: {move || if pressed.get() { "row-edit pressed" } else { "row-edit" }},
            title: "Refine this search",
            aria-label: "Refine this search",
            aria-pressed: {move || if pressed.get() { "true" } else { "false" }},
            onclick: move || state.toggle_edit_search(),
            {icon(__scope, Icon::Write)}
        }
    }
}

/// The colour that marks an account, when there is more than one.
fn account_dot(state: State, account: &EmailAddress) -> Option<&'static str> {
    state.sidebar.with(|s| {
        let accounts = &s.as_ref()?.accounts;
        if accounts.len() < 2 {
            return None;
        }
        accounts
            .iter()
            .position(|a| &a.account.address == account)
            .map(account_color)
    })
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
