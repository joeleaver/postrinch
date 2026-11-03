//! Settings › Reading, after its mockup: when a conversation is marked
//! read, and what opens after archiving or trashing the open one.

use rinch::prelude::*;

use crate::reading::{AfterMove, MarkRead};
use crate::state::State;

#[component]
pub fn reading(state: State) -> NodeHandle {
    rsx! {
        main { class: "settings-main",
            h1 { class: "settings-heading", "Reading" }
            h2 { class: "settings-subheading", "Mark a conversation read" }
            div { class: "choices",
                {mark_read(__scope, state, MarkRead::OnOpen, "When I open it", "The moment it shows in the reading pane.")}
                {mark_read(__scope, state, MarkRead::AfterDelay, "After I have had it open for 3 seconds", "Stepping through the list with the arrow keys leaves mail unread.")}
                {mark_read(__scope, state, MarkRead::Manually, "Only when I say so", "Mark read with Shift+I, unread with Shift+U.")}
            }
            h2 { class: "settings-subheading", "After archiving or trashing the open conversation" }
            div { class: "choices",
                {after_move(__scope, state, AfterMove::OpenNext, "Open the next conversation", "The one below it in the list, so you can keep going.")}
                {after_move(__scope, state, AfterMove::BackToList, "Go back to the list", "Nothing opens until you choose.")}
            }
            p { class: "settings-hint",
                "Every change shows here at once and reaches Gmail in the background, in the order you made it. If you are offline, it waits and goes when you are back."
            }
        }
    }
}

#[component]
fn mark_read(state: State, which: MarkRead, name: &'static str, about: &'static str) -> NodeHandle {
    rsx! {
        div {
            class: {move || if state.mark_read.get() == which { "choice chosen" } else { "choice" }},
            onclick: move || state.set_mark_read(which),
            span { class: "choice-name", {name} }
            span { class: "choice-about", {about} }
        }
    }
}

#[component]
fn after_move(
    state: State,
    which: AfterMove,
    name: &'static str,
    about: &'static str,
) -> NodeHandle {
    rsx! {
        div {
            class: {move || if state.after_move.get() == which { "choice chosen" } else { "choice" }},
            onclick: move || state.set_after_move(which),
            span { class: "choice-name", {name} }
            span { class: "choice-about", {about} }
        }
    }
}
