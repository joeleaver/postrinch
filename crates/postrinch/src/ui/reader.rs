//! The reading pane: the open thread, every message in it.

use jiff::tz::TimeZone;
use postrinch_commands::{Rendered, Start, Thread};
use postrinch_model::Message;
use rinch::prelude::*;

use crate::dates;
use crate::pictures::open_in_browser;
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
        .map(|m| message(__scope, state, m.clone(), dates::long(m.received, &tz)))
        .collect();
    rsx! {
        div { class: "thread",
            div { class: "thread-head",
                h2 { class: "thread-subject", {subject} }
                {toolbar(__scope, state)}
            }
            div { class: "thread-meta", {meta} }
            {messages}
            {answer_bar(__scope, state, thread.clone())}
        }
    }
}

#[component]
fn message(state: State, message: Message, when: String) -> NodeHandle {
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
    let id = message.id.0.clone();
    let shown = message.clone();
    let body = if message.downloaded {
        let text = message.body_text.trim().to_owned();
        // As it renders, once the loader has rendered it; its text until then.
        let unrendered = id.clone();
        rsx! {
            div {
                for rendered in state.rendered.get().get(&id).cloned() {
                    div { key: format!("{}|{}", rendered.remote_hidden, rendered.html.len()),
                        {mail(__scope, state, shown.clone(), rendered.clone())}
                    }
                }
                if !state.rendered.get().contains_key(&unrendered) {
                    {plain(__scope, text.clone())}
                }
            }
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

/// A message as Postrinch drew it: the hidden-pictures bar when pictures
/// from the web were held back, the mail itself, and a way out to the
/// browser.
#[component]
fn mail(state: State, message: Message, rendered: Rendered) -> NodeHandle {
    // The mail's own body (its style, its contents) inside the white card,
    // so a body with no padding of its own still sits off the card's edge.
    // A click on a link in it opens the link (crate::links).
    let (html, body_style) = (rendered.html.clone(), rendered.body_style.clone());
    let host = rsx! {
        div { class: "mail-html",
            HtmlView { html: html, body_style: body_style, onlink: move |href: String| crate::links::open(&href) }
        }
    };
    let hidden = rendered.remote_hidden;
    let line = match hidden {
        1 => "A picture from the web is hidden, so the sender can\u{2019}t tell you opened this."
            .to_owned(),
        n => format!(
            "{n} pictures from the web are hidden, so the sender can\u{2019}t tell you opened this."
        ),
    };
    let (once, always) = (message.clone(), message.clone());
    let bar = (hidden > 0).then(|| {
        rsx! {
            div { class: "pictures-bar", role: "status",
                span { class: "pictures-line", {line.clone()} }
                Button { variant: "default", size: "xs", onclick: move || state.show_pictures(&once), "Show pictures" }
                Button { variant: "default", size: "xs", onclick: move || state.always_show_pictures(&always), "Always from this sender" }
            }
        }
    });
    let foot = rendered.from_html.then(|| {
        let (message, rendered) = (message.clone(), rendered.clone());
        rsx! {
            div { class: "mail-foot",
                span { "Shown as Postrinch draws it: scripts, forms and frames are left out." }
                span { class: "mail-open", onclick: move || open_in_browser(&message, &rendered), "Open in browser" }
            }
        }
    });
    rsx! {
        div { class: "mail",
            {bar}
            {host}
            {foot}
        }
    }
}

/// A message's text, before it is rendered or when it cannot be.
#[component]
fn plain(text: String) -> NodeHandle {
    if text.is_empty() {
        rsx! { div { class: "message-note", "This message has no text." } }
    } else {
        rsx! { div { class: "message-body", {text.clone()} } }
    }
}

/// Answering the conversation, after the main mockup: Reply, Reply all and
/// Forward; Edit draft when its latest message is one of the account's
/// drafts.
#[component]
fn answer_bar(state: State, thread: Thread) -> NodeHandle {
    let draft = thread.messages.last().cloned().filter(|m| {
        crate::actions::commands().is_some_and(|c| c.is_draft(&m.account, m).unwrap_or(false))
    });
    let edit = draft.map(|message| {
        rsx! {
            Button { variant: "filled", size: "sm", onclick: move || state.edit_draft(&message), "Edit draft" }
        }
    });
    rsx! {
        div { class: "answer-bar",
            {edit}
            Button { variant: "default", size: "sm", onclick: move || state.write(Start::Reply), "Reply" }
            Button { variant: "default", size: "sm", onclick: move || state.write(Start::ReplyAll), "Reply all" }
            Button { variant: "default", size: "sm", onclick: move || state.write(Start::Forward), "Forward" }
        }
    }
}
