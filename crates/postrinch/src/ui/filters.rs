//! The filters beside the search box, after the Filters mockup: who it is
//! from and to and when, what the conversation is (unread, starred, with
//! attachments, who wrote last), which accounts, and which labels and
//! folders it has or not (`label_picker`). They write into the search text
//! (`Filters::apply`), so what the box holds is always the whole search;
//! Search runs it and Save as view… keeps it as a view.

use postrinch_commands::{Filters, Last};
use rinch::prelude::*;

use crate::state::State;
use crate::ui::label_picker::label_picker;
use crate::ui::view_dialog::ViewDialog;

/// A choice's value for "no filter".
const ANY: &str = "\u{1f}any";

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
        self.labels.set(f);
    }

    fn filters(self) -> Filters {
        let some = |s: Signal<String>| Some(s.get()).filter(|v| !v.trim().is_empty() && v != ANY);
        Filters {
            account: some(self.account),
            within: some(self.within),
            from: some(self.from),
            to: some(self.to),
            labels: self.labels.with(|l| l.labels.clone()),
            all_labels: self.labels.with(|l| l.all_labels),
            without: self.labels.with(|l| l.without.clone()),
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

/// Labels and folders, every account's, Has or Not.
#[component]
fn label_column(state: State, f: Fields) -> NodeHandle {
    let picker = label_picker(
        __scope,
        state,
        "LABELS AND FOLDERS",
        "filters-head",
        move || f.labels.get(),
        move |l| f.labels.set(l),
    );
    rsx! {
        div { class: "filters-column",
            {picker}
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

pub const CSS: &str = r#"
.filters { display: flex; flex-direction: column; gap: 14px; padding: 6px 4px; }
.filters-columns { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 26px; }
.filters-column { display: flex; flex-direction: column; gap: 10px; min-width: 0; }
.filters-head { margin: 4px 0 0; font-size: 11px; font-weight: 600; letter-spacing: 0.06em; color: var(--muted); }
.filters-grid { display: grid; grid-template-columns: 52px minmax(0, 1fr); align-items: center; row-gap: 8px; column-gap: 10px; }
.filters-name { font-size: 13px; color: var(--muted); }
.chip-slot { display: contents; }
.filters-manage { align-self: flex-start; border: 0; background: transparent; padding: 0; color: var(--accent); font: inherit; font-size: 12px; cursor: pointer; }
.filters-manage:hover { text-decoration: underline; }
.filters-actions { display: flex; align-items: center; gap: 8px; padding-top: 12px; border-top: 1px solid var(--line-soft); }
.filters-actions .spacer { flex: 1; }
"#;
