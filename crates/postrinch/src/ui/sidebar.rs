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

#[component]
pub fn sidebar(state: State) -> NodeHandle {
    rsx! {
        nav {
            class: "sidebar",
            style: {move || format!("width: {}px", state.sidebar_width.get())},
            {crate::ui::outgoing::outgoing_lines(__scope, state)}
            div { class: "folders",
                div { class: "folder-group",
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
    rsx! {
        div {
            class: {move || {
                let mut class = String::from("folder");
                if state.open_view.get() == chosen && state.search.get().is_none() {
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
            onclick: move || state.open_view(&key),
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
