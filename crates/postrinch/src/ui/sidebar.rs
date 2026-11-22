//! The sidebar, after the views mockup: the inbox (with each account's
//! under it, when there are several) and its tabs, a thin
//! line with + to make a view, the person's own views, another line, then
//! Everything else, All mail, Sent and Drafts; and sync below.

use postrinch_commands::Place;
use postrinch_model::{EmailAddress, SavedView};
use rinch::prelude::*;

use crate::state::State;
use crate::ui::icons::{Icon, icon};
use crate::view_edit::Editing;
use crate::sync::thousands;
use crate::ui::account_color;

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
                    {views_in(__scope, state, Place::Top)}
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
                    // A second line only when there are views of the person's
                    // own between the two.
                    div {
                        class: "view-line",
                        style: {move || if has_own(state) { "" } else { "display: none" }},
                        span { class: "view-rule" }
                    }
                    {views_in(__scope, state, Place::Bottom)}
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

fn has_own(state: State) -> bool {
    state
        .views
        .with(|v| v.iter().any(|e| e.place == Place::Yours && !e.view.hidden))
}

/// The views listed in one part of the sidebar. Keyed by what a row shows
/// but its count: a row is built once and its count follows in place.
#[component]
fn views_in(state: State, place: Place) -> NodeHandle {
    let listed = move || {
        let counts = state.view_counts.get();
        let show_hidden = state.show_hidden.get();
        state.views.with(|views| {
            views
                .iter()
                .filter(|e| e.place == place && crate::views::listed(&e.view, &counts, show_hidden))
                .map(|e| (e.view.clone(), e.nested))
                .collect::<Vec<_>>()
        })
    };
    rsx! {
        div { class: "view-group",
            for (view, nested) in listed() {
                div { key: format!("{}|{}|{:?}|{:?}|{}|{nested}", view.key, view.name, view.accounts, view.count, view.hidden),
                    {view_row(__scope, state, view.clone(), nested)}
                }
            }
        }
    }
}

#[component]
fn view_row(state: State, view: SavedView, nested: bool) -> NodeHandle {
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
    let one = match view.accounts.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    };
    let dot_style = Memo::new(
        move || match one.as_ref().and_then(|a| account_dot(state, a)) {
            Some(color) => format!("background: {color}"),
            None => "display: none".to_owned(),
        },
    );
    let editing = view.key.clone();
    let edit = view.key.clone();
    let pressed = Memo::new(move || {
        state.view_edit.with(|e| {
            e.as_ref()
                .is_some_and(|e| e.kind == Editing::View && e.draft.key == editing)
        })
    });
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
                if nested {
                    class.push_str(" nested");
                }
                class
            }},
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
            {pencil(__scope, view.name.clone(), move || pressed.get(), move || {
                if pressed.get() { state.finish_edit() } else { state.edit_view(&edit) }
            })}
        }
    }
}

/// The pencil on a row, which opens (or closes) the view panel on it.
fn pencil(
    __scope: &mut RenderScope,
    name: String,
    pressed: impl Fn() -> bool + Copy + 'static,
    onclick: impl Fn() + 'static,
) -> NodeHandle {
    rsx! {
        button {
            class: {move || if pressed() { "row-pencil pressed" } else { "row-pencil" }},
            title: "Edit view",
            aria-label: {format!("Edit {name}")},
            aria-pressed: {move || if pressed() { "true" } else { "false" }},
            onclick: onclick,
            {icon(__scope, Icon::Write)}
        }
    }
}

/// A row for what is shown but not in the sidebar yet: a search, or a
/// view being made. It has the pencil every view has.
#[component]
fn unsaved_row(state: State, kind: Editing) -> NodeHandle {
    let pressed = move || {
        state
            .view_edit
            .with(|e| e.as_ref().is_some_and(|e| e.kind == kind))
    };
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
            div { class: "row-open", onclick: move || if !pressed() && kind == Editing::Search { state.edit_search() },
                span { class: "folder-name", span { class: "name-text", {name} } }
            }
            {pencil(__scope, name.to_owned(), pressed, move || {
                match (pressed(), kind) {
                    (true, _) => state.finish_edit(),
                    (false, Editing::Search) => state.edit_search(),
                    (false, _) => {}
                }
            })}
            button { class: "row-close", title: close_title, aria-label: close_title, onclick: close, "×" }
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
