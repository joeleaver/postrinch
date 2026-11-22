//! The view panel, after the "one pencil, one panel" mockup: everything
//! about a view in one place, opened by the pencil on its row (or + for a
//! new view, or the pencil on a search's row). It stands where the reader
//! does, so the list beside it shows the view as changed. Which mail it
//! shows is in view_panel_mail.rs; here are the list's order, how the
//! sidebar lists it, and Save.

use postrinch_model::{ListOrder, SavedView, ViewCount};
use rinch::prelude::*;

use crate::state::State;
use crate::view_edit::{Editing, ViewEdit};

/// How the sidebar lists a view, as one choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Listed {
    Always,
    WhenItHasMail,
    Hidden,
}

fn listed(view: &SavedView) -> Listed {
    match (view.hidden, view.hide_when_empty) {
        (true, _) => Listed::Hidden,
        (false, true) => Listed::WhenItHasMail,
        (false, false) => Listed::Always,
    }
}

fn set_listed(view: &mut SavedView, to: Listed) {
    view.hidden = to == Listed::Hidden;
    view.hide_when_empty = to == Listed::WhenItHasMail;
}

/// The panel's view as changed so far, read by `f`.
pub(crate) fn read<T: Default>(state: State, f: impl FnOnce(&ViewEdit) -> T) -> T {
    state
        .view_edit
        .with(|e| e.as_ref().map(f).unwrap_or_default())
}

/// A row of chips of which one is chosen.
pub(crate) fn choice<T: Copy + PartialEq + 'static>(
    __scope: &mut RenderScope,
    state: State,
    label: &'static str,
    choices: Vec<(T, &'static str)>,
    get: fn(&SavedView) -> T,
    set: fn(&mut SavedView, T),
) -> NodeHandle {
    let chips: Vec<NodeHandle> = choices
        .into_iter()
        .map(|(value, name)| {
            let on = move || read(state, |e| Some(get(&e.draft))) == Some(value);
            rsx! {
                button {
                    class: {move || if on() { "view-chip on" } else { "view-chip" }},
                    role: "radio",
                    aria-checked: {move || if on() { "true" } else { "false" }},
                    onclick: move || state.change_edit(move |v| set(v, value)),
                    {name}
                }
            }
        })
        .collect();
    rsx! {
        div { class: "panel-field",
            span { class: "panel-name", {label} }
            div { class: "view-chips", role: "radiogroup", aria-label: label, {chips} }
        }
    }
}

/// The panel, while it is open.
#[component]
pub fn view_panel(state: State) -> NodeHandle {
    rsx! {
        section { class: "view-panel", aria-label: "View",
            // Keyed on the view and what is being done to it, so the
            // fields start from it.
            for key in state.view_edit.get().map(|e| format!("{}|{:?}", e.draft.key, e.kind)) {
                div { key: key.clone(), class: "panel-inner",
                    {body(__scope, state)}
                }
            }
        }
    }
}

#[component]
fn body(state: State) -> NodeHandle {
    let kind = read(state, |e| Some(e.kind)).unwrap_or(Editing::View);
    let title = match kind {
        Editing::View => "EDIT VIEW",
        Editing::New => "NEW VIEW",
        Editing::Search => "SEARCH",
    };
    let orders = choice(
        __scope,
        state,
        "Order",
        vec![
            (ListOrder::NewestFirst, "Newest first"),
            (ListOrder::OldestFirst, "Oldest first"),
            (ListOrder::UnreadFirst, "Unread first"),
        ],
        |v| v.order,
        |v, o| v.order = o,
    );
    let counts = choice(
        __scope,
        state,
        "Count",
        vec![
            (ViewCount::Unread, "Unread"),
            (ViewCount::All, "Everything in it"),
            (ViewCount::None, "No count"),
        ],
        |v| v.count,
        |v, c| v.count = c,
    );
    let listing = choice(
        __scope,
        state,
        "In the sidebar",
        vec![
            (Listed::Always, "Always"),
            (Listed::WhenItHasMail, "Only when it has mail"),
            (Listed::Hidden, "Hidden"),
        ],
        listed,
        set_listed,
    );
    let name_hint = match kind {
        Editing::Search => "Name it to keep it as a view",
        _ => "Name",
    };
    rsx! {
        div { class: "panel-parts",
        // The view's name heads the panel, as it heads the list.
        div { class: "panel-head",
            p { class: "panel-kind", {title} }
            div { class: "panel-title",
                TextInput {
                    placeholder: name_hint,
                    value_fn: move || read(state, |e| e.draft.name.clone()),
                    oninput: move |v: String| state.change_edit(|view| view.name = v),
                    onsubmit: move || state.save_edit(),
                }
            }
        }
        div { class: "panel-body",
            p { class: "panel-group", "WHICH MAIL" }
            {crate::ui::view_panel_mail::which_mail(__scope, state)}
            p { class: "panel-group", "HOW IT SHOWS" }
            {orders}
            {counts}
            {listing}
        }
        // A keyed loop, not `if let` (rinch#2105).
        for error in read(state, |e| e.error.clone()) {
            p { key: error.clone(), class: "panel-trouble", {error.clone()} }
        }
        {footer(__scope, state, kind)}
        }
    }
}

/// Delete (or put back) on the left; Cancel, then Save, on the right.
#[component]
fn footer(state: State, kind: Editing) -> NodeHandle {
    // What the left button does to the view, if anything: a built-in view
    // that is as it first was has nothing to put back.
    let remove = Memo::new(move || {
        if kind != Editing::View {
            return None;
        }
        let entry = state.saved_view()?;
        match &entry.original {
            None => Some("Delete view"),
            Some(original) if *original != entry.view => Some("Back to the original"),
            Some(_) => None,
        }
    });
    let (cancel, save) = match kind {
        Editing::Search => ("Close", "Save as view"),
        _ => ("Cancel", "Save"),
    };
    rsx! {
        div { class: "panel-foot",
            button {
                class: "panel-remove",
                style: {move || if remove.get().is_some() { "" } else { "display: none" }},
                onclick: move || state.remove_edited(),
                {move || remove.get().unwrap_or_default()}
            }
            span { class: "spacer" }
            Button { variant: "default", size: "sm", onclick: move || state.cancel_edit(), {cancel} }
            Button { variant: "filled", size: "sm", onclick: move || state.save_edit(), {save} }
        }
    }
}

pub const CSS: &str = r#"
.view-panel { flex: 1; min-width: 0; display: flex; flex-direction: column; background: var(--surface); }
.panel-inner { flex: 1; min-height: 0; display: flex; flex-direction: column; }
.panel-parts { flex: 1; min-height: 0; display: flex; flex-direction: column; }
.panel-mail { display: flex; flex-direction: column; gap: 8px; }
.panel-head { padding: 12px 24px 14px; border-bottom: 1px solid var(--line-soft); display: flex; flex-direction: column; gap: 6px; }
.panel-kind { margin: 0; font-size: 11px; font-weight: 600; letter-spacing: 0.06em; color: var(--muted); }
.panel-title .rinch-text-input__input { height: 38px; font-size: 17px; font-weight: 600; }
.view-panel .rinch-text-input__input { min-height: 32px; }
.panel-body { flex: 1; min-height: 0; overflow-y: auto; padding: 4px 24px 16px; display: flex; flex-direction: column; gap: 8px; }
.panel-group { margin: 12px 0 2px; padding-top: 12px; border-top: 1px solid var(--line-soft); font-size: 11px; font-weight: 600; letter-spacing: 0.06em; color: var(--muted); }
.panel-group:first-child { border-top: 0; padding-top: 0; }
.panel-field { display: grid; grid-template-columns: 120px minmax(0, 1fr); align-items: center; gap: 12px; min-height: 32px; }
.panel-name { font-size: 13px; color: var(--ink-2); }
.panel-query .rinch-text-input__input { font-family: "IBM Plex Mono", ui-monospace, monospace; font-size: 12px; }
.panel-trouble { margin: 0; padding: 8px 24px; background: var(--warn-bg); color: var(--trouble); font-size: 13px; border-top: 1px solid var(--warn-line); }
.panel-foot { display: flex; align-items: center; gap: 8px; padding: 12px 24px; border-top: 1px solid var(--line); }
.panel-foot .spacer { flex: 1; }
.panel-remove { border: 0; background: transparent; padding: 0 4px; color: var(--danger); font: inherit; font-size: 13px; cursor: pointer; }
.panel-remove:hover { text-decoration: underline; }
.view-chips { display: flex; flex-wrap: wrap; gap: 6px; }
.view-chip {
    height: 28px; padding: 0 12px;
    display: flex; align-items: center; gap: 6px;
    border: 1px solid var(--field); border-radius: 14px;
    background: transparent; color: var(--ink-2);
    font: inherit; font-size: 13px; cursor: pointer;
}
.view-chip:hover { background: var(--hover); }
.view-chip.on { border-color: var(--notice-line); background: var(--accent-tint); color: var(--accent-ink); }
"#;
