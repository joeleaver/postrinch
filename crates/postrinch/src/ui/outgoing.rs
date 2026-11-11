//! The sidebar's word on mail written here: agents' proposals waiting for
//! the person, what is still sending, and what the provider refused.

use postrinch_commands::Queued;
use postrinch_store::OutgoingKind;
use rinch::prelude::*;

use crate::state::State;

#[component]
pub fn outgoing_lines(state: State) -> NodeHandle {
    rsx! {
        div { class: "outgoing",
            style: {move || if state.outgoing.with(|all| all.is_empty()) { "display: none".to_owned() } else { String::new() }},
            // Keyed by what each is, so a line is built once.
            for queued in state.outgoing.get().into_iter().filter(|q| q.kind == OutgoingKind::Proposed || q.refused) {
                div { key: format!("{}|{}", queued.id, queued.refused),
                    {needs_you(__scope, state, queued.clone())}
                }
            }
            div {
                class: "outgoing-line",
                style: {move || if sending(state) == 0 { "display: none".to_owned() } else { String::new() }},
                {move || match sending(state) {
                    1 => "Sending a message\u{2026}".to_owned(),
                    n => format!("Sending {n} messages\u{2026}"),
                }}
            }
        }
    }
}

/// Mail and drafts on their way to the provider.
fn sending(state: State) -> usize {
    state.outgoing.with(|all| {
        all.iter()
            .filter(|q| q.kind != OutgoingKind::Proposed && !q.refused)
            .count()
    })
}

/// A proposal to check, or refused mail to put right: opens it.
#[component]
fn needs_you(state: State, queued: Queued) -> NodeHandle {
    let subject = if queued.draft.subject.trim().is_empty() {
        "(no subject)".to_owned()
    } else {
        queued.draft.subject.clone()
    };
    let (title, detail) = if queued.refused {
        (
            format!("Not sent: {subject}"),
            queued
                .error
                .clone()
                .unwrap_or_else(|| "The provider refused it.".into()),
        )
    } else {
        let agent = queued
            .draft
            .proposal
            .as_ref()
            .map_or("An agent", |p| p.agent.as_str())
            .to_owned();
        (format!("{agent} wrote a message for you"), subject)
    };
    let class = if queued.refused {
        "needs-you trouble"
    } else {
        "needs-you"
    };
    rsx! {
        div { class: class, role: "button", onclick: move || state.open_proposal(&queued),
            span { class: "needs-you-title", {title} }
            span { class: "needs-you-detail", {detail} }
        }
    }
}

pub const CSS: &str = r#"
.outgoing { display: flex; flex-direction: column; gap: 6px; padding: 0 4px 10px; }
.outgoing-line { font-size: 12px; color: var(--muted); padding: 0 6px; }
.needs-you {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--warn-bg);
    cursor: pointer;
}
.needs-you.trouble { border: 1px solid var(--warn-line); }
.needs-you-title { font-size: 13px; font-weight: 600; color: var(--ink); }
.needs-you-detail { font-size: 12px; color: var(--trouble); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
"#;
