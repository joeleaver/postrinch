//! Search's own parts of the window, after the Search mockup: the filters
//! beside the search box, and the lines under the results' heading about
//! Gmail's part and about a query that does not read.
//!
//! The filters write into the search text (`Filters::apply`), so what the
//! box holds is always the whole search.

use postrinch_commands::Filters;
use rinch::prelude::*;

use crate::remote_search::remote_line;
use crate::state::State;

/// A choice's value for "no filter".
const ANY: &str = "\u{1f}any";

/// The lines under the results' heading.
#[component]
pub fn search_notes(state: State) -> NodeHandle {
    rsx! {
        div { class: "search-notes",
            div {
                class: "search-note",
                style: {move || {
                    let line = remote_line(state.remote.get().as_ref());
                    if line.is_empty() || state.search_error.get().is_some() { "display: none".to_owned() } else { String::new() }
                }},
                {move || remote_line(state.remote.get().as_ref())}
            }
            for why in state.search_error.get() {
                div { key: why.clone(), class: "search-error", {why.clone()} }
            }
        }
    }
}

/// The filters button beside the search box, and its panel.
#[component]
pub fn search_filters(state: State) -> NodeHandle {
    let account = Signal::new(String::new());
    let within = Signal::new(String::new());
    let from = Signal::new(String::new());
    let label = Signal::new(String::new());
    let unread = Signal::new(false);
    let starred = Signal::new(false);
    let attachments = Signal::new(false);

    // Open on what the box holds now.
    let open = move || {
        let f = Filters::read(&state.search_draft.get());
        account.set(f.account.unwrap_or_else(|| ANY.to_owned()));
        within.set(f.within.unwrap_or_else(|| ANY.to_owned()));
        from.set(f.from.unwrap_or_default());
        label.set(f.label.unwrap_or_default());
        unread.set(f.unread);
        starred.set(f.starred);
        attachments.set(f.attachments);
        state.filters_open.set(true);
    };
    let reset = move || {
        for s in [account, within] {
            s.set(ANY.to_owned());
        }
        for s in [from, label] {
            s.set(String::new());
        }
        for s in [unread, starred, attachments] {
            s.set(false);
        }
    };
    let done = move || {
        let some = |s: Signal<String>| Some(s.get()).filter(|v| !v.trim().is_empty() && v != ANY);
        let filters = Filters {
            account: some(account),
            within: some(within),
            from: some(from),
            label: some(label),
            unread: unread.get(),
            starred: starred.get(),
            attachments: attachments.get(),
        };
        state
            .search_draft
            .set(filters.apply(&state.search_draft.get()));
        state.filters_open.set(false);
        state.run_search();
    };

    let accounts = Memo::new(move || {
        state.sidebar.with(|s| {
            let mut all = vec![(ANY.to_owned(), "All".to_owned())];
            for a in s.iter().flat_map(|s| &s.accounts) {
                let address = a.account.address.to_string();
                all.push((address.clone(), address));
            }
            all
        })
    });
    let times = Memo::new(move || {
        [
            (ANY, "Any time"),
            ("7d", "Week"),
            ("1m", "Month"),
            ("1y", "Year"),
        ]
        .into_iter()
        .map(|(v, l)| (v.to_owned(), l.to_owned()))
        .collect::<Vec<_>>()
    });

    rsx! {
        Popover {
            opened_fn: move || state.filters_open.get(),
            onclose: move || state.filters_open.set(false),
            position: "bottom-end",
            width: "320px",
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
                    div { class: "filters-grid",
                        span { class: "filters-name", "Account" }
                        {choices(__scope, account, accounts)}
                        span { class: "filters-name", "When" }
                        {choices(__scope, within, times)}
                        span { class: "filters-name", "From" }
                        TextInput { placeholder: "Anyone", value_fn: move || from.get(), oninput: move |v: String| from.set(v) }
                        span { class: "filters-name", "Label" }
                        TextInput { placeholder: "Any label", value_fn: move || label.get(), oninput: move |v: String| label.set(v) }
                        span { class: "filters-name", "Only" }
                        div { class: "filters-only",
                            Checkbox { label: "Unread", checked_fn: move || unread.get(), onchange: move || unread.update(|v| *v = !*v) }
                            Checkbox { label: "Starred", checked_fn: move || starred.get(), onchange: move || starred.update(|v| *v = !*v) }
                            Checkbox { label: "Attachments", checked_fn: move || attachments.get(), onchange: move || attachments.update(|v| *v = !*v) }
                        }
                    }
                    p { class: "filters-hint",
                        "These go into the search box as words you can type too: "
                        span { class: "filters-code", "from: label: is:unread has:attachment after:" }
                    }
                    // The confirming button last, on the right.
                    div { class: "filters-actions",
                        Button { variant: "subtle", size: "sm", onclick: reset, "Reset" }
                        span { class: "spacer" }
                        Button { variant: "filled", size: "sm", onclick: done, "Done" }
                    }
                }
            }
        }
    }
}

/// A row of buttons, one chosen: `options` are (value, label).
#[component]
fn choices(chosen: Signal<String>, options: Memo<Vec<(String, String)>>) -> NodeHandle {
    rsx! {
        div { class: "choice-buttons",
            for (value, label) in options.get() {
                div { key: value.clone(), {choice(__scope, chosen, value.clone(), label.clone())} }
            }
        }
    }
}

#[component]
fn choice(chosen: Signal<String>, value: String, label: String) -> NodeHandle {
    let mine = value.clone();
    rsx! {
        button {
            class: {move || if chosen.get() == mine { "choice-button chosen" } else { "choice-button" }},
            onclick: move || chosen.set(value.clone()),
            {label}
        }
    }
}

pub const CSS: &str = r#"
.title-search { display: flex; align-items: center; gap: 6px; }
.title-search > :first-child { flex: 1; min-width: 0; }
.filters-button {
    flex-shrink: 0;
    height: 26px;
    padding: 0 10px;
    border: 1px solid var(--field);
    border-radius: 6px;
    background: var(--panel);
    color: var(--ink-2);
    font-size: 13px;
}
.filters-button:hover, .filters-button.open { background: var(--accent-tint); color: var(--accent-ink); border-color: var(--accent-tint); }
.filters { display: flex; flex-direction: column; gap: 12px; padding: 4px 2px; }
.filters-grid { display: grid; grid-template-columns: 64px minmax(0, 1fr); align-items: center; row-gap: 8px; column-gap: 10px; }
.filters-name { font-size: 13px; color: var(--muted); }
.choice-buttons { display: flex; flex-wrap: wrap; gap: 4px; }
.choice-buttons > div { display: contents; }
.choice-button {
    height: 26px;
    padding: 0 9px;
    border: 1px solid var(--field);
    border-radius: 6px;
    background: var(--surface);
    color: var(--ink);
    font-size: 12px;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}
.choice-button:hover { background: var(--hover); }
.choice-button.chosen { background: var(--accent-tint); border-color: var(--accent); color: var(--accent-ink); font-weight: 500; }
.filters-only { display: flex; flex-wrap: wrap; gap: 4px 14px; font-size: 13px; }
.filters-hint { margin: 0; font-size: 12px; color: var(--muted); line-height: 1.5; }
.filters-code { font-family: 'IBM Plex Mono', monospace; color: var(--ink-2); }
.filters-actions { display: flex; align-items: center; gap: 10px; padding-top: 10px; border-top: 1px solid var(--line-soft); }
.filters-actions .spacer { flex: 1; }
.search-notes { display: flex; flex-direction: column; }
.search-note {
    padding: 8px 16px;
    border-bottom: 1px solid var(--line-soft);
    background: var(--panel);
    font-size: 13px;
    color: var(--ink-2);
}
.search-error { padding: 8px 16px; border-bottom: 1px solid var(--line-soft); background: var(--warn-bg); color: var(--trouble); font-size: 13px; }
.found-by {
    flex-shrink: 0;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--warn-bg);
    color: var(--trouble);
    font-size: 11px;
    font-family: 'IBM Plex Mono', monospace;
}
"#;
