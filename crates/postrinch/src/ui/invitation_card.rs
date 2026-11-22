//! An invitation at the top of its message: the event as a calendar page
//! and lines (what, when, where, who), and, when it asks for an answer and
//! the account is a guest, Yes, Maybe and No, the account's own answer
//! marked. The calendar's copy of the event is shown once it is found; the
//! invitation's own words until then.

use postrinch_commands::may_invite;
use postrinch_model::{
    Answer, Attendee, CalendarEvent, EmailAddress, EventWhen, Invitation, InvitationKind, Mailbox,
    Message,
};
use rinch::prelude::*;

use crate::invitations::Lookup;
use crate::state::State;

/// What the card shows: the invitation, and its calendar's answer.
type Shown = (Invitation, Option<Lookup>);

#[component]
pub fn invitation_card(state: State, message: Message) -> NodeHandle {
    if !may_invite(&message) {
        return rsx! { span {} };
    }
    let id = message.id.0.clone();
    let account = message.account.clone();
    rsx! {
        div {
            for shown in shown(state, &id) {
                div { key: key(shown),
                    {card(__scope, state, account.clone(), id.clone(), shown.0.clone(), shown.1.clone())}
                }
            }
        }
    }
}

fn shown(state: State, id: &str) -> Option<Shown> {
    let invitation = state.invitations.found.get().get(id).cloned()?;
    let lookup = state.invitations.events.get().get(id).cloned();
    Some((invitation, lookup))
}

/// Changes whenever what the card shows does.
fn key(shown: &Shown) -> String {
    format!("{:?}|{:?}", shown.0.kind, shown.1)
}

#[component]
fn card(
    state: State,
    account: EmailAddress,
    id: String,
    invitation: Invitation,
    lookup: Option<Lookup>,
) -> NodeHandle {
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
    let foot = foot(__scope, state, &account, &id, &invitation, lookup.as_ref());
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

/// What the card's last line holds.
enum Foot {
    Nothing,
    Note(String),
    /// The event as the calendar has it, and an answer on its way.
    Answers(Box<CalendarEvent>, Option<Answer>),
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
        Some(Lookup::NotInCalendar) => Foot::Note(format!(
            "It isn\u{2019}t in {account}\u{2019}s Google Calendar, so it can\u{2019}t be answered here."
        )),
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
) -> NodeHandle {
    let (event, sending) = match what_foot(account, invitation, lookup) {
        Foot::Nothing => return rsx! { span {} },
        Foot::Note(text) => return rsx! { div { class: "invite-note", {text} } },
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
    let buttons: Vec<NodeHandle> = Answer::ALL
        .into_iter()
        .map(|answer| {
            let label = match answer {
                Answer::Yes => "Yes",
                Answer::Maybe => "Maybe",
                Answer::No => "No",
            };
            let class = if chosen == Some(answer) {
                "invite-answer chosen"
            } else {
                "invite-answer"
            };
            let (account, id) = (account.clone(), id.to_owned());
            rsx! {
                button {
                    class: {class},
                    onclick: move || state.answer_invitation(&account, &id, answer),
                    {label}
                }
            }
        })
        .collect();
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
.invite-sending { font-size: 12px; color: var(--muted); margin-left: 4px; }
.invite-open { margin-left: auto; }
"#;
