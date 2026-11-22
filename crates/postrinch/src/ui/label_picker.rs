//! Labels to look for, after the "View labels: Has and Not" mockup: every
//! account's labels in a list to find in, each one a conversation must have
//! (Has) or must not (Not), and whether it needs any or all of the Has
//! ones. Filters and the view dialog both show it; it reads and writes
//! their [`Filters`], so the search text stays the one place a search
//! lives.

use postrinch_commands::{Filters, LabelNode, LabelPick};
use postrinch_model::EmailAddress;
use rinch::prelude::*;

use crate::state::State;
use crate::ui::account_color;

/// A label the picker offers.
#[derive(Debug, Clone, PartialEq)]
struct Pick {
    account: EmailAddress,
    /// The provider's whole name (`Clients/Acme`), as `label:` takes it.
    full: String,
    /// The labels above it (`Clients/`), shown faint.
    parent: String,
    /// What this level is called.
    name: String,
    color: Option<String>,
}

/// An account above its labels: its colour and address.
#[derive(Debug, Clone, PartialEq)]
struct AccountHeading {
    key: String,
    dot: String,
    name: String,
}

/// The labels, found among, and how many of how many show.
#[derive(Debug, Clone, PartialEq)]
struct Offered {
    accounts: Vec<(AccountHeading, Vec<Pick>)>,
    shown: usize,
    total: usize,
}

/// Change the filters `current` holds, through `set`.
fn edit(current: Memo<Filters>, set: impl Fn(Filters), change: impl FnOnce(&mut Filters)) {
    let mut filters = current.get();
    change(&mut filters);
    set(filters);
}

/// The picker, headed `title` (in `title_class`'s style). `get` gives the
/// filters it shows and `set` takes them changed.
pub fn label_picker(
    __scope: &mut RenderScope,
    state: State,
    title: &'static str,
    title_class: &'static str,
    get: impl Fn() -> Filters + Copy + 'static,
    set: impl Fn(Filters) + Copy + 'static,
) -> NodeHandle {
    let current = Memo::new(get);
    let find = Signal::new(String::new());
    let offered = Memo::new(move || offered(state, &find.get().trim().to_lowercase()));
    let several = move || offered.get().accounts.len() > 1;
    let all = move || current.get().all_labels;
    let mode = |__scope: &mut RenderScope, want: bool, name: &'static str| {
        rsx! {
            button {
                class: {move || if all() == want { "on" } else { "" }},
                role: "radio",
                aria-checked: {move || if all() == want { "true" } else { "false" }},
                onclick: move || edit(current, set, |f| f.all_labels = want),
                {name}
            }
        }
    };
    let (any_of, all_of) = (
        mode(__scope, false, "any of them"),
        mode(__scope, true, "all of them"),
    );
    rsx! {
        div { class: "picker",
            div { class: "picker-head",
                p { class: {title_class}, {title} }
                span { class: "picker-note", "Has" }
                div { class: "picker-mode", role: "radiogroup", aria-label: "How many of the labels it needs",
                    {any_of} {all_of}
                }
            }
            {chosen(__scope, current, set, LabelPick::Has)}
            {chosen(__scope, current, set, LabelPick::Not)}
            div { class: "picker-box",
                div { class: "picker-find",
                    TextInput {
                        placeholder: "Find a label",
                        value_fn: move || find.get(),
                        oninput: move |v: String| find.set(v),
                    }
                    span { class: "picker-note", {move || { let o = offered.get(); format!("{} of {}", o.shown, o.total) }} }
                }
                div { class: "picker-list",
                    for (heading, list) in offered.get().accounts {
                        div { key: format!("{}|{}", heading.key, list.len()), class: "picker-account",
                            div {
                                class: "picker-account-name",
                                style: {move || if several() { "" } else { "display: none" }},
                                span { class: "dot", style: {heading.dot.clone()} }
                                {heading.name.clone()}
                            }
                            for pick in list.clone() {
                                div { key: format!("{}|{}", pick.account.key(), pick.full),
                                    {row(__scope, current, set, pick.clone())}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The labels asked to be there (`Has`) or not, as chips to take out.
fn chosen(
    __scope: &mut RenderScope,
    current: Memo<Filters>,
    set: impl Fn(Filters) + Copy + 'static,
    which: LabelPick,
) -> NodeHandle {
    let names = Memo::new(move || {
        let f = current.get();
        match which {
            LabelPick::Has => f.labels,
            LabelPick::Not => f.without,
        }
    });
    let (name, class) = match which {
        LabelPick::Has => ("Has", "picker-chip has"),
        LabelPick::Not => ("Not", "picker-chip not"),
    };
    rsx! {
        // Shown while empty too, so the list below stays put as labels
        // are picked.
        div { class: "picker-chosen",
            span { class: "picker-note picker-which", {name} }
            div { class: "picker-chips",
                span {
                    class: "picker-none",
                    style: {move || if names.get().is_empty() { "" } else { "display: none" }},
                    "None"
                }
                for label in names.get() {
                    span { key: label.clone(), class: "picker-chip-slot",
                        {chip(__scope, current, set, class, label.clone())}
                    }
                }
            }
        }
    }
}

/// A chosen label, and the × that takes it out.
fn chip(
    __scope: &mut RenderScope,
    current: Memo<Filters>,
    set: impl Fn(Filters) + Copy + 'static,
    class: &'static str,
    label: String,
) -> NodeHandle {
    let remove = format!("Remove {label}");
    let name = label.clone();
    rsx! {
        span { class: {class},
            span { class: "picker-chip-name", {name} }
            button {
                class: "picker-x",
                aria-label: {remove.clone()},
                onclick: move || {
                    let label = label.clone();
                    edit(current, set, move |f| f.pick_label(&label, None))
                },
                "\u{d7}"
            }
        }
    }
}

/// One label, with its Has and Not.
fn row(
    __scope: &mut RenderScope,
    current: Memo<Filters>,
    set: impl Fn(Filters) + Copy + 'static,
    pick: Pick,
) -> NodeHandle {
    let full = pick.full.clone();
    let asked = Memo::new(move || current.get().label(&full));
    let button = |__scope: &mut RenderScope, which: LabelPick, name: &'static str, on: &'static str| {
        let full = pick.full.clone();
        rsx! {
            button {
                class: {move || if asked.get() == Some(which) { on } else { "" }},
                aria-pressed: {move || if asked.get() == Some(which) { "true" } else { "false" }},
                onclick: move || {
                    let full = full.clone();
                    let next = if asked.get() == Some(which) { None } else { Some(which) };
                    edit(current, set, move |f| f.pick_label(&full, next))
                },
                {name}
            }
        }
    };
    let has = button(__scope, LabelPick::Has, "Has", "has");
    let not = button(__scope, LabelPick::Not, "Not", "not");
    let dot = pick
        .color
        .clone()
        .map(|c| rsx! { span { class: "dot", style: {format!("background: {c}")} } });
    rsx! {
        div { class: "picker-row",
            {dot}
            span { class: "picker-name",
                span { class: "picker-parent", {pick.parent.clone()} }
                {pick.name.clone()}
            }
            div { class: "picker-seg", {has} {not} }
        }
    }
}

/// Every account's labels, its own and other apps' (while finding, those
/// whose name has `find` in it). Hidden ones too: any of them can be what
/// a search looks for.
fn offered(state: State, find: &str) -> Offered {
    state.sidebar.with(|s| {
        let (mut shown, mut total) = (0, 0);
        let mut accounts = Vec::new();
        for (at, a) in s.iter().flat_map(|s| &s.accounts).enumerate() {
            let mut every = Vec::new();
            walk(&a.account.address, &a.labels.own, &mut every);
            for app in &a.labels.other_apps {
                walk(&a.account.address, &app.labels, &mut every);
            }
            total += every.len();
            every.retain(|p| {
                find.is_empty()
                    || p.full.to_lowercase().contains(find)
                    || p.name.to_lowercase().contains(find)
            });
            shown += every.len();
            let heading = AccountHeading {
                key: a.account.address.key(),
                dot: format!("background: {}", account_color(at)),
                name: a.account.address.to_string(),
            };
            accounts.push((heading, every));
        }
        Offered {
            accounts,
            shown,
            total,
        }
    })
}

fn walk(account: &EmailAddress, nodes: &[LabelNode], out: &mut Vec<Pick>) {
    for node in nodes {
        if node.entry.is_some() && !node.full.is_empty() {
            let parent = node
                .full
                .rfind('/')
                .map(|at| node.full[..=at].to_owned())
                .unwrap_or_default();
            out.push(Pick {
                account: account.clone(),
                full: node.full.clone(),
                parent,
                name: node.name.clone(),
                color: node.color.clone(),
            });
        }
        walk(account, &node.children, out);
    }
}

pub const CSS: &str = r#"
.picker { display: flex; flex-direction: column; gap: 8px; min-width: 0; }
.picker-head { display: flex; align-items: center; gap: 10px; }
.picker-head > p { flex: 1; margin: 0; }
.picker-note { font-size: 12px; color: var(--muted); }
.picker-mode, .picker-seg { display: flex; border: 1px solid var(--field); border-radius: 6px; overflow: hidden; flex-shrink: 0; }
.picker-mode button, .picker-seg button {
    height: 24px; padding: 0 10px; border: 0; background: transparent;
    color: var(--muted); font: inherit; font-size: 12px; cursor: pointer;
}
.picker-mode button + button, .picker-seg button + button { border-left: 1px solid var(--field); }
.picker-mode button:hover, .picker-seg button:hover { background: var(--hover); }
.picker-mode .on, .picker-seg .has { background: var(--accent-tint); color: var(--accent-ink); }
.picker-seg .not { background: var(--red-tint); color: var(--red-ink); }
.picker-chosen { display: flex; align-items: flex-start; gap: 6px; }
.picker-which { width: 30px; flex-shrink: 0; line-height: 26px; }
.picker-none { font-size: 12px; line-height: 26px; color: var(--faint); }
.picker-chips { flex: 1; min-width: 0; display: flex; flex-wrap: wrap; gap: 6px; }
.picker-chip-slot { display: contents; }
.picker-chip {
    height: 26px; padding: 0 4px 0 10px; border-radius: 13px; border: 1px solid;
    display: flex; align-items: center; gap: 4px; font-size: 13px;
}
.picker-chip.has { background: var(--accent-tint); border-color: var(--notice-line); color: var(--accent-ink); }
.picker-chip.not { background: var(--red-tint); border-color: var(--red-line); color: var(--red-ink); }
.picker-chip.not .picker-chip-name { text-decoration: line-through; }
.picker-x {
    width: 20px; height: 20px; border: 0; border-radius: 10px; padding: 0;
    background: transparent; color: inherit; font: inherit; font-size: 14px; cursor: pointer;
}
.picker-x:hover { background: var(--hover); }
.picker-box { border: 1px solid var(--field); border-radius: 8px; }
.picker-find { display: flex; align-items: center; gap: 8px; padding: 6px 10px; border-bottom: 1px solid var(--line-soft); }
.picker-find > :first-child { flex: 1; }
.picker-list { display: flex; flex-direction: column; height: 214px; overflow-y: auto; padding: 4px; }
.picker-account { display: flex; flex-direction: column; }
.picker-account-name { display: flex; align-items: center; gap: 6px; padding: 8px 8px 2px; font-size: 12px; color: var(--muted); }
.picker-row { display: flex; align-items: center; gap: 8px; min-height: 32px; padding: 0 8px; border-radius: 6px; font-size: 13px; color: var(--ink); }
.picker-row:hover { background: var(--row-hover); }
.picker-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.picker-parent { color: var(--muted); }
"#;
