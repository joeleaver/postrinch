//! Settings › Reading, after its mockups: conversations or messages, which
//! end of a conversation is at the top and what opens in full, when a
//! conversation is marked read, and what opens after archiving or trashing
//! the open one.

use rinch::prelude::*;

use crate::conversations::{OpenInFull, Order, ShowMail};
use crate::reading::{AfterMove, MarkRead};
use crate::state::State;

#[component]
pub fn reading(state: State) -> NodeHandle {
    rsx! {
        main { class: "settings-main",
            h1 { class: "settings-heading", "Reading" }
            h2 { class: "settings-subheading", "Show mail" }
            div { class: "choices",
                {show_mail(__scope, state, ShowMail::Conversations, "As conversations", "A message and its replies are one row in the list, read together.")}
                {show_mail(__scope, state, ShowMail::Messages, "One message at a time", "Every message is its own row, newest first. Archiving or labelling one leaves the rest of its conversation as it is.")}
            }
            h2 { class: "settings-subheading", "In a conversation, show" }
            div { class: "choices",
                {order(__scope, state, Order::NewestFirst, "The newest message at the top", "What just arrived is the first thing you see.")}
                {order(__scope, state, Order::OldestFirst, "The oldest message at the top", "Read it in the order it was written, as Gmail does.")}
            }
            h2 { class: "settings-subheading", "Open in full" }
            div { class: "choices",
                {open_in_full(__scope, state, OpenInFull::UnreadAndNewest, "Unread messages and the newest", "The others fold to one line: who, when, and how it starts. Click one to open it.")}
                {open_in_full(__scope, state, OpenInFull::Newest, "Only the newest", "Everything before it folds, unread or not.")}
                {open_in_full(__scope, state, OpenInFull::All, "Every message", "Nothing folds.")}
            }
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

#[component]
fn show_mail(state: State, which: ShowMail, name: &'static str, about: &'static str) -> NodeHandle {
    rsx! {
        div {
            class: {move || if state.show_mail.get() == which { "choice chosen" } else { "choice" }},
            onclick: move || state.set_show_mail(which),
            span { class: "choice-name", {name} }
            span { class: "choice-about", {about} }
        }
    }
}

#[component]
fn order(state: State, which: Order, name: &'static str, about: &'static str) -> NodeHandle {
    rsx! {
        div {
            class: {move || if state.order.get() == which { "choice chosen" } else { "choice" }},
            onclick: move || state.set_order(which),
            span { class: "choice-name", {name} }
            span { class: "choice-about", {about} }
        }
    }
}

#[component]
fn open_in_full(
    state: State,
    which: OpenInFull,
    name: &'static str,
    about: &'static str,
) -> NodeHandle {
    rsx! {
        div {
            class: {move || if state.open_in_full.get() == which { "choice chosen" } else { "choice" }},
            onclick: move || state.set_open_in_full(which),
            span { class: "choice-name", {name} }
            span { class: "choice-about", {about} }
        }
    }
}
