//! The reading pane: the open conversation, after the conversations and
//! list header mockups. Its subject is at the top; what can be done to the
//! conversation is above the list, and each message is answered from the
//! icons in its own header. Its messages show newest or oldest first, as
//! set, and those the setting does not open in full fold to one line.

use jiff::tz::TimeZone;
use postrinch_commands::{Rendered, Start, Thread};
use postrinch_model::{Message, ProviderMessageId};
use rinch::prelude::*;

use crate::conversations::in_order;
use crate::dates;
use crate::state::State;
use crate::ui::icons::{Icon, icon};

#[component]
pub fn reader(state: State) -> NodeHandle {
    rsx! {
        article { class: "reader",
            // A keyed loop over the Option, not `if let`, so another thread
            // (or this one, changed) is drawn again (rinch#2105).
            for (thread, order, only) in state.thread.get().map(|t| (t, state.order.get(), alone(state))) {
                div { key: format!("{}|{order:?}|{only:?}", thread_key(thread)), class: "reader-frame",
                    {open_thread(__scope, state, thread.clone(), order, only.clone())}
                }
            }
            if state.thread.get().is_none() {
                div { class: "reader-empty", "Choose a conversation to read it." }
            }
        }
    }
}

/// The message shown alone, when mail is read one message at a time and
/// the person has not asked for its whole conversation.
fn alone(state: State) -> Option<ProviderMessageId> {
    state
        .open_message_id()
        .filter(|_| !state.whole_conversation.get())
}

/// Changes whenever the thread, or anything shown of it, does. Not its
/// unread count: the pane shows nothing of it, and marking the open thread
/// read would draw every message again, its mail cleaned a second time.
fn thread_key(thread: &Thread) -> String {
    let s = &thread.summary;
    let downloaded = thread.messages.iter().filter(|m| m.downloaded).count();
    format!(
        "{}|{}|{}|{downloaded}",
        s.account.key(),
        s.id.0,
        s.message_count
    )
}

#[component]
fn open_thread(
    state: State,
    thread: Thread,
    order: crate::conversations::Order,
    only: Option<ProviderMessageId>,
) -> NodeHandle {
    let tz = TimeZone::system();
    let summary = &thread.summary;
    // Which account, by its colour, where the sidebar does not already say
    // (a list of more than one account's mail).
    let account = state.sidebar.with(|s| {
        crate::ui::list::account_index(s.as_ref(), summary)
            .filter(|_| state.shown_account().is_none())
            .map(crate::ui::account_color)
    });
    let dot = match account {
        Some(c) => format!("background: {c}"),
        None => "display: none".to_owned(),
    };
    let account_name = summary.account.to_string();
    let one_of_many = only.is_some() && summary.message_count > 1;
    let subject = if summary.subject.trim().is_empty() {
        "(no subject)".to_owned()
    } else {
        summary.subject.clone()
    };
    let mut shown = in_order(order, &thread);
    if let Some(id) = &only {
        shown.retain(|m| &m.id == id);
    }
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
            div { class: "reader-scroll",
            div { class: "thread",
            div { class: "thread-title",
                span { class: "dot", title: {account_name.clone()}, style: {dot.clone()} }
                h2 { class: "thread-subject", {subject} }
                span {
                    class: "open-all",
                    style: {if one_of_many { "" } else { "display: none" }},
                    onclick: move || state.whole_conversation.set(true),
                    "Show the conversation"
                }
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
                // Redrawn when the window turns light or dark, for mail
                // drawn in its colours.
                for drawn in state.rendered.get().get(&id).cloned().map(|r| { let dark = r.plain && state.dark(); (r, dark) }) {
                    div { key: format!("{}|{}|{}", drawn.0.remote_hidden, drawn.0.html.len(), drawn.1),
                        {mail(__scope, state, shown.clone(), drawn.0.clone(), drawn.1)}
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
    let attachments =
        crate::ui::attachment_cards::attachment_cards(__scope, state, message.clone());
    let invitation = crate::ui::invitation_card::invitation_card(__scope, state, message.clone());
    rsx! {
        div { class: "message",
            div { class: "message-head",
                span { class: "message-from", {name} }
                span { class: "message-address", {address} }
                span { class: "message-when", {when} }
                {edit_draft(__scope, state, message.clone())}
                {answer_icons(__scope, state, message.id.clone())}
            }
            {to}
            {invitation}
            {body}
            {attachments}
        }
    }
}

/// A message as Postrinch drew it: the hidden-pictures bar when pictures
/// from the web were held back, and the mail itself with the earlier
/// messages it quotes folded under "Show quoted text". Mail with no design
/// of its own is drawn in the window's colours, and under `dark` redrawn in
/// dark ones (postrinch_render::for_dark); other mail on the white it was
/// designed on. View › Open in browser shows it there.
#[component]
fn mail(state: State, message: Message, rendered: Rendered, dark: bool) -> NodeHandle {
    // The mail's own body (its style, its contents) inside the white card,
    // so a body with no padding of its own still sits off the card's edge.
    // HTML mail is cleaned by the view at the card's width, and again when a
    // resize crosses one of its `@media` breakpoints (a narrow pane gets what
    // a phone shows); text mail is drawn as rendered. A click on a link opens
    // it (crate::links). The quote is hidden by a user style sheet, not
    // removed (postrinch_render::quotes), so showing it only restyles.
    let quote_shown = Signal::new(false);
    let plain = rendered.plain;
    let quoted = Signal::new(0usize);
    let view = if rendered.document.is_empty() {
        let (html, body_style) = (rendered.html.clone(), rendered.body_style.clone());
        rsx! { HtmlView { html: html, body_style: body_style, onlink: move |href: String| crate::links::open(&href) } }
    } else {
        let document = if dark {
            postrinch_render::for_dark(&rendered.document, crate::ui::palette::DARK_MAIL)
        } else {
            rendered.document.clone()
        };
        let policy = postrinch_render::policy(rendered.remote);
        rsx! {
            HtmlView {
                document: document,
                policy: policy,
                user_css_fn: move || postrinch_render::quote_sheet(quote_shown.get(), plain),
                onusermatches: move |counts: Vec<usize>| quoted.set(counts.iter().sum()),
                onlink: move |href: String| crate::links::open(&href),
            }
        }
    };
    let card = if rendered.plain {
        "mail-html plain"
    } else {
        "mail-html"
    };
    let host = rsx! { div { class: card, {view} } };
    let quote_toggle = rsx! {
        div {
            class: "quote-toggle",
            role: "button",
            style: {move || if quoted.get() > 0 { "" } else { "display: none" }},
            onclick: move || quote_shown.update(|shown| *shown = !*shown),
            {move || if quote_shown.get() { "Hide quoted text" } else { "Show quoted text" }}
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
    // Showing them is remembered for the sender, as the person asked.
    let bar = (hidden > 0).then(|| {
        let sender = message.clone();
        rsx! {
            div { class: "pictures-bar", role: "status",
                span { class: "pictures-line", {line.clone()} }
                Button { variant: "default", size: "xs", onclick: move || state.show_pictures(&sender), "Show pictures" }
            }
        }
    });
    rsx! {
        div { class: "mail",
            {bar}
            {host}
            {quote_toggle}
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

/// Edit draft, in the header of a message that is one of its account's
/// drafts; nothing for any other.
#[component]
fn edit_draft(state: State, message: Message) -> NodeHandle {
    let draft = crate::actions::commands()
        .is_some_and(|c| c.is_draft(&message.account, &message).unwrap_or(false));
    if !draft {
        return rsx! { span {} };
    }
    rsx! {
        Button { variant: "filled", size: "xs", onclick: move || state.edit_draft(&message), "Edit draft" }
    }
}

/// Reply, Reply all and Forward for one message, as icons at the right of
/// its header.
#[component]
fn answer_icons(state: State, message: ProviderMessageId) -> NodeHandle {
    let (reply, all, forward) = (message.clone(), message.clone(), message);
    rsx! {
        span { class: "answer-icons",
            button { class: "answer-icon", title: "Reply (R)", aria-label: "Reply",
                onclick: move || state.answer(Start::Reply, Some(&reply)),
                {icon(__scope, Icon::Reply)}
            }
            button { class: "answer-icon", title: "Reply all (A)", aria-label: "Reply all",
                onclick: move || state.answer(Start::ReplyAll, Some(&all)),
                {icon(__scope, Icon::ReplyAll)}
            }
            button { class: "answer-icon", title: "Forward (F)", aria-label: "Forward",
                onclick: move || state.answer(Start::Forward, Some(&forward)),
                {icon(__scope, Icon::Forward)}
            }
        }
    }
}
