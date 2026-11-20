//! The open view's header, after the views mockup: its name, the accounts
//! it shows, its order and its menu; then, while the person has changed it
//! without saving, the filters added and a bar to save or undo that.

use postrinch_model::{EmailAddress, ListOrder};
use rinch::prelude::*;

use crate::state::State;
use crate::ui::account_color;
use crate::ui::icons::{Icon, icon};
use crate::ui::view_dialog::ViewDialog;

#[component]
pub fn view_head(state: State) -> NodeHandle {
    let changed = move || state.view_changes.with(|c| c.any());
    rsx! {
        div { class: "view-head",
            div { class: "view-title-row",
                h1 { class: "view-title", {move || state.saved_view().map(|e| e.view.name).unwrap_or_default()} }
                {account_menu(__scope, state)}
                {sort_menu(__scope, state)}
                {view_menu(__scope, state)}
            }
            if !state.view_changes.get().added.is_empty() {
                div { class: "view-added",
                    span { class: "view-added-name", "Added:" }
                    for term in state.view_changes.get().added {
                        span { key: term.clone(), class: "view-filter",
                            {term.clone()}
                            button {
                                class: "view-filter-x",
                                title: "Take this filter off",
                                aria-label: "Take this filter off",
                                onclick: {
                                    let term = term.clone();
                                    move || state.remove_filter(&term)
                                },
                                "×"
                            }
                        }
                    }
                }
            }
            if changed() {
                div { class: "view-changed",
                    span { class: "view-changed-line", "Changed from the saved view" }
                    div { class: "view-changed-buttons",
                    button { class: "view-undo", onclick: move || state.undo_view_changes(), "Undo changes" }
                    Button {
                        size: "xs",
                        variant: "default",
                        onclick: move || {
                            if let Some(view) = state.shown_view() {
                                state.view_dialog.set(Some(ViewDialog::save_as(&view)));
                            }
                        },
                        "Save as new view…"
                    }
                    Button { size: "xs", onclick: move || state.save_view_changes(), "Save" }
                    }
                }
            }
        }
    }
}

/// A button in the header that opens a menu below it.
fn menu_button(
    __scope: &mut RenderScope,
    open: Signal<bool>,
    kind: &'static str,
    title: &'static str,
    label: NodeHandle,
    on: impl Fn() -> bool + 'static,
    items: Vec<NodeHandle>,
) -> NodeHandle {
    let on = Memo::new(on);
    rsx! {
        Popover {
            opened_fn: move || open.get(),
            onclose: move || open.set(false),
            position: "bottom-end",
            width: "240px",
            PopoverTarget {
                button {
                    class: {move || match (open.get(), on.get()) {
                        (true, _) => format!("head-button {kind} open"),
                        (false, true) => format!("head-button {kind} on"),
                        (false, false) => format!("head-button {kind}"),
                    }},
                    title: title,
                    aria-haspopup: "menu",
                    onclick: move || open.update(|o| *o = !*o),
                    {label}
                }
            }
            PopoverDropdown {
                div { class: "head-menu", role: "menu", {items} }
            }
        }
    }
}

/// A menu item that closes its menu and does `act`.
fn item(
    __scope: &mut RenderScope,
    open: Signal<bool>,
    name: String,
    ticked: impl Fn() -> bool + 'static,
    act: impl Fn() + 'static,
) -> NodeHandle {
    let ticked = Memo::new(ticked);
    rsx! {
        button {
            class: "head-choice",
            role: "menuitem",
            onclick: move || {
                open.set(false);
                act();
            },
            span { class: "head-tick", {move || if ticked.get() { "✓" } else { "" }} }
            span { class: "name-text", {name} }
        }
    }
}

/// The accounts the view shows: all of them, or one.
#[component]
fn account_menu(state: State) -> NodeHandle {
    let open = Signal::new(false);
    let shown = move || state.shown_view().map(|v| v.accounts).unwrap_or_default();
    let all = item(
        __scope,
        open,
        "All accounts".into(),
        move || shown().is_empty(),
        move || state.show_accounts(Vec::new()),
    );
    // The accounts as they load: none until the sidebar's first answer.
    let each = rsx! {
        div { class: "head-menu-part",
            for account in crate::ui::labels::sidebar_accounts(state) {
                div { key: account.key(),
                    {item(
                        __scope,
                        open,
                        account.to_string(),
                        {
                            let ticked = account.clone();
                            move || shown() == [ticked.clone()]
                        },
                        {
                            let chosen = account.clone();
                            move || state.show_accounts(vec![chosen.clone()])
                        },
                    )}
                }
            }
        }
    };
    let items = vec![all, each];
    let label = rsx! {
        span { class: "head-label",
            span {
                class: "dot",
                style: {move || match shown().as_slice() {
                    [one] => colour(state, one).map_or("display: none".to_owned(), |c| format!("background: {c}")),
                    _ => "display: none".to_owned(),
                }},
            }
            span { class: "name-text", {move || match shown().as_slice() {
                [] => "All accounts".to_owned(),
                [one] => one.to_string(),
                many => format!("{} accounts", many.len()),
            }} }
            span { class: "head-caret", "▾" }
        }
    };
    menu_button(
        __scope,
        open,
        "head-account",
        "The accounts this view shows",
        label,
        move || !shown().is_empty(),
        items,
    )
}

/// The colour that marks an account, when there is more than one.
fn colour(state: State, account: &EmailAddress) -> Option<&'static str> {
    state.sidebar.with(|s| {
        let accounts = &s.as_ref()?.accounts;
        let at = accounts
            .iter()
            .position(|a| &a.account.address == account)?;
        (accounts.len() > 1).then(|| account_color(at))
    })
}

const ORDERS: [ListOrder; 3] = [
    ListOrder::NewestFirst,
    ListOrder::OldestFirst,
    ListOrder::UnreadFirst,
];

fn order_name(order: ListOrder) -> &'static str {
    match order {
        ListOrder::NewestFirst => "Newest first",
        ListOrder::OldestFirst => "Oldest first",
        ListOrder::UnreadFirst => "Unread first",
    }
}

/// The view's order, kept with the view whether or not it is saved.
#[component]
fn sort_menu(state: State) -> NodeHandle {
    let open = Signal::new(false);
    let order = move || state.saved_view().map(|e| e.view.order).unwrap_or_default();
    let items = ORDERS
        .into_iter()
        .map(|o| {
            item(
                __scope,
                open,
                order_name(o).into(),
                move || order() == o,
                move || state.set_view_order(o),
            )
        })
        .collect();
    let label = rsx! {
        span { class: "head-label",
            {icon(__scope, Icon::Sort)}
            span { {move || order_name(order())} }
        }
    };
    menu_button(
        __scope,
        open,
        "head-order",
        "The list's order",
        label,
        || false,
        items,
    )
}

/// The view's menu: change it, hide it, and remove it or put it back.
#[component]
fn view_menu(state: State) -> NodeHandle {
    let open = Signal::new(false);
    let entry = move || state.saved_view();
    let edit = item(
        __scope,
        open,
        "Edit view…".into(),
        || false,
        move || {
            if let Some(e) = entry() {
                state.view_dialog.set(Some(ViewDialog::edit(&e.view)));
            }
        },
    );
    let hide = rsx! {
        button {
            class: "head-choice",
            role: "menuitem",
            onclick: move || {
                open.set(false);
                if let Some(mut view) = entry().map(|e| e.view) {
                    view.hidden = !view.hidden;
                    state.keep_view(view);
                }
            },
            span { class: "head-tick" }
            span { {move || if entry().is_some_and(|e| e.view.hidden) { "Show in the sidebar" } else { "Hide from the sidebar" }} }
        }
    };
    let remove = rsx! {
        button {
            class: "head-choice",
            role: "menuitem",
            style: {move || match entry() {
                Some(e) if e.original.as_ref().is_some_and(|o| *o == e.view) => "display: none",
                _ => "",
            }},
            onclick: move || {
                open.set(false);
                if let Some(e) = entry() {
                    state.remove_view(&e.view.key);
                }
            },
            span { class: "head-tick" }
            span { {move || if entry().is_some_and(|e| e.original.is_some()) { "Back to the original" } else { "Delete view" }} }
        }
    };
    let note = rsx! {
        div {
            class: "head-note",
            style: {move || if entry().is_some_and(|e| e.original.is_some()) { "" } else { "display: none" }},
            {move || entry().map(|e| format!("{} is built in, so it can't be deleted.", e.view.name)).unwrap_or_default()}
        }
    };
    let items = vec![
        edit,
        hide,
        rsx! { div { class: "head-rule" } },
        remove,
        note,
    ];
    let label = rsx! { span { class: "head-label", {icon(__scope, Icon::More)} } };
    menu_button(
        __scope,
        open,
        "head-more",
        "This view",
        label,
        || false,
        items,
    )
}

/// The header's styles.
pub const CSS: &str = r#"
.view-head { flex-shrink: 0; display: flex; flex-direction: column; border-bottom: 1px solid var(--line-soft); }
.view-title-row { display: flex; align-items: center; gap: 6px; padding: 12px 14px 8px 20px; min-width: 0; }
.view-title { flex: 1 1 auto; min-width: 0; margin: 0; font-size: 18px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.head-button {
    height: 28px;
    display: flex; align-items: center;
    padding: 0 10px;
    border: 1px solid var(--field); border-radius: 14px;
    background: var(--surface); color: var(--ink-2);
    font: inherit; font-size: 13px;
    cursor: pointer; white-space: nowrap;
}
.head-button:hover { background: var(--hover); }
.head-button.on { border-color: var(--notice-line); background: var(--accent-tint); color: var(--accent-ink); }
.head-button.open { border-color: var(--accent); background: var(--accent-tint); color: var(--accent); }
.head-label { display: flex; align-items: center; gap: 6px; min-width: 0; }
/* An account's address is cut short, so the view's name keeps its room. */
.head-account { max-width: 150px; }
.head-account .head-label, .head-account .name-text { flex: 1; }
.head-caret { color: var(--muted); font-size: 11px; }
.head-menu { display: flex; flex-direction: column; padding: 5px; }
.head-menu-part { display: flex; flex-direction: column; }
.head-choice {
    display: flex; align-items: center; gap: 8px;
    height: 32px; padding: 0 8px;
    border: 0; border-radius: 6px;
    background: transparent; color: var(--ink);
    font: inherit; font-size: 13px; text-align: left;
    cursor: pointer; min-width: 0;
}
.head-choice:hover { background: var(--row-hover); }
.head-tick { width: 14px; flex-shrink: 0; color: var(--accent); }
.head-rule { height: 1px; margin: 4px 0; background: var(--line); }
.head-note { padding: 2px 8px 6px 30px; font-size: 12px; color: var(--muted); }
.view-added { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; padding: 6px 20px; border-top: 1px solid var(--line-soft); }
.view-added-name { font-size: 12px; color: var(--muted); }
.view-filter {
    display: flex; align-items: center; gap: 4px;
    height: 22px; padding: 0 4px 0 9px;
    border: 1px solid var(--field); border-radius: 11px;
    font-size: 12px; color: var(--ink-2);
}
.view-filter-x { border: 0; background: transparent; color: var(--muted); font: inherit; cursor: pointer; padding: 0 3px; }
.view-filter-x:hover { color: var(--ink); }
/* In a narrow list the buttons go under the line, at the right. */
.view-changed {
    display: flex; flex-wrap: wrap; align-items: center; gap: 6px;
    padding: 7px 14px 7px 20px;
    background: var(--warn-bg); border-top: 1px solid var(--warn-line);
}
.view-changed-line { flex: 1 0 auto; font-size: 13px; color: var(--trouble); white-space: nowrap; }
.view-changed-buttons { margin-left: auto; display: flex; align-items: center; gap: 6px; }
.view-undo { border: 0; background: transparent; color: var(--trouble); font: inherit; font-size: 13px; cursor: pointer; padding: 0 6px; }
.view-undo:hover { text-decoration: underline; }
"#;
