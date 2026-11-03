//! The reading pane: the open thread, every message in it.

use jiff::tz::TimeZone;
use postrinch_commands::Thread;
use postrinch_model::Message;
use rinch::prelude::*;

use crate::dates;
use crate::state::State;
use crate::ui::thread_actions::toolbar;

#[component]
pub fn reader(state: State) -> NodeHandle {
    rsx! {
        article { class: "reader",
            // A keyed loop over the Option, not `if let`, so another thread
            // (or this one, changed) is drawn again (rinch#2105).
            for thread in state.thread.get() {
                div { key: thread_key(thread),
                    {open_thread(__scope, state, thread.clone())}
                }
            }
            if state.thread.get().is_none() {
                div { class: "reader-empty", "Choose a conversation to read it." }
            }
        }
    }
}

/// Changes whenever the thread, or anything shown of it, does.
fn thread_key(thread: &Thread) -> String {
    let s = &thread.summary;
    let downloaded = thread.messages.iter().filter(|m| m.downloaded).count();
    format!(
        "{}|{}|{}|{}|{downloaded}",
        s.account.key(),
        s.id.0,
        s.message_count,
        s.unread_count
    )
}

#[component]
fn open_thread(state: State, thread: Thread) -> NodeHandle {
    let tz = TimeZone::system();
    let summary = &thread.summary;
    let count = match summary.message_count {
        1 => "1 message".to_owned(),
        n => format!("{n} messages"),
    };
    let meta = format!("{count} · {}", summary.account);
    let subject = if summary.subject.trim().is_empty() {
        "(no subject)".to_owned()
    } else {
        summary.subject.clone()
    };
    let messages: Vec<NodeHandle> = thread
        .messages
        .iter()
        .map(|m| message(__scope, m.clone(), dates::long(m.received, &tz)))
        .collect();
    rsx! {
        div { class: "thread",
            div { class: "thread-head",
                h2 { class: "thread-subject", {subject} }
                {toolbar(__scope, state)}
            }
            div { class: "thread-meta", {meta} }
            {messages}
        }
    }
}

#[component]
fn message(message: Message, when: String) -> NodeHandle {
    let (name, address) = match message.headers.from.first() {
        Some(from) => (from.display_name(), from.address.to_string()),
        None => ("(unknown sender)".to_owned(), String::new()),
    };
    let to: Vec<String> = message
        .headers
        .to
        .iter()
        .chain(&message.headers.cc)
        .map(|m| m.display_name())
        .collect();
    let to = (!to.is_empty()).then(|| format!("to {}", to.join(", ")));
    let to = to.map(|line| rsx! { div { class: "message-to", {line} } });
    let body = if message.downloaded {
        let text = message.body_text.trim().to_owned();
        if text.is_empty() {
            rsx! { div { class: "message-note", "This message has no text." } }
        } else {
            rsx! { div { class: "message-body", {text} } }
        }
    } else {
        rsx! {
            div { class: "message-note",
                "Downloading this message…"
            }
        }
    };
    let attachments: Vec<NodeHandle> = message
        .attachments
        .iter()
        .filter(|a| !a.inline)
        .map(|a| {
            let name = a.filename.clone().unwrap_or_else(|| a.mime_type.clone());
            rsx! { span { class: "chip", {format!("📎 {name}")} } }
        })
        .collect();
    rsx! {
        div { class: "message",
            div { class: "message-head",
                span { class: "message-from", {name} }
                span { class: "message-address", {address} }
                span { class: "message-when", {when} }
            }
            {to}
            {body}
            div { class: "attachments", {attachments} }
        }
    }
}
