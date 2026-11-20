//! The filters beside the search box, after the Filters mockup: who it is
//! from and to and when, what the conversation is (unread, starred, with
//! attachments, who wrote last), which accounts, and which labels and
//! folders, any of them. They write into the search text
//! (`Filters::apply`), so what the box holds is always the whole search;
//! Search runs it and Save as view… keeps it as a view.

use postrinch_commands::{Filters, LabelNode, Last};
use postrinch_model::EmailAddress;
use rinch::prelude::*;

use crate::state::State;
use crate::ui::account_color;
use crate::ui::view_dialog::ViewDialog;

/// A choice's value for "no filter".
const ANY: &str = "\u{1f}any";

/// A label or folder the panel offers.
#[derive(Debug, Clone, PartialEq)]
struct Pick {
    account: EmailAddress,
    /// The provider's whole name (`Clients/Acme`), as `label:` takes it.
    full: String,
    /// What this level is called.
    name: String,
    depth: usize,
    color: Option<String>,
}

/// The panel's fields, as signals.
#[derive(Clone, Copy)]
struct Fields {
    account: Signal<String>,
    within: Signal<String>,
    from: Signal<String>,
    to: Signal<String>,
    labels: Signal<Filters>,
    unread: Signal<bool>,
    starred: Signal<bool>,
    attachments: Signal<bool>,
    last: Signal<Option<Last>>,
    find: Signal<String>,
}

impl Fields {
    fn new() -> Self {
        Self {
            account: Signal::new(String::new()),
            within: Signal::new(String::new()),
            from: Signal::new(String::new()),
            to: Signal::new(String::new()),
            labels: Signal::new(Filters::default()),
            unread: Signal::new(false),
            starred: Signal::new(false),
            attachments: Signal::new(false),
            last: Signal::new(None),
            find: Signal::new(String::new()),
        }
    }

    /// Start from what the box holds now.
    fn read(self, query: &str) {
        let f = Filters::read(query);
        self.account
            .set(f.account.clone().unwrap_or_else(|| ANY.to_owned()));
        self.within
            .set(f.within.clone().unwrap_or_else(|| ANY.to_owned()));
        self.from.set(f.from.clone().unwrap_or_default());
        self.to.set(f.to.clone().unwrap_or_default());
        self.unread.set(f.unread);
        self.starred.set(f.starred);
        self.attachments.set(f.attachments);
        self.last.set(f.last);
        self.find.set(String::new());
        self.labels.set(f);
    }

    fn filters(self) -> Filters {
        let some = |s: Signal<String>| Some(s.get()).filter(|v| !v.trim().is_empty() && v != ANY);
        Filters {
            account: some(self.account),
            within: some(self.within),
            from: some(self.from),
            to: some(self.to),
            labels: self.labels.get().labels,
            unread: self.unread.get(),
            starred: self.starred.get(),
            attachments: self.attachments.get(),
            last: self.last.get(),
        }
    }
}

/// The filters button beside the search box, and its panel.
#[component]
pub fn search_filters(state: State) -> NodeHandle {
    let f = Fields::new();
    let open = move || {
        f.read(&state.search_draft.get());
        state.filters_open.set(true);
    };
    let query = move || f.filters().apply(&state.search_draft.get());
    let search = move || {
        state.search_draft.set(query());
        state.filters_open.set(false);
        state.run_search();
    };
    let save_as = move || {
        let text = query();
        state.search_draft.set(text.clone());
        state.filters_open.set(false);
        state.view_dialog.set(Some(ViewDialog::new_view(&text)));
    };
    let clear = move || f.read("");
    rsx! {
        Popover {
            opened_fn: move || state.filters_open.get(),
            onclose: move || state.filters_open.set(false),
            position: "bottom-end",
            width: "720px",
            PopoverTarget {
                button {
                    class: {move || if state.filters_open.get() { "filters-button open" } else { "filters-button" }},
                    title: "Search filters",
                    aria-haspopup: "dialog",
                    onclick: move || if state.filters_open.get() { state.filters_open.set(false) } else { open() },
                    "Filters"
                }
            }
            PopoverDropdown {
                div { class: "filters",
                    div { class: "filters-columns",
                        {left_column(__scope, state, f)}
                        {label_column(__scope, state, f)}
                    }
                    // The confirming button last, on the right.
                    div { class: "filters-actions",
                        Button { variant: "subtle", size: "sm", onclick: clear, "Clear" }
                        span { class: "spacer" }
                        Button { variant: "default", size: "sm", onclick: save_as, "Save as view\u{2026}" }
                        Button { variant: "filled", size: "sm", onclick: search, "Search" }
                    }
                }
            }
        }
    }
}

#[component]
fn left_column(state: State, f: Fields) -> NodeHandle {
    let times = [
        (ANY, "Any time"),
        ("7d", "Week"),
        ("1m", "Month"),
        ("1y", "Year"),
    ]
    .into_iter()
    .map(|(value, name)| chip(__scope, value.to_owned(), name.to_owned(), f.within))
    .collect::<Vec<_>>();
    let flags = [
        ("Unread", f.unread),
        ("Starred", f.starred),
        ("Has an attachment", f.attachments),
    ]
    .into_iter()
    .map(|(name, on)| {
        rsx! {
            button {
                class: {move || if on.get() { "view-chip on" } else { "view-chip" }},
                onclick: move || on.update(|v| *v = !*v),
                {name}
            }
        }
    })
    .collect::<Vec<_>>();
    let lasts = [
        (Last::Me, "I replied last"),
        (Last::Them, "Someone else replied last"),
    ]
    .into_iter()
    .map(|(which, name)| {
        rsx! {
            button {
                class: {move || if f.last.get() == Some(which) { "view-chip on" } else { "view-chip" }},
                onclick: move || f.last.update(|l| *l = if *l == Some(which) { None } else { Some(which) }),
                {name}
            }
        }
    })
    .collect::<Vec<_>>();
    rsx! {
        div { class: "filters-column",
            p { class: "filters-head", "MESSAGES" }
            div { class: "filters-grid",
                span { class: "filters-name", "From" }
                TextInput { placeholder: "Anyone", value_fn: move || f.from.get(), oninput: move |v: String| f.from.set(v) }
                span { class: "filters-name", "To" }
                TextInput { placeholder: "Anyone", value_fn: move || f.to.get(), oninput: move |v: String| f.to.set(v) }
                span { class: "filters-name", "When" }
                div { class: "view-chips", {times} }
            }
            p { class: "filters-head", "CONVERSATIONS" }
            div { class: "view-chips", {flags} {lasts} }
            p { class: "filters-head", "ACCOUNTS" }
            div { class: "view-chips",
                {chip(__scope, ANY.to_owned(), "All accounts".to_owned(), f.account)}
                for account in crate::ui::labels::sidebar_accounts(state) {
                    div { key: account.key(), class: "chip-slot",
                        {chip(__scope, account.to_string(), account.to_string(), f.account)}
                    }
                }
            }
        }
    }
}

/// One of a row of chips, one chosen.
fn chip(
    __scope: &mut RenderScope,
    value: String,
    name: String,
    chosen: Signal<String>,
) -> NodeHandle {
    let mine = value.clone();
    rsx! {
        button {
            class: {move || if chosen.get() == mine { "view-chip on" } else { "view-chip" }},
            onclick: move || chosen.set(value.clone()),
            {name}
        }
    }
}

/// Labels and folders, every account's, any of them.
#[component]
fn label_column(state: State, f: Fields) -> NodeHandle {
    let picks = Memo::new(move || {
        let find = f.find.get().trim().to_lowercase();
        state.sidebar.with(|s| {
            let mut picks = Vec::new();
            for (at, a) in s.iter().flat_map(|s| &s.accounts).enumerate() {
                let mut mine = Vec::new();
                walk(&a.account.address, &a.labels.own, "", 0, &find, &mut mine);
                let heading = AccountHeading {
                    key: a.account.address.key(),
                    dot: format!("background: {}", account_color(at)),
                    name: format!("{} \u{b7} labels", a.account.address),
                };
                picks.push((heading, mine));
            }
            picks
        })
    });
    let several = move || picks.get().len() > 1;
    rsx! {
        div { class: "filters-column",
            div { class: "filters-labels-head",
                p { class: "filters-head", "LABELS AND FOLDERS" }
                span { class: "filters-any", "Any of these" }
            }
            TextInput { placeholder: "Find a label or folder", value_fn: move || f.find.get(), oninput: move |v: String| f.find.set(v) }
            div { class: "filters-labels",
                for (heading, list) in picks.get() {
                    div { key: format!("{}|{}", heading.key, list.len()), class: "filters-account",
                        div {
                            class: "filters-account-name",
                            style: {move || if several() { "" } else { "display: none" }},
                            span { class: "dot", style: {heading.dot.clone()} }
                            {heading.name.clone()}
                        }
                        for pick in list.clone() {
                            div { key: format!("{}|{}", pick.account.key(), pick.full),
                                {label_pick(__scope, f, pick.clone())}
                            }
                        }
                    }
                }
            }
            button {
                class: "filters-manage",
                onclick: move || {
                    state.filters_open.set(false);
                    state.settings_section.set(crate::state::SettingsSection::Labels);
                    state.view.set(crate::state::View::Settings);
                },
                "Manage labels in Settings"
            }
        }
    }
}

/// An account above its labels: its colour and address.
#[derive(Debug, Clone, PartialEq)]
struct AccountHeading {
    key: String,
    dot: String,
    name: String,
}

/// The labels under `nodes` (while finding, those whose name has `find`
/// in it), each with its whole name. Every one of the person's labels,
/// hidden ones too: any of them can be what a search looks for.
fn walk(
    account: &EmailAddress,
    nodes: &[LabelNode],
    parent: &str,
    depth: usize,
    find: &str,
    out: &mut Vec<Pick>,
) {
    for node in nodes {
        let full = if parent.is_empty() {
            node.segment.clone()
        } else {
            format!("{parent}/{}", node.segment)
        };
        let wanted = find.is_empty()
            || full.to_lowercase().contains(find)
            || node.name.to_lowercase().contains(find);
        if wanted && node.entry.is_some() {
            out.push(Pick {
                account: account.clone(),
                full: full.clone(),
                name: if find.is_empty() {
                    node.name.clone()
                } else {
                    full.clone()
                },
                depth: if find.is_empty() { depth } else { 0 },
                color: node.color.clone(),
            });
        }
        walk(account, &node.children, &full, depth + 1, find, out);
    }
}

#[component]
fn label_pick(f: Fields, pick: Pick) -> NodeHandle {
    let (full, toggled) = (pick.full.clone(), pick.full.clone());
    let on = Memo::new(move || f.labels.with(|l| l.has_label(&full)));
    let dot = pick
        .color
        .clone()
        .map(|c| rsx! { span { class: "dot", style: {format!("background: {c}")} } });
    rsx! {
        button {
            class: "filters-label",
            style: {format!("padding-left: {}px", 4 + 16 * pick.depth)},
            role: "checkbox",
            aria-checked: {move || if on.get() { "true" } else { "false" }},
            onclick: move || {
                let toggled = toggled.clone();
                f.labels.update(move |l| l.toggle_label(&toggled))
            },
            span { class: {move || if on.get() { "filters-box on" } else { "filters-box" }}, {move || if on.get() { "✓" } else { "" }} }
            {dot}
            span { class: "name-text", {pick.name.clone()} }
        }
    }
}

pub const CSS: &str = r#"
.filters { display: flex; flex-direction: column; gap: 14px; padding: 6px 4px; }
.filters-columns { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 26px; }
.filters-column { display: flex; flex-direction: column; gap: 10px; min-width: 0; }
.filters-head { margin: 4px 0 0; font-size: 11px; font-weight: 600; letter-spacing: 0.06em; color: var(--muted); }
.filters-grid { display: grid; grid-template-columns: 52px minmax(0, 1fr); align-items: center; row-gap: 8px; column-gap: 10px; }
.filters-name { font-size: 13px; color: var(--muted); }
.chip-slot { display: contents; }
.filters-labels-head { display: flex; align-items: center; }
.filters-labels-head .filters-head { flex: 1; }
.filters-any { font-size: 12px; color: var(--muted); }
.filters-labels { display: flex; flex-direction: column; max-height: 320px; overflow-y: auto; }
.filters-account { display: flex; flex-direction: column; }
.filters-account-name { display: flex; align-items: center; gap: 6px; margin: 8px 0 2px; font-size: 12px; color: var(--muted); }
.filters-label {
    display: flex; align-items: center; gap: 8px;
    height: 28px; padding-right: 6px;
    border: 0; border-radius: 6px;
    background: transparent; color: var(--ink);
    font: inherit; font-size: 13px; text-align: left; cursor: pointer;
    min-width: 0;
}
.filters-label:hover { background: var(--row-hover); }
.filters-box {
    width: 14px; height: 14px; flex-shrink: 0;
    display: flex; align-items: center; justify-content: center;
    border: 1px solid var(--field); border-radius: 3px;
    font-size: 10px; color: #ffffff;
}
.filters-box.on { background: var(--rinch-primary-color); border-color: var(--rinch-primary-color); }
.filters-manage { align-self: flex-start; border: 0; background: transparent; padding: 0; color: var(--accent); font: inherit; font-size: 12px; cursor: pointer; }
.filters-manage:hover { text-decoration: underline; }
.filters-actions { display: flex; align-items: center; gap: 8px; padding-top: 12px; border-top: 1px solid var(--line-soft); }
.filters-actions .spacer { flex: 1; }
"#;
