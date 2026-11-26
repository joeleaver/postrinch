//! The open view's order, in the bar over the list: a button showing only
//! the order's icon, named in its tooltip, which opens the three orders.
//! A rinch DropdownMenu, so the keyboard works as in any menu: Enter or
//! Space opens it, the arrows move, Enter picks, Escape closes.
//! The order lives only here, never also in the view panel.

use postrinch_model::ListOrder;
use rinch::prelude::*;

use crate::state::State;
use crate::ui::icons::{Icon, icon};

/// The orders a list can be in, as the menu lists them.
const ORDERS: [ListOrder; 3] = [
    ListOrder::NewestFirst,
    ListOrder::OldestFirst,
    ListOrder::UnreadFirst,
];

/// What the menu and the tooltip call an order.
pub fn order_name(order: ListOrder) -> &'static str {
    match order {
        ListOrder::NewestFirst => "Newest first",
        ListOrder::OldestFirst => "Oldest first",
        ListOrder::UnreadFirst => "Unread first",
    }
}

/// The button's tooltip and accessible name: which order is on.
pub fn order_title(order: ListOrder) -> String {
    format!("Order: {}", order_name(order))
}

fn order_icon(order: ListOrder) -> Icon {
    match order {
        ListOrder::NewestFirst => Icon::NewestFirst,
        ListOrder::OldestFirst => Icon::OldestFirst,
        ListOrder::UnreadFirst => Icon::UnreadFirst,
    }
}

#[component]
pub fn order_menu(state: State) -> NodeHandle {
    let open = state.ordering;
    let shown = Memo::new(move || state.shown_order());
    rsx! {
        DropdownMenu {
            opened_fn: move || open.get(),
            on_close: move || open.set(false),
            position: "bottom-end",
            width: "180px",
            DropdownMenuTarget {
                button {
                    class: {move || if open.get() { "icon-button list-order open" } else { "icon-button list-order" }},
                    title: {move || order_title(shown.get())},
                    aria-label: {move || order_title(shown.get())},
                    aria-haspopup: "menu",
                    aria-expanded: {move || if open.get() { "true" } else { "false" }},
                    onclick: move || open.update(|o| *o = !*o),
                    // Each order's icon, the one that is on shown: an icon
                    // built in a reactive closure would need a scope.
                    for order in ORDERS {
                        span {
                            key: format!("{order:?}"),
                            class: "order-icon",
                            style: {move || if shown.get() == order { "" } else { "display: none" }},
                            {icon(__scope, order_icon(order))}
                        }
                    }
                }
            }
            DropdownMenuDropdown {
                div { class: "order-menu", role: "menu", aria-label: "Order",
                    // The menu's own item class, which its arrow keys and
                    // type-ahead move among; radio items of our own, to tick
                    // the order that is on.
                    for order in ORDERS {
                        button {
                            key: format!("{order:?}"),
                            class: {move || if shown.get() == order { "rinch-dropdown-menu__item order-choice on" } else { "rinch-dropdown-menu__item order-choice" }},
                            role: "menuitemradio",
                            aria-checked: {move || if shown.get() == order { "true" } else { "false" }},
                            onclick: move || {
                                open.set(false);
                                state.set_view_order(order);
                            },
                            span { class: "order-tick", {move || if shown.get() == order { "\u{2713}" } else { "" }} }
                            span { class: "order-choice-icon", {icon(__scope, order_icon(order))} }
                            span { class: "rinch-dropdown-menu__item-label", {order_name(order)} }
                        }
                    }
                }
            }
        }
    }
}

/// The order button's and menu's styles.
pub const CSS: &str = r#"
.list-bar .rinch-dropdown-menu,
.list-bar .rinch-dropdown-menu__target { display: flex; }
.list-order { width: 30px; height: 30px; flex-shrink: 0; border: 1px solid var(--field); }
.order-icon { display: flex; }
.order-menu { display: flex; flex-direction: column; }
.order-choice {
    display: flex; align-items: center; gap: 8px; padding: 6px 10px;
    border: 0; border-radius: 6px; background: transparent; color: var(--ink);
    font: inherit; font-size: 13px; text-align: left; cursor: pointer;
}
.order-choice:focus { background: var(--hover); }
.order-choice:hover { background: var(--hover); }
.order-choice.on { color: var(--accent-ink); font-weight: 600; }
.order-tick { width: 12px; flex-shrink: 0; }
.order-choice-icon { display: flex; color: var(--muted); }
.order-choice.on .order-choice-icon { color: var(--accent-ink); }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_order_has_its_own_name_and_icon() {
        let names: Vec<_> = ORDERS.iter().map(|&o| order_name(o)).collect();
        assert_eq!(names, ["Newest first", "Oldest first", "Unread first"]);
        let icons: Vec<_> = ORDERS.iter().map(|&o| order_icon(o)).collect();
        assert_eq!(
            icons,
            [Icon::NewestFirst, Icon::OldestFirst, Icon::UnreadFirst]
        );
    }

    #[test]
    fn the_button_names_the_order_that_is_on() {
        assert_eq!(order_title(ListOrder::UnreadFirst), "Order: Unread first");
        assert_eq!(order_title(ListOrder::default()), "Order: Newest first");
    }
}
