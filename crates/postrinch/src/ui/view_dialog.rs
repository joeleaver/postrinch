//! Making or changing a view, after the Save as new view mockup: its name,
//! the search it shows (in the search box's own words), its accounts, what
//! the sidebar counts beside it, and whether it hides while empty. Opened
//! by + in the sidebar, Save as new view… on the changed bar and in
//! Filters, and Edit view… on a view's menu.

use postrinch_commands::Filters;
use postrinch_model::{EmailAddress, ListOrder, SavedView, Timestamp, ViewCount};
use rinch::prelude::*;

use crate::state::State;
use crate::ui::account_color;
use crate::ui::label_picker::label_picker;

/// The view the dialog is making or changing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewDialog {
    pub view: SavedView,
    /// A view not saved before, which opens once saved.
    pub is_new: bool,
    pub error: Option<String>,
}

impl ViewDialog {
    /// A new view showing what `query` finds.
    pub fn new_view(query: &str) -> Self {
        let now = Timestamp::from_millis(jiff::Timestamp::now().as_millisecond());
        Self {
            view: SavedView {
                key: format!("v{}", now.as_millis()),
                name: String::new(),
                query: query.trim().to_owned(),
                accounts: Vec::new(),
                count: ViewCount::Unread,
                hide_when_empty: false,
                hidden: false,
                order: ListOrder::default(),
            },
            is_new: true,
            error: None,
        }
    }

    /// A new view showing what `view` shows now, changes and all.
    pub fn save_as(view: &SavedView) -> Self {
        let mut dialog = Self::new_view(&view.query);
        dialog.view.accounts.clone_from(&view.accounts);
        dialog.view.count = view.count;
        dialog.view.order = view.order;
        dialog
    }

    /// `view` as saved, to change.
    pub fn edit(view: &SavedView) -> Self {
        Self {
            view: view.clone(),
            is_new: false,
            error: None,
        }
    }
}

fn change(state: State, edit: impl FnOnce(&mut SavedView)) {
    state.view_dialog.update(|d| {
        if let Some(d) = d {
            edit(&mut d.view);
            d.error = None;
        }
    });
}

fn save(state: State) {
    let Some(dialog) = state.view_dialog.get() else {
        return;
    };
    let mut view = dialog.view;
    view.name = view.name.trim().to_owned();
    view.query = view.query.trim().to_owned();
    let why = if view.name.is_empty() {
        Some("Give the view a name.".to_owned())
    } else {
        state.put_view(view).err()
    };
    match why {
        Some(why) => state
            .view_dialog
            .update(|d| d.iter_mut().for_each(|d| d.error = Some(why.clone()))),
        None => state.view_dialog.set(None),
    }
}

#[component]
pub fn view_dialog(state: State) -> NodeHandle {
    rsx! {
        Modal {
            opened_fn: move || state.view_dialog.get().is_some(),
            onclose: move || state.view_dialog.set(None),
            title: "",
            size: "md",
            // Keyed on the view, so the fields start from it.
            for key in state.view_dialog.get().map(|d| d.view.key) {
                div { key: key.clone(), class: "dialog view-dialog",
                    {body(__scope, state)}
                }
            }
        }
    }
}

#[component]
fn body(state: State) -> NodeHandle {
    let read = move |f: fn(&ViewDialog) -> String| {
        state
            .view_dialog
            .with(|d| d.as_ref().map(f).unwrap_or_default())
    };
    let title = read(|d| {
        if d.is_new {
            "Save as new view".into()
        } else {
            format!("Edit \u{201c}{}\u{201d}", d.view.name)
        }
    });
    let accounts: Vec<EmailAddress> = state.sidebar.with(|s| {
        s.iter()
            .flat_map(|s| s.accounts.iter().map(|a| a.account.address.clone()))
            .collect()
    });
    let several = accounts.len() > 1;
    let shown = move || {
        state.view_dialog.with(|d| {
            d.as_ref()
                .map(|d| d.view.accounts.clone())
                .unwrap_or_default()
        })
    };
    let account_chips: Vec<NodeHandle> = accounts
        .iter()
        .enumerate()
        .map(|(at, account)| {
            let (on, toggled) = (account.clone(), account.clone());
            let color = account_color(at);
            rsx! {
                button {
                    class: {move || if shown().contains(&on) { "view-chip on" } else { "view-chip" }},
                    onclick: move || {
                        let toggled = toggled.clone();
                        change(state, move |v| {
                            if !v.accounts.contains(&toggled) {
                                v.accounts.push(toggled);
                            } else {
                                v.accounts.retain(|a| *a != toggled);
                            }
                        })
                    },
                    span { class: "dot", style: {format!("background: {color}")} }
                    {account.to_string()}
                }
            }
        })
        .collect();
    let count = move || {
        state
            .view_dialog
            .with(|d| d.as_ref().map(|d| d.view.count).unwrap_or_default())
    };
    let counts: Vec<NodeHandle> = [
        (ViewCount::Unread, "Unread"),
        (ViewCount::All, "Everything in it"),
        (ViewCount::None, "No count"),
    ]
    .into_iter()
    .map(|(which, name)| {
        rsx! {
            button {
                class: {move || if count() == which { "view-chip on" } else { "view-chip" }},
                role: "radio",
                aria-checked: {move || if count() == which { "true" } else { "false" }},
                onclick: move || change(state, move |v| v.count = which),
                {name}
            }
        }
    })
    .collect();
    let labels = label_picker(
        __scope,
        state,
        "Labels",
        "view-part-name",
        move || {
            state
                .view_dialog
                .with(|d| d.as_ref().map(|d| Filters::read(&d.view.query)).unwrap_or_default())
        },
        move |l: Filters| change(state, move |v| v.query = l.apply(&v.query)),
    );
    rsx! {
        div { class: "view-fields",
            h2 { class: "dialog-title", {title} }
            TextInput {
                label: "Name",
                value_fn: move || read(|d| d.view.name.clone()),
                oninput: move |v: String| change(state, |view| view.name = v),
                onsubmit: move || save(state),
            }
            {labels}
            div { class: "view-query",
                TextInput {
                    label: "Shows",
                    placeholder: "label:clients is:unread",
                    value_fn: move || read(|d| d.view.query.clone()),
                    oninput: move |v: String| change(state, |view| view.query = v),
                    onsubmit: move || save(state),
                }
                p { class: "dialog-hint", "The search box\u{2019}s words. They follow the labels above, and the other way round." }
            }
            // Shown or not once: the accounts don't change while it is open.
            div { class: "view-part", style: {if several { "" } else { "display: none" }},
                    p { class: "view-part-name", "Accounts" }
                    div { class: "view-chips",
                        button {
                            class: {move || if shown().is_empty() { "view-chip on" } else { "view-chip" }},
                            onclick: move || change(state, |v| v.accounts.clear()),
                            "All accounts"
                        }
                        {account_chips}
                    }
            }
            div { class: "view-part",
                p { class: "view-part-name", "Its count in the sidebar" }
                div { class: "view-chips", role: "radiogroup", {counts} }
            }
            Checkbox {
                label: "Hide it when it\u{2019}s empty",
                checked_fn: move || state.view_dialog.with(|d| d.as_ref().is_some_and(|d| d.view.hide_when_empty)),
                onchange: move || change(state, |v| v.hide_when_empty = !v.hide_when_empty),
            }
            // A keyed loop, not `if let` (rinch#2105).
            for error in state.view_dialog.get().and_then(|d| d.error) {
                p { key: error.clone(), class: "dialog-trouble", {error.clone()} }
            }
            div { class: "dialog-actions",
                Button { variant: "default", size: "sm", onclick: move || state.view_dialog.set(None), "Cancel" }
                Button { variant: "filled", size: "sm", onclick: move || save(state), "Save view" }
            }
        }
    }
}

pub const CSS: &str = r#"
.view-fields { display: flex; flex-direction: column; gap: 14px; }
.view-query .rinch-text-input__input { font-family: "IBM Plex Mono", ui-monospace, monospace; font-size: 13px; }
.view-query .dialog-hint { margin: 6px 0 0; }
.view-part { display: flex; flex-direction: column; gap: 6px; }
.view-part-name { margin: 0; font-size: 13px; color: var(--muted); }
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
