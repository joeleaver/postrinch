//! The row above the list, after the list header mockup: what can be done
//! to the open conversation (archive, trash, mark unread, its labels, the
//! star) at the left, and the list's order at the right.

use postrinch_commands::ListOrder;
use rinch::prelude::*;

use crate::state::State;
use crate::ui::icons::{Icon, icon};
use crate::ui::thread_actions::toolbar;

#[component]
pub fn list_actions(state: State) -> NodeHandle {
    rsx! {
        div { class: "list-actions",
            div {
                // Nothing to act on until a conversation is open.
                class: {move || if state.selected.get().is_some() { "list-tools" } else { "list-tools idle" }},
                {toolbar(__scope, state)}
            }
            span { class: "list-actions-gap" }
            // A search keeps its own order.
            div { style: {move || if state.search.get().is_some() { "display: none" } else { "" }},
                {sort_menu(__scope, state)}
            }
        }
    }
}

const ORDERS: [ListOrder; 3] = [
    ListOrder::NewestFirst,
    ListOrder::OldestFirst,
    ListOrder::UnreadFirst,
];

fn name(order: ListOrder) -> &'static str {
    match order {
        ListOrder::NewestFirst => "Newest first",
        ListOrder::OldestFirst => "Oldest first",
        ListOrder::UnreadFirst => "Unread first",
    }
}

/// The order button and its menu of orders, the one in use ticked.
#[component]
fn sort_menu(state: State) -> NodeHandle {
    let open = Signal::new(false);
    let choices: Vec<NodeHandle> = ORDERS
        .into_iter()
        .map(|order| {
            rsx! {
                button {
                    class: "sort-choice",
                    role: "menuitemradio",
                    aria-checked: {move || if state.list_order.get() == order { "true" } else { "false" }},
                    onclick: move || {
                        open.set(false);
                        state.set_list_order(order);
                    },
                    span { class: "sort-tick", {move || if state.list_order.get() == order { "✓" } else { "" }} }
                    span { {name(order)} }
                }
            }
        })
        .collect();
    rsx! {
        Popover {
            opened_fn: move || open.get(),
            onclose: move || open.set(false),
            position: "bottom-end",
            width: "180px",
            PopoverTarget {
                button {
                    class: {move || if open.get() { "sort-button open" } else { "sort-button" }},
                    title: "The list's order",
                    aria-haspopup: "menu",
                    onclick: move || open.update(|o| *o = !*o),
                    {icon(__scope, Icon::Sort)}
                    span { {move || name(state.list_order.get())} }
                }
            }
            PopoverDropdown {
                div { class: "sort-menu", role: "menu", {choices} }
            }
        }
    }
}

/// The row's styles.
pub const CSS: &str = r#"
.list-actions {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 12px 14px 8px;
}
.list-actions .icon-button { width: 30px; height: 30px; }
.list-tools.idle { opacity: 0.4; pointer-events: none; }
.list-actions-gap { flex: 1; }
.sort-button {
    height: 30px;
    display: flex; align-items: center; gap: 6px;
    padding: 0 10px;
    border: 1px solid var(--field);
    border-radius: 6px;
    background: var(--surface);
    color: var(--ink);
    font: inherit;
    font-size: 13px;
    cursor: pointer;
    white-space: nowrap;
}
.sort-button:hover { background: var(--hover); }
.sort-button.open { border-color: var(--accent); background: var(--accent-tint); color: var(--accent); }
.sort-menu { display: flex; flex-direction: column; padding: 5px; }
.sort-choice {
    display: flex; align-items: center; gap: 8px;
    height: 32px; padding: 0 8px;
    border: 0; border-radius: 6px;
    background: transparent;
    color: var(--ink);
    font: inherit;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
}
.sort-choice:hover { background: var(--row-hover); }
.sort-tick { width: 14px; color: var(--accent); }
"#;
