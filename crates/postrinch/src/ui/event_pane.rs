//! The calendar's right pane, after the calendar mockup: the open event
//! (what, when, where, what it clashes with, an agent's answer waiting,
//! Yes / Maybe / No when the account is a guest, the guests, the notes, ✎
//! and Delete), or the editor.

use jiff::tz::TimeZone;
use postrinch_commands::Happening;
use postrinch_model::{Answer, EventWhen};
use rinch::prelude::*;

use crate::state::State;
use crate::ui::calendar_view::{colours, summary};

#[component]
pub fn event_pane(state: State) -> NodeHandle {
    // Only whether the editor is open: typing must not build it again.
    let editing = Memo::new(move || state.calendar.editing.with(|e| e.is_some()));
    rsx! {
        div { class: "event-pane",
            if editing.get() {
                {crate::ui::event_editor::editor(__scope, state)}
            } else {
                for open in state.calendar.open.get().map(|h| (h, state.calendar.answering.get())) {
                    div { key: format!("{:?}|{:?}", open.0, open.1), class: "event-detail",
                        {detail(__scope, state, open.0.clone(), open.1)}
                    }
                }
                if state.calendar.open.get().is_none() {
                    div { class: "event-empty", "Choose an event to see it here, or make one with + Event." }
                }
            }
        }
    }
}

fn writable(state: State, h: &Happening) -> bool {
    state
        .calendar
        .calendars
        .get()
        .iter()
        .any(|(a, c)| *a == h.account && c.id == h.event.calendar && c.writable)
}

/// Whether the account organizes the event (or nobody does), so may
/// change it; a guest may only answer it, or take it off its calendar.
fn organizes(h: &Happening) -> bool {
    h.event.organizer.as_ref().is_none_or(|o| {
        o.address.key() == h.account.key() || o.address.to_string() == h.event.calendar
    })
}

/// The other events the open one overlaps that make the person busy.
fn clashes(state: State, h: &Happening) -> Vec<String> {
    let EventWhen::Times { start, end } = h.event.when else {
        return Vec::new();
    };
    let tz = TimeZone::system();
    crate::ui::calendar_view::shown(state)
        .into_iter()
        // Not itself, nor its copy in another account's calendar (the same
        // event has the same id in each).
        .filter(|o| o.event.id != h.event.id)
        .filter(|o| o.event.busy && !o.event.cancelled && o.event.answer != Some(Answer::No))
        .filter_map(|o| match o.event.when {
            EventWhen::Times { start: s, end: e } if s < end && e > start => Some(format!(
                "Clashes with {}, {}",
                summary(&o),
                crate::dates::event_hours(&o.event.when, &tz)
            )),
            _ => None,
        })
        .collect()
}

#[component]
fn detail(state: State, h: Happening, answering: Option<Answer>) -> NodeHandle {
    let tz = TimeZone::system();
    let e = &h.event;
    let colour = colours(state)
        .get(&crate::calendar_ui::hide_key(&h.account, &e.calendar))
        .cloned()
        .unwrap_or_else(|| "#6f9cf0".into());
    let when = crate::dates::event(&e.when, &tz);
    let repeats = e.repeating.then_some("Repeats");
    let clash_lines = clashes(state, &h);
    let may_delete = writable(state, &h);
    let may_change = may_delete && organizes(&h);
    let edit_h = h.clone();
    let chosen = answering.or(e.answer);
    let answer_buttons: Vec<NodeHandle> = if e.guest {
        Answer::ALL
            .into_iter()
            .map(|a| {
                let label = match a {
                    Answer::Yes => "Yes",
                    Answer::Maybe => "Maybe",
                    Answer::No => "No",
                };
                let class = if chosen == Some(a) {
                    "invite-answer chosen"
                } else {
                    "invite-answer"
                };
                rsx! { button { class: {class}, onclick: move || state.answer_event(a), {label} } }
            })
            .collect()
    } else {
        Vec::new()
    };
    let going = e.guest.then(|| {
        rsx! {
            div { class: "invite-answers",
                span { class: "invite-going", "Going?" }
                {answer_buttons}
                if answering.is_some() {
                    span { class: "invite-sending", "Answering\u{2026}" }
                }
            }
        }
    });
    let said = |a: Option<Answer>| match a {
        Some(Answer::Yes) => "Yes",
        Some(Answer::Maybe) => "Maybe",
        Some(Answer::No) => "No",
        None => "Not answered",
    };
    let guests: Vec<(String, &'static str)> = e
        .attendees
        .iter()
        .map(|g| {
            let mut name = g.mailbox.display_name();
            if e.organizer
                .as_ref()
                .is_some_and(|o| o.address == g.mailbox.address)
            {
                name.push_str(" (organizer)");
            }
            if g.mailbox.address == h.account {
                name = "You".into();
            }
            (name, said(g.answer))
        })
        .collect();
    let guest_head = (!guests.is_empty()).then(|| {
        let yes = e
            .attendees
            .iter()
            .filter(|g| g.answer == Some(Answer::Yes))
            .count();
        let n = guests.len();
        let noun = if n == 1 { "guest" } else { "guests" };
        format!("{n} {noun} \u{b7} {yes} going")
    });
    let link = e.link.clone();
    let location = e.location.clone();
    let notes = e.description.clone();
    let title = summary(&h);
    let swatch = format!("background: {colour}");
    let delete_label = match (organizes(&h), e.repeating) {
        (false, _) => "Remove from calendar",
        (true, true) => "Delete this time",
        (true, false) => "Delete",
    };
    rsx! {
        div { class: "event-detail-inner",
            div { class: "event-top",
                span { class: "event-swatch", style: {swatch.clone()} }
                div { class: "event-what",
                    span { class: "event-title", {title.clone()} }
                    span { class: "event-when", {when} }
                    for r in repeats {
                        span { key: r, class: "event-sub", {r} }
                    }
                }
                if may_change {
                    button { class: "cal-button square", title: "Change it", aria-label: "Change it",
                        onclick: move || state.edit_event(&edit_h), "\u{270e}" }
                }
            }
            for line in location.clone() {
                div { key: line.clone(), class: "event-line", {line.clone()} }
            }
            for line in clash_lines.clone() {
                div { key: line.clone(), class: "event-line clash", {line.clone()} }
            }
            {going}
            for head in guest_head.clone() {
                div { key: head.clone(), class: "event-guests",
                    span { class: "event-sub", {head.clone()} }
                    for (name, said) in guests.clone() {
                        div { key: name.clone(), class: "event-guest",
                            span { class: "event-guest-name", {name.clone()} }
                            span { class: "event-sub", {said} }
                        }
                    }
                }
            }
            for note in notes.clone() {
                div { key: note.len(), class: "event-notes", {note.clone()} }
            }
            div { class: "event-foot",
                for href in link.clone() {
                    button { key: href.clone(), class: "link-button",
                        onclick: move || crate::links::open(&href), "Open in Google Calendar" }
                }
                if may_delete {
                    button { class: "link-button danger", onclick: move || state.delete_event(),
                        {delete_label} }
                }
            }
        }
    }
}

pub const CSS: &str = r#"
.event-pane {
    width: 380px; flex-shrink: 0; box-sizing: border-box; border-left: 1px solid var(--line);
    background: var(--surface); overflow-y: auto;
}
.event-empty { padding: 24px 20px; color: var(--muted); }
.event-detail-inner { padding: 20px; display: flex; flex-direction: column; gap: 12px; }
.event-top { display: flex; align-items: flex-start; gap: 10px; }
.event-swatch { width: 12px; height: 12px; margin-top: 6px; border-radius: 3px; flex-shrink: 0; }
.event-what { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.event-title { font-size: 18px; font-weight: 600; color: var(--ink); overflow-wrap: anywhere; }
.event-when { color: var(--ink-2); }
.event-sub { font-size: 12px; color: var(--muted); }
.event-line { color: var(--ink-2); overflow-wrap: anywhere; }
.event-line.clash { color: var(--trouble); }
.event-guests { border-top: 1px solid var(--line-soft); padding-top: 10px; display: flex; flex-direction: column; gap: 6px; }
.event-guest { display: flex; gap: 8px; align-items: center; }
.event-guest-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--ink); }
.event-notes {
    border-top: 1px solid var(--line-soft); padding-top: 10px; color: var(--ink-2);
    font-size: 13px; line-height: 1.5; white-space: pre-wrap; overflow-wrap: anywhere;
}
.event-foot { display: flex; gap: 14px; align-items: center; padding-top: 4px; }
.link-button.danger { color: var(--danger); }
"#;
