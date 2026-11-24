//! The bar over the list, in place of a header that repeated the
//! sidebar: a box that narrows the open view as you type (never saved),
//! the view's order, and the one way to change a view, ✎, which opens the
//! view panel. While searching it says how much was found instead.

use postrinch_model::ListOrder;
use rinch::prelude::*;

use crate::state::State;
use crate::sync::thousands;
use crate::ui::icons::{Icon, icon};
use crate::view_edit::Editing;

/// The filter box's id, which Escape clears (keys.rs).
pub const FILTER_ID: &str = "list-filter";

#[component]
pub fn list_bar(state: State) -> NodeHandle {
    let searching = move || state.search.get().is_some();
    let pressed = Memo::new(move || state.view_edit.with(|e| e.is_some()));
    let orders: Vec<SelectOption> = [
        (ListOrder::NewestFirst, "Newest first"),
        (ListOrder::OldestFirst, "Oldest first"),
        (ListOrder::UnreadFirst, "Unread first"),
    ]
    .into_iter()
    .map(|(o, name)| SelectOption::new(order_word(o), name))
    .collect();
    rsx! {
        div { class: "list-bar",
            // Rebuilt for each view, so the box names it.
            for key in filter_keys(state) {
                div { key: key.clone(), class: "list-filter",
                    style: {move || if searching() { "display: none" } else { "" }},
                    TextInput {
                        spellcheck: false,
                        id: FILTER_ID,
                        size: "sm",
                        placeholder: {format!("Filter {}", view_name(state))},
                        value_fn: move || state.list_filter.get(),
                        oninput: move |v: String| state.filter_list(v),
                    }
                }
            }
            div { class: "list-found",
                style: {move || if searching() { "" } else { "display: none" }},
                {move || found_line(state)}
            }
            div { class: "list-order",
                style: {move || if searching() { "display: none" } else { "" }},
                Select {
                    size: "sm",
                    data: orders,
                    value_fn: move || order_word(state.shown_order()).to_owned(),
                    onchange: move |v: String| state.set_view_order(order_of(&v)),
                }
            }
            button {
                class: {move || if pressed.get() { "icon-button list-edit pressed" } else { "icon-button list-edit" }},
                title: {move || if searching() { "Refine this search" } else { "Edit this view" }},
                aria-label: {move || if searching() { "Refine this search" } else { "Edit this view" }},
                aria-pressed: {move || if pressed.get() { "true" } else { "false" }},
                onclick: move || state.toggle_edit(),
                {icon(__scope, Icon::Write)}
            }
        }
    }
}

/// One key per view the box is for, so it is built again for another.
fn filter_keys(state: State) -> Vec<String> {
    let new = state
        .view_edit
        .with(|e| e.as_ref().is_some_and(|e| e.kind == Editing::New));
    vec![if new {
        "new".to_owned()
    } else {
        state.open_view.get()
    }]
}

fn view_name(state: State) -> String {
    let new = state
        .view_edit
        .with(|e| e.as_ref().is_some_and(|e| e.kind == Editing::New));
    if new {
        return "the new view".to_owned();
    }
    state
        .saved_view()
        .map(|e| e.view.name)
        .unwrap_or_else(|| "this view".to_owned())
}

/// "1,204 conversations" found by the search.
fn found_line(state: State) -> String {
    match state.results.get() {
        Some(r) => {
            let n = r.threads.len();
            let messages = r.threads.first().is_some_and(|t| t.message.is_some());
            let what = match (messages, n == 1) {
                (false, true) => "conversation",
                (false, false) => "conversations",
                (true, true) => "message",
                (true, false) => "messages",
            };
            let more = if r.more { "+" } else { "" };
            format!("{}{more} {what} found", thousands(n))
        }
        None => String::new(),
    }
}

fn order_word(order: ListOrder) -> &'static str {
    match order {
        ListOrder::NewestFirst => "newest",
        ListOrder::OldestFirst => "oldest",
        ListOrder::UnreadFirst => "unread",
    }
}

fn order_of(word: &str) -> ListOrder {
    match word {
        "oldest" => ListOrder::OldestFirst,
        "unread" => ListOrder::UnreadFirst,
        _ => ListOrder::NewestFirst,
    }
}

/// The bar's styles.
pub const CSS: &str = r#"
.list-bar { flex-shrink: 0; display: flex; align-items: center; gap: 8px; padding: 12px 14px 0; min-width: 0; }
.list-filter { flex: 1; min-width: 0; }
.list-found { flex: 1; min-width: 0; color: var(--muted); font-size: 13px; padding-left: 4px; }
.list-order { width: 132px; flex-shrink: 0; }
.list-edit { width: 30px; height: 30px; flex-shrink: 0; }
.list-edit svg { width: 15px; height: 15px; }
.list-edit.pressed { background: var(--accent); border-color: var(--accent); color: var(--surface); }
"#;
