//! The reading pane: the open conversation, after the conversations
//! mockups. Its actions sit in one bar at the top left, above the subject;
//! its messages show newest or oldest first, as set, and those the setting
//! does not open in full fold to one line.

use jiff::tz::TimeZone;
use postrinch_commands::{Rendered, Start, Thread};
use postrinch_model::{Message, ProviderMessageId};
use rinch::prelude::*;

use crate::conversations::in_order;
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
            for (thread, order) in state.thread.get().map(|t| (t, state.order.get())) {
                div { key: format!("{}|{order:?}", thread_key(thread)), class: "reader-frame",
                    {open_thread(__scope, state, thread.clone(), order)}
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
fn open_thread(state: State, thread: Thread, order: crate::conversations::Order) -> NodeHandle {
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
    let shown = in_order(order, &thread);
    let folded = {
        let shown = std::rc::Rc::new(shown.clone());
        move || shown.iter().any(|m| !state.is_open(m))
    };
    let messages: Vec<NodeHandle> = shown
        .iter()
        .map(|m| {
            let when = dates::long(m.received, &tz);
            let (full, line, open) = (m.clone(), m.clone(), m.clone());
            rsx! {
                div {
                    if state.is_open(&open) {
                        {message(__scope, state, full.clone(), when.clone())}
                    } else {
                        {folded_message(__scope, state, line.clone(), when.clone())}
                    }
                }
            }
        })
        .collect();
    rsx! {
        div { class: "reader-frame",
            {action_bar(__scope, state, thread.clone())}
            div { class: "reader-scroll",
            div { class: "thread",
            h2 { class: "thread-subject", {subject} }
            div { class: "thread-meta",
                {meta}
                span {
                    class: "open-all",
                    style: {let folded = folded.clone(); move || if folded() { "" } else { "display: none" }},
                    onclick: move || state.open_all(),
                    "Open all"
                }
            }
            {messages}
            }
            }
        }
    }
}

/// A message folded to one line: who, how it starts, when. A click opens
/// it in full.
#[component]
fn folded_message(state: State, message: Message, when: String) -> NodeHandle {
    let name = message
        .headers
        .from
        .first()
        .map_or_else(|| "(unknown sender)".to_owned(), |from| from.display_name());
    let snippet = message.snippet.clone();
    rsx! {
        div {
            class: "message-folded",
            role: "button",
            title: "Open this message",
            onclick: move || state.open_message(&message),
            span { class: "folded-from", {name} }
            span { class: "folded-snippet", {snippet} }
            span { class: "message-when", {when} }
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
            {answer_links(__scope, state, message.id.clone())}
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
    // HTML mail is cleaned by the view at the card's width, and again when a
    // resize crosses one of its `@media` breakpoints (a narrow pane gets what
    // a phone shows); text mail is drawn as rendered. A click on a link opens
    // it (crate::links).
    let view = if rendered.document.is_empty() {
        let (html, body_style) = (rendered.html.clone(), rendered.body_style.clone());
        rsx! { HtmlView { html: html, body_style: body_style, onlink: move |href: String| crate::links::open(&href) } }
    } else {
        let document = rendered.document.clone();
        let policy = postrinch_render::policy(rendered.remote);
        rsx! { HtmlView { document: document, policy: policy, onlink: move |href: String| crate::links::open(&href) } }
    };
    let host = rsx! { div { class: "mail-html", {view} } };
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

/// The conversation's actions, at the top left: Edit draft when its latest
/// message is one of the account's drafts, Reply, Reply all and Forward,
/// then what the toolbar does to the conversation.
#[component]
fn action_bar(state: State, thread: Thread) -> NodeHandle {
    let draft = thread.messages.last().cloned().filter(|m| {
        crate::actions::commands().is_some_and(|c| c.is_draft(&m.account, m).unwrap_or(false))
    });
    let edit = draft.map(|message| {
        rsx! {
            Button { variant: "filled", size: "sm", onclick: move || state.edit_draft(&message), "Edit draft" }
        }
    });
    rsx! {
        div { class: "thread-bar",
            {edit}
            Button { variant: "default", size: "sm", onclick: move || state.write(Start::Reply), "Reply" }
            Button { variant: "default", size: "sm", onclick: move || state.write(Start::ReplyAll), "Reply all" }
            Button { variant: "default", size: "sm", onclick: move || state.write(Start::Forward), "Forward" }
            span { class: "thread-actions-gap" }
            {toolbar(__scope, state)}
        }
    }
}

/// Answering one message of the conversation, under it.
#[component]
fn answer_links(state: State, message: ProviderMessageId) -> NodeHandle {
    let (reply, all, forward) = (message.clone(), message.clone(), message);
    rsx! {
        div { class: "answer-links",
            span { class: "answer-link", onclick: move || state.answer(Start::Reply, Some(&reply)), "Reply" }
            span { class: "answer-link", onclick: move || state.answer(Start::ReplyAll, Some(&all)), "Reply all" }
            span { class: "answer-link", onclick: move || state.answer(Start::Forward, Some(&forward)), "Forward" }
        }
    }
}
