//! The sidebar's word on mail written here: agents' proposals (messages
//! and rules) waiting for the person, what is still sending, and what the
//! provider refused.

use postrinch_commands::{Queued, RuleProposal};
use postrinch_store::OutgoingKind;
use rinch::prelude::*;

use crate::state::State;

#[component]
pub fn outgoing_lines(state: State) -> NodeHandle {
    rsx! {
        div { class: "outgoing",
            style: {move || if state.outgoing.with(|all| all.is_empty()) && state.rules.proposals.with(|all| all.is_empty()) && state.invitations.proposals.with(|all| all.is_empty()) && state.calendar.proposals.with(|all| all.is_empty()) { "display: none".to_owned() } else { String::new() }},
            // Keyed by what each is, so a line is built once.
            for queued in state.outgoing.get().into_iter().filter(|q| q.kind == OutgoingKind::Proposed || q.refused) {
                div { key: format!("{}|{}", queued.id, queued.refused),
                    {needs_you(__scope, state, queued.clone())}
                }
            }
            for (account, proposal) in state.invitations.proposals.get() {
                div { key: format!("{}|{}|{:?}", account.key(), proposal.message.0, proposal.answer),
                    {answer_proposed(__scope, state, account.clone(), proposal.clone())}
                }
            }
            for (account, proposal) in state.calendar.proposals.get() {
                div { key: format!("{}|{}", account.key(), proposal.id),
                    {event_proposed(__scope, state, account.clone(), proposal.clone())}
                }
            }
            for proposal in state.rules.proposals.get() {
                div { key: format!("{}|{}", proposal.account.key(), proposal.proposed.id),
                    {rule_proposed(__scope, state, proposal.clone())}
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

/// A rule an agent proposed: opens it in the rule editor.
#[component]
fn rule_proposed(state: State, proposal: RuleProposal) -> NodeHandle {
    let title = format!("{} proposed a rule for you", proposal.proposed.agent);
    let detail = proposal.proposed.rule.name.clone();
    rsx! {
        div { class: "needs-you", role: "button",
            onclick: move || {
                state.view.set(crate::state::View::Settings);
                state.settings_section.set(crate::state::SettingsSection::Rules);
                state.rules.open_proposal(&proposal);
            },
            span { class: "needs-you-title", {title} }
            span { class: "needs-you-detail", {detail} }
        }
    }
}

/// An agent's answer to an invitation: opens its conversation.
#[component]
fn answer_proposed(
    state: State,
    account: postrinch_model::EmailAddress,
    proposal: postrinch_model::ProposedAnswer,
) -> NodeHandle {
    let title = format!("{} suggests an answer to an invitation", proposal.agent);
    let word = match proposal.answer {
        postrinch_model::Answer::Yes => "Yes",
        postrinch_model::Answer::Maybe => "Maybe",
        postrinch_model::Answer::No => "No",
    };
    let detail = match &proposal.summary {
        Some(s) => format!("{word}: {s}"),
        None => word.to_owned(),
    };
    rsx! {
        div { class: "needs-you", role: "button",
            onclick: move || state.open_answer(&account, &proposal),
            span { class: "needs-you-title", {title} }
            span { class: "needs-you-detail", {detail} }
        }
    }
}

/// An agent's calendar change: opens it where it can be made.
#[component]
fn event_proposed(
    state: State,
    account: postrinch_model::EmailAddress,
    proposal: postrinch_model::EventProposal,
) -> NodeHandle {
    use postrinch_model::EventChange;
    let what = match &proposal.change {
        EventChange::Save { event: None, .. } => "proposes an event",
        EventChange::Save { .. } => "wants to change an event",
        EventChange::Delete { .. } => "wants to delete an event",
        EventChange::Answer { .. } => "suggests an answer to an event",
    };
    let title = format!("{} {what}", proposal.agent);
    let detail = proposal.change.summary().unwrap_or("(no title)").to_owned();
    rsx! {
        div { class: "needs-you", role: "button",
            onclick: move || state.open_event_proposal(&account, &proposal),
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
