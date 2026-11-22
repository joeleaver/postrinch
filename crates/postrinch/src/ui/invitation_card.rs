//! An invitation at the top of its message: the event as a calendar page
//! and lines (what, when, where, who), and, when it asks for an answer and
//! the account is a guest, Yes, Maybe and No, the account's own answer
//! marked. The calendar's copy of the event is shown once it is found; the
//! invitation's own words until then. An agent's answer waiting for the
//! person shows above the buttons, its answer ringed: pressing any answer
//! gives that one, and Dismiss drops the agent's.

use postrinch_commands::may_invite;
use postrinch_model::{
    Answer, Attendee, CalendarEvent, EmailAddress, EventWhen, Invitation, InvitationKind, Mailbox,
    Message, ProposedAnswer,
};
use rinch::prelude::*;

use crate::invitations::Lookup;
use crate::state::State;

/// What the card shows: the invitation, its calendar's answer, and an
/// agent's answer waiting.
type Shown = (Invitation, Option<Lookup>, Option<ProposedAnswer>);

#[component]
pub fn invitation_card(state: State, message: Message) -> NodeHandle {
    if !may_invite(&message) {
        return rsx! { span {} };
    }
    let id = message.id.0.clone();
    let account = message.account.clone();
    rsx! {
        div {
            for shown in shown(state, &account, &id) {
                div { key: key(shown),
                    {card(__scope, state, account.clone(), id.clone(), shown.clone())}
                }
            }
        }
    }
}

fn shown(state: State, account: &EmailAddress, id: &str) -> Option<Shown> {
    let invitation = state.invitations.found.get().get(id).cloned()?;
    let lookup = state.invitations.events.get().get(id).cloned();
    Some((invitation, lookup, state.proposed_answer(account, id)))
}

/// Changes whenever what the card shows does.
fn key(shown: &Shown) -> String {
    let proposed = shown.2.as_ref().map(|p| (p.answer, p.made));
    format!("{:?}|{:?}|{proposed:?}", shown.0.kind, shown.1)
}

#[component]
fn card(state: State, account: EmailAddress, id: String, shown: Shown) -> NodeHandle {
    let (invitation, lookup, proposed) = shown;
    let proposed = proposed.filter(|_| invitation.kind == InvitationKind::Request);
    let event = match &lookup {
        Some(Lookup::Found(e) | Lookup::Answering(e, _)) => Some(e.clone()),
        _ => None,
    };
    let summary = event
        .as_ref()
        .and_then(|e| e.summary.clone())
        .or_else(|| invitation.summary.clone())
        .unwrap_or_else(|| "An event".to_owned());
    let when: Option<EventWhen> = event.as_ref().map(|e| e.when).or(invitation.when);
    let tz = crate::dates::now().time_zone().clone();
    let (month, day) = when
        .map(|w| crate::dates::event_page(&w, &tz))
        .unwrap_or_default();
    let when_line = when.map(|w| crate::dates::event(&w, &tz));
    let location = event
        .as_ref()
        .and_then(|e| e.location.clone())
        .or_else(|| invitation.location.clone());
    let organizer = event
        .as_ref()
        .and_then(|e| e.organizer.clone())
        .or_else(|| invitation.organizer.clone());
    let guests = event
        .as_ref()
        .map(|e| e.attendees.clone())
        .unwrap_or_else(|| invitation.attendees.clone());
    let who = who(organizer.as_ref(), &guests, &account);
    let repeats = invitation.repeats || event.as_ref().is_some_and(|e| e.repeating);
    let suggested = proposed.as_ref().map(|p| p.answer);
    let foot = foot(
        __scope,
        state,
        &account,
        &id,
        &invitation,
        lookup.as_ref(),
        suggested,
    );
    let suggestion = proposed.map(|p| suggestion(__scope, state, account.clone(), p));
    let cancelled =
        invitation.kind == InvitationKind::Cancel || event.as_ref().is_some_and(|e| e.cancelled);
    let class = if cancelled {
        "invite cancelled"
    } else {
        "invite"
    };
    rsx! {
        div { class: {class},
            div { class: "invite-page",
                span { class: "invite-month", {month} }
                span { class: "invite-day", {day} }
            }
            div { class: "invite-main",
                div { class: "invite-title",
                    span { {summary} }
                    if repeats {
                        span { class: "invite-chip", "Repeats" }
                    }
                }
                for line in when_line.clone() {
                    div { key: line.clone(), class: "invite-line", {line.clone()} }
                }
                for line in location.clone() {
                    div { key: line.clone(), class: "invite-line", {line.clone()} }
                }
                for line in who.clone() {
                    div { key: line.clone(), class: "invite-line muted", {line.clone()} }
                }
                {suggestion}
                {foot}
            }
        }
    }
}

/// "Ada Lovelace invites you and 3 others", or who organizes it.
fn who(organizer: Option<&Mailbox>, guests: &[Attendee], account: &EmailAddress) -> Option<String> {
    let others = guests
        .iter()
        .filter(|g| {
            g.mailbox.address != *account
                && Some(&g.mailbox.address) != organizer.map(|o| &o.address)
        })
        .count();
    let organizer = organizer?;
    if organizer.address == *account {
        return Some(match others {
            0 => "You organize this".to_owned(),
            1 => "You organize this, with 1 guest".to_owned(),
            n => format!("You organize this, with {n} guests"),
        });
    }
    let name = organizer.display_name();
    Some(match others {
        0 => format!("{name} invites you"),
        1 => format!("{name} invites you and 1 other"),
        n => format!("{name} invites you and {n} others"),
    })
}

/// An agent's answer waiting for the person, with its reason.
#[component]
fn suggestion(state: State, account: EmailAddress, proposed: ProposedAnswer) -> NodeHandle {
    let word = label(proposed.answer);
    let line = format!(
        "{} suggests answering \u{201c}{word}\u{201d}.",
        proposed.agent
    );
    let reason = proposed.reason.clone();
    let message = proposed.message.clone();
    rsx! {
        div { class: "invite-suggest",
            div { class: "invite-suggest-line",
                span { class: "invite-suggest-who", {line} }
                button {
                    class: "link-button",
                    onclick: move || state.dismiss_answer(&account, &message),
                    "Dismiss"
                }
            }
            for reason in reason.clone() {
                div { key: reason.clone(), class: "invite-suggest-reason", {reason.clone()} }
            }
        }
    }
}

/// What the card's last line holds.
enum Foot {
    Nothing,
    Note(String),
    /// The event as the calendar has it, and an answer on its way.
    Answers(Box<CalendarEvent>, Option<Answer>),
    /// Not in the calendar, but the account is a guest: answered by mail
    /// to the organizer.
    ByMail(Mailbox),
}

fn what_foot(account: &EmailAddress, invitation: &Invitation, lookup: Option<&Lookup>) -> Foot {
    match invitation.kind {
        InvitationKind::Cancel => return Foot::Note("This event is cancelled.".into()),
        InvitationKind::Reply => {
            return invitation
                .attendees
                .iter()
                .find_map(|a| {
                    let word = match a.answer? {
                        Answer::Yes => "is going",
                        Answer::Maybe => "might go",
                        Answer::No => "isn\u{2019}t going",
                    };
                    Some(Foot::Note(format!("{} {word}.", a.mailbox.display_name())))
                })
                .unwrap_or(Foot::Nothing);
        }
        InvitationKind::Publish => return Foot::Nothing,
        InvitationKind::Request => {}
    }
    match lookup {
        None | Some(Lookup::Looking) => {
            Foot::Note("Looking for it in your calendar\u{2026}".into())
        }
        Some(Lookup::NotInCalendar) => {
            let guest = invitation
                .attendees
                .iter()
                .any(|a| a.mailbox.address.key() == account.key());
            match (&invitation.organizer, guest) {
                (Some(organizer), true) => Foot::ByMail(organizer.clone()),
                _ => Foot::Note(format!(
                    "It isn\u{2019}t in {account}\u{2019}s Google Calendar, and {account} isn\u{2019}t among its guests, so there is nothing to answer here."
                )),
            }
        }
        Some(Lookup::Mailed(answer)) => {
            let to = invitation
                .organizer
                .as_ref()
                .map_or_else(|| "the organizer".to_owned(), Mailbox::display_name);
            Foot::Note(format!(
                "Answered \u{201c}{}\u{201d}: on its way to {to} by mail.",
                label(*answer)
            ))
        }
        Some(Lookup::Failed(why)) => Foot::Note(why.clone()),
        Some(Lookup::Found(e)) if e.cancelled => Foot::Note("This event is cancelled.".into()),
        Some(Lookup::Found(e)) => Foot::Answers(Box::new(e.clone()), None),
        Some(Lookup::Answering(e, a)) => Foot::Answers(Box::new(e.clone()), Some(*a)),
    }
}

/// The card's last line: the answer buttons, or what stands in for them.
#[component]
fn foot(
    state: State,
    account: &EmailAddress,
    id: &str,
    invitation: &Invitation,
    lookup: Option<&Lookup>,
    suggested: Option<Answer>,
) -> NodeHandle {
    let (event, sending) = match what_foot(account, invitation, lookup) {
        Foot::Nothing => return rsx! { span {} },
        Foot::Note(text) => return rsx! { div { class: "invite-note", {text} } },
        Foot::ByMail(organizer) => {
            let note = format!(
                "It isn\u{2019}t in {account}\u{2019}s Google Calendar, so your answer goes to {} by mail.",
                organizer.display_name()
            );
            let buttons = answer_buttons(__scope, state, account, id, (None, suggested), true);
            return rsx! {
                div {
                    div { class: "invite-note", {note} }
                    div { class: "invite-answers",
                        span { class: "invite-going", "Going?" }
                        {buttons}
                    }
                }
            };
        }
        Foot::Answers(event, sending) => (event, sending),
    };
    let link = event.link.clone().map(|href| {
        rsx! {
            button {
                class: "link-button invite-open",
                onclick: move || crate::links::open(&href),
                "Open in Google Calendar"
            }
        }
    });
    if !event.guest {
        return rsx! { div { class: "invite-answers", {link} } };
    }
    let chosen = sending.or(event.answer);
    let buttons = answer_buttons(__scope, state, account, id, (chosen, suggested), false);
    let status = sending.map(|_| {
        rsx! { span { class: "invite-sending", "Answering\u{2026}" } }
    });
    rsx! {
        div { class: "invite-answers",
            span { class: "invite-going", "Going?" }
            {buttons}
            {status}
            {link}
        }
    }
}

fn label(answer: Answer) -> &'static str {
    match answer {
        Answer::Yes => "Yes",
        Answer::Maybe => "Maybe",
        Answer::No => "No",
    }
}

/// Yes, Maybe and No: the account's answer (`chosen`) filled, an agent's
/// (`suggested`) ringed. Each gives its answer in the calendar, or `by_mail`.
fn answer_buttons(
    __scope: &mut RenderScope,
    state: State,
    account: &EmailAddress,
    id: &str,
    (chosen, suggested): (Option<Answer>, Option<Answer>),
    by_mail: bool,
) -> Vec<NodeHandle> {
    Answer::ALL
        .into_iter()
        .map(|answer| {
            let class = match (chosen == Some(answer), suggested == Some(answer)) {
                (true, _) => "invite-answer chosen",
                (false, true) => "invite-answer suggested",
                (false, false) => "invite-answer",
            };
            let (account, id) = (account.clone(), id.to_owned());
            rsx! {
                button {
                    class: {class},
                    onclick: move || if by_mail {
                        state.answer_by_mail(&account, &id, answer)
                    } else {
                        state.answer_invitation(&account, &id, answer)
                    },
                    {label(answer)}
                }
            }
        })
        .collect()
}

/// The card's styles.
pub const CSS: &str = r#"
.invite {
    display: flex; gap: 14px;
    padding: 14px 16px;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--panel);
}
.invite-page {
    width: 48px; height: 52px; flex-shrink: 0;
    display: flex; flex-direction: column; align-items: center;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--surface);
    overflow: hidden;
}
.invite-month {
    width: 100%; padding: 2px 0;
    text-align: center;
    font-size: 10px; font-weight: 700; letter-spacing: 0.06em;
    background: var(--accent); color: #ffffff;
}
.invite-day { flex: 1; display: flex; align-items: center; font-size: 20px; font-weight: 600; color: var(--ink); }
.invite-main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.invite-title { display: flex; align-items: center; gap: 8px; font-weight: 600; font-size: 15px; color: var(--ink); }
.invite.cancelled .invite-title span:first-child { text-decoration: line-through; color: var(--muted); }
.invite-chip {
    padding: 1px 7px; border-radius: 999px;
    font-size: 11px; font-weight: 500;
    background: var(--accent-tint); color: var(--accent-ink);
}
.invite-line { font-size: 13px; color: var(--ink-2); overflow-wrap: anywhere; }
.invite-line.muted { color: var(--muted); }
.invite-note { margin-top: 6px; font-size: 13px; color: var(--ink-2); }
.invite-answers { margin-top: 8px; display: flex; align-items: center; flex-wrap: wrap; gap: 6px; }
.invite-going { font-size: 13px; color: var(--ink-2); margin-right: 4px; }
.invite-answer {
    height: 28px; padding: 0 14px;
    border: 1px solid var(--field); border-radius: 6px;
    background: var(--surface); color: var(--ink);
    font: inherit; font-size: 13px; cursor: pointer;
}
.invite-answer:hover { background: var(--hover); }
.invite-answer.chosen { background: var(--accent); border-color: var(--accent); color: #ffffff; }
.invite-answer.suggested { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent); }
.invite-suggest {
    margin-top: 8px; padding: 8px 10px;
    border-radius: 8px;
    background: var(--warn-bg);
    display: flex; flex-direction: column; gap: 2px;
}
.invite-suggest-line { display: flex; align-items: center; gap: 10px; }
.invite-suggest-who { flex: 1; font-size: 13px; font-weight: 600; color: var(--ink); }
.invite-suggest-reason { font-size: 13px; color: var(--ink-2); }
.invite-sending { font-size: 12px; color: var(--muted); margin-left: 4px; }
.invite-open { margin-left: auto; }
"#;
