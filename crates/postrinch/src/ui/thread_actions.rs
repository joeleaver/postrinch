//! The open conversation's actions, after the Main mockup's reader header:
//! archive (or back to the inbox), trash (or restore), mark unread, then
//! the label menu (the LabelMenu mockup) and the star.

use postrinch_commands::{Folder, LabelChoice, ThreadAction};
use postrinch_model::{EmailAddress, SystemLabel, ThreadSummary};
use rinch::prelude::*;

use crate::changes::patch;
use crate::state::State;
use crate::ui::icons::{Icon, icon};

/// The open conversation's summary, with pending actions on it applied.
pub fn open_summary(state: State) -> Option<ThreadSummary> {
    let mut summary = state.thread.get()?.summary;
    let key = (summary.account.clone(), summary.id.clone());
    state.pending.with(|p| {
        for change in p.iter().filter(|c| c.threads.contains(&key)) {
            patch(&mut summary, &change.action);
        }
    });
    Some(summary)
}

/// The provider's id for an account's inbox, from the sidebar.
fn inbox_of(state: State, account: &EmailAddress) -> Option<String> {
    state.sidebar.with(|s| {
        let found = s
            .as_ref()?
            .accounts
            .iter()
            .find(|a| a.account.address == *account)?;
        match &found.inbox.folder {
            Folder::Label { label, .. } => Some(label.clone()),
            Folder::Everywhere(_) => None,
        }
    })
}

/// Whether the open conversation is in its account's inbox.
pub fn in_inbox(state: State) -> bool {
    open_summary(state)
        .is_some_and(|s| inbox_of(state, &s.account).is_some_and(|inbox| s.labels.contains(&inbox)))
}

/// Whether the person is looking at the trash.
fn in_trash(state: State) -> bool {
    state.folder.get() == Folder::Everywhere(SystemLabel::Trash)
}

/// Archive, or back to the inbox for a conversation out of it: what `e`
/// and the first button do.
pub fn archive_or_restore(state: State) -> ThreadAction {
    if in_inbox(state) {
        ThreadAction::Archive
    } else {
        ThreadAction::MoveToInbox
    }
}

/// Trash, or out of the trash while looking at it: what `#` and the
/// second button do.
pub fn trash_or_restore(state: State) -> ThreadAction {
    if in_trash(state) {
        ThreadAction::Untrash
    } else {
        ThreadAction::Trash
    }
}

/// Mark read while the conversation has unread mail, else mark unread:
/// what the envelope button does, as in Gmail.
pub fn read_or_unread(state: State) -> ThreadAction {
    if open_summary(state).is_some_and(|s| s.unread_count > 0) {
        ThreadAction::MarkRead
    } else {
        ThreadAction::MarkUnread
    }
}

/// Star, or unstar a starred conversation: what `s` and the star do.
pub fn star_or_unstar(state: State) -> ThreadAction {
    if open_summary(state).is_some_and(|s| s.starred) {
        ThreadAction::Unstar
    } else {
        ThreadAction::Star
    }
}

#[component]
pub fn toolbar(state: State) -> NodeHandle {
    rsx! {
        div { class: "thread-actions", role: "toolbar", aria-label: "Conversation actions",
            {action_button(__scope, state, Kind::Archive)}
            {action_button(__scope, state, Kind::Trash)}
            {action_button(__scope, state, Kind::Unread)}
            span { class: "thread-actions-gap" }
            {label_menu(__scope, state)}
            {action_button(__scope, state, Kind::Star)}
        }
    }
}

/// The buttons that do one action, which may depend on the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Archive,
    Trash,
    Unread,
    Star,
}

impl Kind {
    fn action(self, state: State) -> ThreadAction {
        match self {
            Kind::Archive => archive_or_restore(state),
            Kind::Trash => trash_or_restore(state),
            Kind::Unread => read_or_unread(state),
            Kind::Star => star_or_unstar(state),
        }
    }

    /// What the button says (its tooltip) for `action`.
    fn title(action: &ThreadAction) -> &'static str {
        match action {
            ThreadAction::Archive => "Archive (E)",
            ThreadAction::MoveToInbox => "Move to inbox (E)",
            ThreadAction::Trash => "Move to trash (#)",
            ThreadAction::Untrash => "Restore from trash (#)",
            ThreadAction::MarkUnread => "Mark unread (Shift+U)",
            ThreadAction::MarkRead => "Mark read (Shift+I)",
            ThreadAction::Star => "Star (S)",
            ThreadAction::Unstar => "Unstar (S)",
            _ => "",
        }
    }

    fn icon(action: &ThreadAction) -> Icon {
        match action {
            ThreadAction::Archive => Icon::Archive,
            ThreadAction::MoveToInbox => Icon::Inbox,
            ThreadAction::Trash => Icon::Trash,
            ThreadAction::Untrash => Icon::Restore,
            ThreadAction::Unstar => Icon::Starred,
            ThreadAction::Star => Icon::Star,
            ThreadAction::MarkRead => Icon::Read,
            _ => Icon::Unread,
        }
    }
}

#[component]
fn action_button(state: State, kind: Kind) -> NodeHandle {
    let action = Memo::new(move || kind.action(state));
    rsx! {
        button {
            class: {move || match action.get() {
                ThreadAction::Unstar => "icon-button starred",
                _ => "icon-button",
            }},
            title: {move || Kind::title(&action.get())},
            aria-label: {move || Kind::title(&action.get())},
            onclick: move || state.act_on_open(action.get()),
            // Keyed on the icon, so it is drawn again only when it changes.
            for which in [Kind::icon(&action.get())] {
                span { key: format!("{which:?}"), class: "icon-slot", {icon(__scope, which)} }
            }
        }
    }
}

/// The label button and its menu: every label of the conversation's
/// account, ticked where the conversation has it, with a filter.
#[component]
fn label_menu(state: State) -> NodeHandle {
    let filter = Signal::new(String::new());
    rsx! {
        Popover {
            opened_fn: move || state.labelling.get(),
            onclose: move || {
                state.labelling.set(false);
                filter.set(String::new());
            },
            position: "bottom-end",
            width: "280px",
            PopoverTarget {
                button {
                    class: {move || if state.labelling.get() { "icon-button open" } else { "icon-button" }},
                    title: "Label (L)",
                    aria-label: "Label",
                    aria-haspopup: "menu",
                    onclick: move || state.labelling.update(|open| *open = !*open),
                    {icon(__scope, Icon::Label)}
                }
            }
            PopoverDropdown {
                div { class: "label-menu",
                    input {
                        class: "label-filter",
                        placeholder: "Find a label",
                        aria-label: "Find a label",
                        value: {move || filter.get()},
                        oninput: move |value: String| filter.set(value),
                    }
                    div { class: "label-choices",
                        for choice in matching(state, &filter.get()) {
                            div { key: format!("{choice:?}"),
                                {label_choice(__scope, state, choice.clone())}
                            }
                        }
                        if matching(state, &filter.get()).is_empty() {
                            div { class: "label-none", "No label matches." }
                        }
                    }
                    div { class: "label-menu-foot", "Applies to the whole conversation." }
                }
            }
        }
    }
}

/// The account's labels whose names contain `filter`, ignoring case.
fn matching(state: State, filter: &str) -> Vec<LabelChoice> {
    let filter = filter.trim().to_lowercase();
    state.label_choices.with(|choices| {
        choices
            .iter()
            .filter(|c| c.name.to_lowercase().contains(&filter))
            .cloned()
            .collect()
    })
}

#[component]
fn label_choice(state: State, choice: LabelChoice) -> NodeHandle {
    let label = choice.label.clone();
    let has = Memo::new(move || open_summary(state).is_some_and(|s| s.labels.contains(&label)));
    let toggle = {
        let label = choice.label.clone();
        move || {
            let action = if has.get() {
                ThreadAction::RemoveLabel(label.clone())
            } else {
                ThreadAction::AddLabel(label.clone())
            };
            state.act_on_open(action);
        }
    };
    let dot = choice
        .color
        .clone()
        .map(|c| format!("background: {c}"))
        .unwrap_or_else(|| "background: var(--muted)".into());
    rsx! {
        button {
            class: "label-choice",
            role: "menuitemcheckbox",
            aria-checked: {move || if has.get() { "true" } else { "false" }},
            onclick: toggle,
            span { class: {move || if has.get() { "label-box ticked" } else { "label-box" }},
                svg {
                    xmlns: "http://www.w3.org/2000/svg",
                    viewBox: "0 0 24 24",
                    width: "12",
                    height: "12",
                    fill: "none",
                    stroke: "currentColor",
                    stroke-width: "3",
                    stroke-linecap: "round",
                    stroke-linejoin: "round",
                    aria-hidden: "true",
                    path { d: "M5 12l5 5 9-10" }
                }
            }
            span { class: "dot", style: {dot.clone()} }
            span { class: "label-choice-name", {choice.name.clone()} }
        }
    }
}

/// The toolbar's and the label menu's styles.
pub const CSS: &str = r#"
.thread-actions { display: flex; align-items: center; gap: 4px; flex-shrink: 0; }
.thread-actions-gap { width: 1px; height: 22px; margin: 0 4px; background: var(--line); }
.icon-button {
    width: 34px; height: 34px;
    display: flex; align-items: center; justify-content: center;
    border: 1px solid var(--field);
    border-radius: 6px;
    background: var(--surface);
    color: var(--ink);
    cursor: pointer;
}
.icon-button:hover { background: var(--hover); }
.icon-button.open { border-color: var(--accent); background: var(--accent-tint); color: var(--accent); }
.icon-button.starred { color: #d99a06; }
.icon-slot { display: flex; }
.label-menu { display: flex; flex-direction: column; }
.label-filter {
    margin: 10px;
    height: 32px;
    padding: 0 10px;
    border: 1px solid var(--field);
    border-radius: 6px;
    background: var(--surface);
    color: var(--ink);
    font: inherit;
}
.label-filter:focus { border-color: var(--accent); outline: none; }
.label-choices {
    display: flex; flex-direction: column;
    padding: 0 6px 6px;
    max-height: 250px;
    overflow-y: auto;
}
.label-choice {
    display: flex; align-items: center; gap: 10px;
    height: 34px; padding: 0 8px;
    border: 0; border-radius: 6px;
    background: transparent;
    color: var(--ink);
    font: inherit;
    text-align: left;
    cursor: pointer;
}
.label-choice:hover { background: var(--row-hover); }
.label-box {
    width: 16px; height: 16px; flex-shrink: 0;
    display: flex; align-items: center; justify-content: center;
    border: 1px solid var(--field);
    border-radius: 4px;
    color: transparent;
}
.label-box.ticked { background: var(--accent); border-color: var(--accent); color: #ffffff; }
.label-choice-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.label-none { padding: 8px; color: var(--muted); }
.label-menu-foot { padding: 8px 12px; border-top: 1px solid var(--line-soft); font-size: 12px; color: var(--muted); }
"#;
