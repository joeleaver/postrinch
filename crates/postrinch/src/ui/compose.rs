//! The compose dialog, after the Compose and Confirm mockups: From, To,
//! Cc, Bcc, Subject and the text, with Send, Keep as draft and Discard.
//! An agent's proposal shows its banner, its reason and who is outside
//! the account's domain, and goes only when the person presses Send. The
//! buttons are in the dialog's footer, which stays put as a long draft
//! scrolls.

use postrinch_model::{Draft, EmailAddress, format_mailboxes, parse_mailboxes};
use rinch::prelude::*;

use crate::state::State;
use crate::writing::Compose;

/// The open form's fields, for the buttons in the dialog's footer, which
/// stays put under the fields as they scroll.
#[derive(Clone)]
struct Form {
    draft: Draft,
    from: Signal<String>,
    to: Signal<String>,
    cc: Signal<String>,
    bcc: Signal<String>,
    subject: Signal<String>,
    body: Signal<String>,
}

impl Form {
    /// The draft as the form now has it, or why it does not read.
    fn current(&self) -> Result<Draft, String> {
        let mut out = self.draft.clone();
        out.account = EmailAddress::parse(&self.from.get())
            .map_err(|_| "Choose an account to send from.".to_owned())?;
        out.to = parse_mailboxes(&self.to.get()).map_err(|e| format!("To: {e}."))?;
        out.cc = parse_mailboxes(&self.cc.get()).map_err(|e| format!("Cc: {e}."))?;
        out.bcc = parse_mailboxes(&self.bcc.get()).map_err(|e| format!("Bcc: {e}."))?;
        out.subject = self.subject.get();
        out.body = self.body.get();
        Ok(out)
    }
}

#[component]
pub fn compose_dialog(state: State) -> NodeHandle {
    let open: Signal<Option<Form>> = Signal::new(None);
    let act = move |send: bool| {
        let Some(form) = open.get() else {
            return;
        };
        match form.current() {
            Ok(d) if send => state.send_composed(d),
            Ok(d) => state.keep_composed(d),
            Err(e) => state.compose_error.set(Some(e)),
        }
    };
    // The confirming button last, on the right; the rest before it.
    let footer: SectionRenderer = std::rc::Rc::new(move |__scope| {
        rsx! {
            div { class: "compose-footer",
                Button { variant: "subtle", color: "red", size: "sm", onclick: move || state.discard_composed(), "Discard" }
                span { class: "compose-footer-gap" }
                Button { variant: "default", size: "sm", onclick: move || act(false), "Keep as draft" }
                Button { variant: "filled", size: "sm", onclick: move || act(true), "Send" }
            }
        }
    });
    rsx! {
        Modal {
            opened_fn: move || state.composing.get().is_some(),
            onclose: move || state.discard_composed(),
            title: "",
            size: "xl",
            // The confirming button last, on the right; the rest before it.
            footer: Some(footer),
            // Keyed on each opening, so every draft starts the form afresh
            // (rinch#2105: a loop, not `if let`).
            for compose in state.composing.get() {
                div { key: compose.opening.to_string(),
                    {form(__scope, state, compose.clone(), open)}
                }
            }
        }
    }
}

#[component]
fn form(state: State, compose: Compose, open: Signal<Option<Form>>) -> NodeHandle {
    let draft = compose.draft.clone();
    let from = Signal::new(draft.account.to_string());
    let to = Signal::new(format_mailboxes(&draft.to));
    let cc = Signal::new(format_mailboxes(&draft.cc));
    let bcc = Signal::new(format_mailboxes(&draft.bcc));
    let subject = Signal::new(draft.subject.clone());
    let body = Signal::new(draft.body.clone());
    let fields = Form {
        draft: draft.clone(),
        from,
        to,
        cc,
        bcc,
        subject,
        body,
    };
    open.set(Some(fields.clone()));
    let accounts: Vec<SelectOption> = state.sidebar.with(|s| {
        s.as_ref()
            .map(|s| {
                s.accounts
                    .iter()
                    .map(|a| {
                        SelectOption::new(
                            a.account.address.to_string(),
                            a.account.address.to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    });
    let outside = Memo::new(move || outside_line(&fields.current()));
    let proposal = compose.draft.proposal.clone();
    let banner = proposal.as_ref().map(|_| {
        let title = compose.title.clone();
        rsx! {
            div { class: "compose-banner",
                span { class: "compose-banner-title", {title} }
                span { "Change anything you like. It goes only when you press Send." }
            }
        }
    });
    let reason = proposal
        .as_ref()
        .and_then(|p| p.reason.clone())
        .map(|reason| {
            rsx! {
                div { class: "compose-aside",
                    span { class: "compose-aside-title", "The agent\u{2019}s reason, in its words" }
                    span { class: "compose-reason", {reason} }
                }
            }
        });
    let heading = (proposal.is_none()).then(|| {
        let title = compose.title.clone();
        rsx! { h2 { class: "dialog-title", {title} } }
    });
    rsx! {
        div { class: "compose",
            {banner}
            {heading}
            div { class: "compose-fields",
                div { class: "compose-row",
                    span { class: "compose-label", "From" }
                    Select { data: accounts, value_fn: move || from.get(), onchange: move |v: String| from.set(v) }
                }
                {field(__scope, "To", to)}
                {field(__scope, "Cc", cc)}
                {field(__scope, "Bcc", bcc)}
                {field(__scope, "Subject", subject)}
            }
            Textarea {
                autosize: true,
                min_rows: Some(14),
                max_rows: Some(28),
                value_fn: move || body.get(),
                oninput: move |v: String| body.set(v),
            }
            div { class: "compose-warn",
                style: {move || if outside.get().is_empty() { "display: none".to_owned() } else { String::new() }},
                {move || outside.get()}
            }
            {reason}
            for error in state.compose_error.get() {
                p { key: error.clone(), class: "agent-warn", {error.clone()} }
            }
        }
    }
}

#[component]
fn field(label: &'static str, value: Signal<String>) -> NodeHandle {
    rsx! {
        div { class: "compose-row",
            span { class: "compose-label", {label} }
            TextInput { value_fn: move || value.get(), oninput: move |v: String| value.set(v) }
        }
    }
}

/// Who the draft goes to outside the sending account's domain.
fn outside_line(draft: &Result<Draft, String>) -> String {
    let Ok(draft) = draft else {
        return String::new();
    };
    let domain = draft.account.domain().to_owned();
    let names: Vec<String> = draft
        .recipients()
        .filter(|m| m.address.domain() != domain)
        .map(|m| m.display_name())
        .collect();
    match names.len() {
        0 => String::new(),
        1 => format!("{} is outside {domain}.", names[0]),
        _ => format!("{} are outside {domain}.", names.join(", ")),
    }
}

pub const CSS: &str = r#"
.compose { display: flex; flex-direction: column; gap: 12px; }
.compose-banner {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 10px 14px;
    border-radius: 8px;
    background: var(--warn-bg);
    color: var(--trouble);
    font-size: 13px;
}
.compose-banner-title { font-size: 15px; font-weight: 600; color: var(--ink); }
.compose-fields { display: flex; flex-direction: column; border-bottom: 1px solid var(--line-soft); }
.compose-row { display: flex; align-items: center; gap: 12px; min-height: 40px; border-top: 1px solid var(--line-soft); }
.compose-row > :last-child { flex: 1; }
.compose-label { width: 60px; color: var(--muted); }
.compose-warn { padding: 8px 12px; border-radius: 8px; background: var(--warn-bg); color: var(--trouble); font-size: 13px; }
.compose-aside { display: flex; flex-direction: column; gap: 4px; padding: 10px 14px; border-radius: 8px; background: var(--panel); }
.compose-aside-title { font-size: 12px; font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase; color: var(--muted); }
.compose-reason { font-family: 'IBM Plex Mono', monospace; font-size: 13px; color: var(--ink-2); line-height: 1.5; }
/* Discard on the left of the footer, the rest on the right. */
.compose-footer { flex: 1; display: flex; align-items: center; gap: 10px; }
.compose-footer-gap { flex: 1; }
"#;
