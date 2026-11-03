//! Changing how a label shows on this computer: its right-click menu, and
//! the dialog that renames it or gives it a colour.

use postrinch_model::{EmailAddress, LabelPrefs};
use rinch::prelude::*;

use crate::state::State;

/// A label whose menu is open or which is being renamed or coloured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelEdit {
    pub account: EmailAddress,
    pub label: String,
    /// What the provider calls it, shown when the person clears their name.
    pub provider_name: String,
    pub prefs: LabelPrefs,
    pub mode: EditMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditMode {
    Rename,
    Colour,
}

/// Colours to choose from: distinct from each other and from the blue
/// that marks what is chosen.
const SWATCHES: [(&str, &str); 10] = [
    ("Red", "#c2412d"),
    ("Orange", "#c2621a"),
    ("Amber", "#a87800"),
    ("Green", "#2f8a55"),
    ("Teal", "#1f7a7a"),
    ("Purple", "#7a4fc2"),
    ("Pink", "#c2417a"),
    ("Brown", "#8a5a3c"),
    ("Slate", "#5b6170"),
    ("Black", "#16181d"),
];

/// The right-click menu of a label row.
#[component]
pub fn label_menu(state: State, edit: LabelEdit, shown: bool) -> NodeHandle {
    let changed = edit.prefs != LabelPrefs::default();
    let pinned = edit.prefs.pinned;
    let open = |mode: EditMode| {
        let edit = LabelEdit {
            mode,
            ..edit.clone()
        };
        move || state.editing.set(Some(edit.clone()))
    };
    let save = |change: fn(&mut LabelPrefs)| {
        let edit = edit.clone();
        move || {
            let mut prefs = edit.prefs.clone();
            change(&mut prefs);
            state.save_label_prefs(&edit.account, &edit.label, &prefs);
        }
    };
    let pin = save(|p| p.pinned = !p.pinned);
    let visibility = if shown {
        save(|p| p.shown = Some(false))
    } else {
        save(|p| p.shown = Some(true))
    };
    let reset = changed.then(|| {
        let reset = save(|p| *p = LabelPrefs::default());
        rsx! {
            div {
                DropdownMenuDivider {}
                DropdownMenuItem { onclick: reset, "Undo my changes" }
            }
        }
    });
    rsx! {
        ContextMenuDropdown {
            DropdownMenuItem { onclick: open(EditMode::Rename), "Rename…" }
            DropdownMenuItem { onclick: open(EditMode::Colour), "Colour…" }
            DropdownMenuItem { onclick: pin, {if pinned { "Unpin" } else { "Pin to the top" }} }
            DropdownMenuItem { onclick: visibility, {if shown { "Hide" } else { "Always show" }} }
            {reset}
        }
    }
}

/// The rename or colour dialog, open while a label is being edited.
#[component]
pub fn label_dialog(state: State) -> NodeHandle {
    rsx! {
        Modal {
            opened_fn: move || state.editing.get().is_some(),
            onclose: move || state.editing.set(None),
            title: "",
            size: "sm",
            // Keyed on the label and mode, so each opening starts fresh
            // (rinch#2105 is why this is a loop and not `if let`).
            for edit in state.editing.get() {
                div { key: format!("{}|{}|{:?}", edit.account.key(), edit.label, edit.mode),
                    {editor(__scope, state, edit.clone())}
                }
            }
        }
    }
}

#[component]
fn editor(state: State, edit: LabelEdit) -> NodeHandle {
    match edit.mode {
        EditMode::Rename => rename(__scope, state, edit),
        EditMode::Colour => colour(__scope, state, edit),
    }
}

#[component]
fn rename(state: State, edit: LabelEdit) -> NodeHandle {
    let draft = Signal::new(
        edit.prefs
            .name
            .clone()
            .unwrap_or_else(|| edit.provider_name.clone()),
    );
    let hint = format!(
        "Only on this computer. Gmail still calls it \u{201c}{}\u{201d}.",
        edit.provider_name
    );
    let save = move || {
        let mut prefs = edit.prefs.clone();
        let name = draft.get();
        // The provider's own name is no rename at all.
        prefs.name = (name.trim() != edit.provider_name).then_some(name);
        state.save_label_prefs(&edit.account, &edit.label, &prefs);
        state.editing.set(None);
    };
    rsx! {
        div { class: "dialog",
            h2 { class: "dialog-title", "Rename label" }
            TextInput {
                value_fn: move || draft.get(),
                oninput: move |v: String| draft.set(v),
                onsubmit: save.clone(),
            }
            p { class: "dialog-hint", {hint} }
            div { class: "dialog-actions",
                Button { variant: "default", size: "sm", onclick: move || state.editing.set(None), "Cancel" }
                Button { variant: "filled", size: "sm", onclick: save, "Rename" }
            }
        }
    }
}

#[component]
fn colour(state: State, edit: LabelEdit) -> NodeHandle {
    let current = edit.prefs.color.clone();
    let choose = |color: Option<&'static str>| {
        let edit = edit.clone();
        move || {
            let mut prefs = edit.prefs.clone();
            prefs.color = color.map(str::to_owned);
            state.save_label_prefs(&edit.account, &edit.label, &prefs);
            state.editing.set(None);
        }
    };
    let swatches: Vec<NodeHandle> = SWATCHES
        .iter()
        .map(|(name, hex)| {
            let class = if current.as_deref() == Some(hex) { "swatch chosen" } else { "swatch" };
            let style = format!("background: {hex}");
            rsx! { button { class: class, style: {style.clone()}, title: *name, onclick: choose(Some(hex)) } }
        })
        .collect();
    rsx! {
        div { class: "dialog",
            h2 { class: "dialog-title", "Label colour" }
            div { class: "swatches", {swatches} }
            div { class: "dialog-actions",
                Button { variant: "default", size: "sm", onclick: choose(None), "No colour of mine" }
            }
        }
    }
}
