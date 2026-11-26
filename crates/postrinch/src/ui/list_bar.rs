//! The bar over the list, in place of a header that repeated the
//! sidebar: Write, a box that narrows the open view as you type (never
//! saved), and the view's order (list_order.rs). While searching it says
//! how much was found instead. A view is changed from ✎ on its row in the
//! sidebar.

use rinch::prelude::*;

use crate::state::State;
use crate::sync::thousands;
use crate::ui::icons::{Icon, icon};
use crate::ui::list_order::order_menu;
use crate::view_edit::Editing;

/// The filter box's id, which Escape clears (keys.rs).
pub const FILTER_ID: &str = "list-filter";

#[component]
pub fn list_bar(state: State) -> NodeHandle {
    let searching = move || state.search.get().is_some();
    rsx! {
        div { class: "list-bar",
            button {
                class: "list-write",
                title: "Write (C)",
                aria-label: "Write",
                onclick: move || state.write(postrinch_commands::Start::New),
                {icon(__scope, Icon::Write)}
            }
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
            div { class: "list-order-slot",
                style: {move || if searching() { "display: none" } else { "" }},
                {order_menu(__scope, state)}
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

/// The bar's styles.
pub const CSS: &str = r#"
.list-bar { flex-shrink: 0; display: flex; align-items: center; gap: 8px; padding: 12px 14px 0; min-width: 0; }
.list-filter { flex: 1; min-width: 0; }
.list-found { flex: 1; min-width: 0; color: var(--muted); font-size: 13px; padding-left: 4px; }
.list-order-slot { display: flex; flex-shrink: 0; }
/* Write: the accent, so it is easy to find. */
.list-write {
    width: 30px; height: 30px; flex-shrink: 0;
    display: flex; align-items: center; justify-content: center;
    padding: 0; border: none; border-radius: 6px;
    background: var(--accent); color: #ffffff; cursor: pointer;
}
.list-write:hover { filter: brightness(1.1); }
.list-write svg { width: 16px; height: 16px; }
"#;
