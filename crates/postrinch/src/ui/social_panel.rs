//! The social view panel, after the "Edit a view" state of the Social
//! mockup: opened by ✎ on a view's row (or "New view…"), over the view
//! beside the sidebar. Its name, its places (social_panel_places.rs), which
//! kinds it shows, a filter, how it reads, and its count; notices are the
//! places' own, changed on their chips. Delete view on the left, Cancel
//! and Save on the right.

use postrinch_store::{ReadAs, SocialCount};
use rinch::prelude::*;

use crate::social_edit::{KINDS, Removal, SocialEdit};
use crate::social_shown::built_in_holds;
use crate::state::State;

/// The panel's draft, read by `f`.
pub(crate) fn read<T: Default>(state: State, f: impl FnOnce(&SocialEdit) -> T) -> T {
    state
        .social
        .edit
        .with(|e| e.as_ref().map(f).unwrap_or_default())
}

/// The panel, while it is open.
#[component]
pub fn social_panel(state: State) -> NodeHandle {
    // Keyed on the view, so the fields start from it.
    let key = Memo::new(move || {
        state.social.edit.with(|e| {
            e.as_ref().map(|e| {
                if e.is_new() {
                    String::new()
                } else {
                    e.before.id.clone()
                }
            })
        })
    });
    rsx! {
        section { class: "social-panel", aria-label: "View",
            for key in key.get() {
                div { key: key.clone(), class: "social-panel-inner",
                    {body(__scope, state)}
                }
            }
        }
    }
}

#[component]
fn body(state: State) -> NodeHandle {
    let built_in = read(state, |e| e.built_in);
    let filters = read(state, |e| Some(e.filters_apply())).unwrap_or(true);
    let title = if read(state, SocialEdit::is_new) {
        "New view"
    } else {
        "Edit view"
    };
    let places = match built_in {
        Some(kind) => rsx! {
            div { class: "social-field",
                span { class: "social-label", "Places" }
                p { class: "social-holds", {built_in_holds(kind)} }
                p { class: "social-hint", "Built in, so the places it shows stay as they are. Make a view of your own to choose places." }
            }
        },
        None => crate::ui::social_panel_places::places_field(__scope, state),
    };
    let shows = filters.then(|| kinds_field(__scope, state));
    let only = filters.then(|| {
        rsx! {
            div { class: "social-field",
                span { class: "social-label", "Only" }
                TextInput {
                    spellcheck: false,
                    placeholder: "From, words, tags: from:mira tag:meetup",
                    value_fn: move || read(state, |e| e.draft.filters.search.clone()),
                    oninput: move |v: String| state.change_social_edit(|e| e.draft.filters.search = v),
                }
            }
        }
    });
    let notices = built_in.is_none().then(|| {
        rsx! {
            div { class: "social-field",
                span { class: "social-label", "Notices" }
                p { class: "social-hint flush", "Set on each channel or group, not on the view, so they match wherever the place appears. Change one with \u{1f514} \u{25be} on its chip." }
            }
        }
    });
    rsx! {
        div { class: "social-panel-parts",
            div { class: "social-panel-body",
                h3 { class: "social-panel-title", {title} }
                div { class: "social-field",
                    span { class: "social-label", "Name" }
                    TextInput {
                        spellcheck: false,
                        value_fn: move || read(state, |e| e.draft.name.clone()),
                        oninput: move |v: String| state.change_social_edit(|e| e.draft.name = v),
                        onsubmit: move || state.save_social_edit(),
                    }
                }
                {places}
                {shows}
                {only}
                {read_as_field(__scope, state)}
                {notices}
                div { class: "social-field",
                    span { class: "social-label", "Count" }
                    Select {
                        data: vec![
                            SelectOption::new("unread", "Unread"),
                            SelectOption::new("mentions", "Unread mentions"),
                            SelectOption::new("none", "None"),
                        ],
                        value_fn: move || count_word(read(state, |e| e.draft.count)).to_owned(),
                        onchange: move |v: String| {
                            if let Some(count) = count_of(&v) {
                                state.change_social_edit(|e| e.draft.count = count);
                            }
                        },
                    }
                }
            }
            // A keyed loop, not `if let` (rinch#2105).
            for error in read(state, |e| e.error.clone()) {
                p { key: error.clone(), class: "social-trouble", {error.clone()} }
            }
            {foot(__scope, state)}
        }
    }
}

/// Show: which kinds the view shows.
fn kinds_field(__scope: &mut RenderScope, state: State) -> NodeHandle {
    let boxes: Vec<NodeHandle> = KINDS
        .into_iter()
        .map(|kind| {
            let name = match kind {
                postrinch_store::ContentKind::Chat => "Chat",
                postrinch_store::ContentKind::Memo => "Memos",
                postrinch_store::ContentKind::Article => "Articles",
            };
            rsx! {
                Checkbox {
                    size: "sm",
                    label: name,
                    checked_fn: move || read(state, |e| e.shows(kind)),
                    onchange: move || state.change_social_edit(|e| e.toggle_kind(kind)),
                }
            }
        })
        .collect();
    rsx! {
        div { class: "social-field",
            span { class: "social-label", "Show" }
            div { class: "social-checks", {boxes} }
        }
    }
}

/// Read as: a stream, or a list with a reader.
fn read_as_field(__scope: &mut RenderScope, state: State) -> NodeHandle {
    let segment = |__scope: &mut RenderScope, value: ReadAs, name: &'static str| {
        let on = move || read(state, |e| Some(e.draft.read_as)) == Some(value);
        rsx! {
            button {
                class: {move || if on() { "on" } else { "" }},
                role: "radio",
                aria-checked: {move || if on() { "true" } else { "false" }},
                onclick: move || state.change_social_edit(|e| e.draft.read_as = value),
                {name}
            }
        }
    };
    let stream = segment(__scope, ReadAs::Stream, "Stream");
    let list = segment(__scope, ReadAs::List, "List and reader");
    rsx! {
        div { class: "social-field",
            span { class: "social-label", "Read as" }
            div { class: "social-seg", role: "radiogroup", aria-label: "Read as", {stream} {list} }
        }
    }
}

/// Delete view (or put back) on the left, asking first when it can't be
/// undone; Cancel, then Save, on the right.
#[component]
fn foot(state: State) -> NodeHandle {
    let removal = Memo::new(move || read(state, SocialEdit::removal));
    let confirming = Signal::new(false);
    let can_save = Memo::new(move || read(state, SocialEdit::can_save));
    let name = move || read(state, |e| e.before.name.clone());
    rsx! {
        div { class: "social-actions",
            button {
                class: "social-btn red",
                style: {move || if removal.get().is_some() && !confirming.get() { "" } else { "display: none" }},
                onclick: move || match removal.get() {
                    Some(Removal::Delete) => confirming.set(true),
                    _ => state.remove_social_edit(),
                },
                {move || removal.get().map(Removal::label).unwrap_or_default()}
            }
            div {
                class: "social-confirm",
                style: {move || if confirming.get() { "" } else { "display: none" }},
                span { {move || format!("Delete \u{201c}{}\u{201d}? This can\u{2019}t be undone.", name())} }
                button { class: "social-btn", onclick: move || confirming.set(false), "Keep it" }
                button { class: "social-btn danger", onclick: move || state.remove_social_edit(), "Delete" }
            }
            span { class: "social-spacer" }
            div {
                style: {move || if confirming.get() { "display: none" } else { "display: contents" }},
                button { class: "social-btn", onclick: move || state.cancel_social_edit(), "Cancel" }
                button {
                    class: "social-btn filled",
                    disabled: {move || !can_save.get()},
                    title: {move || if can_save.get() { "" } else { "Give it a name to save it." }},
                    onclick: move || if can_save.get() { state.save_social_edit() },
                    "Save"
                }
            }
        }
    }
}

fn count_word(count: SocialCount) -> &'static str {
    match count {
        SocialCount::Unread => "unread",
        SocialCount::Mentions => "mentions",
        SocialCount::None => "none",
    }
}

fn count_of(word: &str) -> Option<SocialCount> {
    [
        SocialCount::Unread,
        SocialCount::Mentions,
        SocialCount::None,
    ]
    .into_iter()
    .find(|&c| count_word(c) == word)
}

pub const CSS: &str = r#"
.social-panel {
    position: absolute; top: 0; bottom: 0; left: 0; width: 400px; z-index: 2;
    display: flex; flex-direction: column;
    background: var(--surface); border-right: 1px solid var(--line);
    box-shadow: 8px 0 24px rgba(0, 0, 0, 0.12);
}
.social-panel-inner, .social-panel-parts { flex: 1; min-height: 0; display: flex; flex-direction: column; }
.social-panel-body { flex: 1; min-height: 0; overflow-y: auto; padding: 16px 18px 4px; display: flex; flex-direction: column; }
.social-panel-title { margin: 0 0 12px; font-size: 16px; font-weight: 700; }
.social-field { display: flex; flex-direction: column; gap: 4px; margin-bottom: 12px; }
.social-label { font-size: 12px; color: var(--muted); font-weight: 600; }
.social-hint { margin: 4px 0 0; color: var(--muted); font-size: 12.5px; }
.social-hint.flush { margin: 0; }
.social-holds { margin: 0; font-size: 13px; color: var(--ink); }
.social-checks { display: flex; gap: 12px; flex-wrap: wrap; font-size: 13px; }
.social-seg { display: flex; border: 1px solid var(--field); border-radius: 6px; overflow: hidden; }
.social-seg button {
    flex: 1; padding: 5px 0; border: 0; background: transparent; text-align: center;
    color: var(--ink); font: inherit; font-size: 13px; cursor: pointer;
}
.social-seg button + button { border-left: 1px solid var(--field); }
.social-seg button:hover { background: var(--hover); }
.social-seg button.on { background: var(--accent-tint); color: var(--accent-ink); font-weight: 600; }
.social-trouble { margin: 0; padding: 8px 18px; background: var(--warn-bg); color: var(--trouble); font-size: 13px; border-top: 1px solid var(--warn-line); }
.social-actions { display: flex; align-items: center; gap: 8px; padding: 12px 18px 16px; }
.social-spacer { flex: 1; }
.social-confirm { display: flex; align-items: center; gap: 8px; font-size: 13px; color: var(--ink-2); }
.social-btn {
    padding: 5px 12px; border: 1px solid var(--field); border-radius: 6px;
    background: var(--surface); color: var(--ink); font: inherit; font-size: 13px; cursor: pointer;
}
.social-btn:hover { background: var(--hover); }
.social-btn.filled { background: var(--accent); border-color: var(--accent); color: #fff; }
.social-btn.filled:disabled { opacity: 0.5; cursor: default; }
.social-btn.red { color: var(--danger); border-color: var(--danger); }
.social-btn.danger { background: var(--danger); border-color: var(--danger); color: #fff; }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_count_has_its_own_word_and_reads_back() {
        for count in [
            SocialCount::Unread,
            SocialCount::Mentions,
            SocialCount::None,
        ] {
            assert_eq!(count_of(count_word(count)), Some(count));
        }
        assert_eq!(count_of("all"), None);
    }
}
