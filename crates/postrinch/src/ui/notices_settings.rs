//! Settings › Notifications, after its mockup: new mail by view, who from,
//! what a notice shows and several at once; reminders by calendar (when is
//! Google Calendar's to say); invitations, social messages, agents and
//! refused mail; quiet hours and the window in front. Each change is
//! saved at once.

use rinch::prelude::*;

use crate::notices::{NoticePrefs, Several, Shows};
use crate::state::State;

fn prefs(state: State) -> NoticePrefs {
    state.notices.prefs.get()
}

#[component]
pub fn notices_page(state: State) -> NodeHandle {
    let views: Vec<NodeHandle> = state
        .views
        .get()
        .into_iter()
        // Mail the person wrote tells them nothing new.
        .filter(|v| !matches!(v.view.key.as_str(), "sent" | "drafts"))
        .map(|v| {
            let key = v.view.key.clone();
            let depth = match v.under.as_deref() {
                None => "notice-row",
                Some("inbox") => "notice-row under",
                Some(_) => "notice-row under2",
            };
            let (on_key, set_key) = (key.clone(), key.clone());
            rsx! {
                div { class: {depth},
                    span { class: "notice-name", {v.view.name.clone()} }
                    Switch {
                        checked_fn: move || prefs(state).view_on(&on_key),
                        onchange: move || {
                            let on = prefs(state).view_on(&set_key);
                            let k = set_key.clone();
                            state.set_notices(|p| { p.views.insert(k, !on); });
                        },
                    }
                }
            }
        })
        .collect();
    let calendars: Vec<NodeHandle> = state
        .calendar
        .calendars
        .get()
        .into_iter()
        .map(|(account, info)| {
            let dot = format!(
                "background: {}",
                info.color.clone().unwrap_or_else(|| "var(--accent)".into())
            );
            let label = if info.name == account.to_string() {
                info.name.clone()
            } else {
                format!("{} \u{b7} {account}", info.name)
            };
            let (a1, i1, a2, i2) = (account.clone(), info.clone(), account.clone(), info.clone());
            rsx! {
                div { class: "notice-row",
                    span { class: "cal-dot", style: {dot.clone()} }
                    span { class: "notice-name", {label} }
                    Switch {
                        checked_fn: move || prefs(state).calendar_on(&a1, &i1),
                        onchange: move || {
                            let on = prefs(state).calendar_on(&a2, &i2);
                            let k = crate::calendar_ui::hide_key(&a2, &i2.id);
                            state.set_notices(|p| { p.calendars.insert(k, !on); });
                        },
                    }
                }
            }
        })
        .collect();
    let no_calendars = calendars.is_empty().then(|| {
        rsx! { div { class: "notice-row", span { class: "notice-note", "No calendars yet: sync brings them." } } }
    });
    rsx! {
        main { class: "settings-main notices",
            h1 { class: "settings-heading", "Notifications" }
            div { class: "notice-lead",
                span { "Notices show in your desktop\u{2019}s notification area. A click on one opens what it is about." }
                Button { variant: "default", onclick: move || state.test_notice(), "Send a test notice" }
            }

            {section(__scope, state, "New mail", |p| p.mail, |p| p.mail = !p.mail)}
            p { class: "settings-hint", "Tell me when mail arrives in these views. A view of your own, or a new account, starts off." }
            div { class: "notice-list", {views} }
            h2 { class: "settings-subheading", "Only from" }
            div { class: "choices two",
                {card(__scope, state, "Anyone", "Every new message in those views.", |p| !p.known_only, |p| p.known_only = false)}
                {card(__scope, state, "People I have written to", "Mail from strangers and lists comes in quietly.", |p| p.known_only, |p| p.known_only = true)}
            }
            h2 { class: "settings-subheading", "What a notice shows" }
            div { class: "choices three",
                {card(__scope, state, "Who, the subject and how it starts", "Ada Lovelace \u{b7} Plans for Tuesday, and its first words.", |p| p.shows == Shows::Everything, |p| p.shows = Shows::Everything)}
                {card(__scope, state, "Who and the subject", "Ada Lovelace \u{b7} Plans for Tuesday", |p| p.shows == Shows::WhoAndSubject, |p| p.shows = Shows::WhoAndSubject)}
                {card(__scope, state, "Only that mail came", "\u{201c}New mail in Inbox\u{201d}, for a screen others see.", |p| p.shows == Shows::Nothing, |p| p.shows = Shows::Nothing)}
            }
            h2 { class: "settings-subheading", "When several arrive at once" }
            div { class: "choices two",
                {card(__scope, state, "A notice each, up to three", "More than three become one: \u{201c}5 new messages in Inbox\u{201d}.", |p| p.several == Several::EachUpToThree, |p| p.several = Several::EachUpToThree)}
                {card(__scope, state, "One notice for them all", "\u{201c}3 new messages: Ada, Bob, the bank\u{201d}.", |p| p.several == Several::OneForAll, |p| p.several = Several::OneForAll)}
            }

            {section(__scope, state, "Event reminders", |p| p.reminders, |p| p.reminders = !p.reminders)}
            p { class: "settings-hint", "When each event\u{2019}s reminders say, as set in Google Calendar: its own, or its calendar\u{2019}s. Here you choose only which calendars remind you. Not for events you declined." }
            div { class: "notice-list", {calendars} {no_calendars} }

            h2 { class: "notices-heading", "Also tell me" }
            div { class: "notice-list",
                {toggle(__scope, state, "New invitations", "Even when they land outside the views above.", |p| p.invitations, |p| p.invitations = !p.invitations)}
                {toggle(__scope, state, "New social messages", "What each channel\u{2019}s or group\u{2019}s notices let through, and private messages not muted.", |p| p.social, |p| p.social = !p.social)}
                {toggle(__scope, state, "An agent needs you", "A message, rule, answer or calendar change an agent proposed, waiting for you.", |p| p.agents, |p| p.agents = !p.agents)}
                {toggle(__scope, state, "Mail Gmail would not send", "A message refused, waiting for you to put right.", |p| p.refused, |p| p.refused = !p.refused)}
            }

            {section(__scope, state, "Quiet hours", |p| p.quiet, |p| p.quiet = !p.quiet)}
            div { class: "quiet-row",
                span { "From" }
                TextInput {
                    spellcheck: false,
                    class: "quiet-time",
                    value_fn: move || prefs(state).quiet_from,
                    oninput: move |v: String| state.set_notices(|p| p.quiet_from = v),
                }
                span { "to" }
                TextInput {
                    spellcheck: false,
                    class: "quiet-time",
                    value_fn: move || prefs(state).quiet_to,
                    oninput: move |v: String| state.set_notices(|p| p.quiet_to = v),
                }
                span { class: "quiet-on", "on" }
                for (i, name) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].into_iter().enumerate() {
                    button {
                        key: i,
                        class: {move || if prefs(state).quiet_days[i] { "quiet-day on" } else { "quiet-day" }},
                        onclick: move || state.set_notices(|p| p.quiet_days[i] = !p.quiet_days[i]),
                        {name}
                    }
                }
            }
            Checkbox {
                label: "Reminders still come through",
                checked_fn: move || prefs(state).quiet_reminders,
                onchange: move || state.set_notices(|p| p.quiet_reminders = !p.quiet_reminders),
            }
            Checkbox {
                label: "No mail notices while Postrinch is in front: the sidebar\u{2019}s counts say it",
                checked_fn: move || prefs(state).not_in_front,
                onchange: move || state.set_notices(|p| p.not_in_front = !p.not_in_front),
            }
            p { class: "settings-hint", "Mail that arrives in quiet hours waits in its views; nothing is told later all at once." }
        }
    }
}

/// A section's heading with its switch.
#[component]
fn section(
    state: State,
    title: &'static str,
    on: fn(&NoticePrefs) -> bool,
    flip: fn(&mut NoticePrefs),
) -> NodeHandle {
    rsx! {
        div { class: "notices-section",
            h2 { class: "notices-heading", {title} }
            Switch {
                checked_fn: move || on(&prefs(state)),
                onchange: move || state.set_notices(flip),
            }
        }
    }
}

/// One choice of several, as Settings' other pages draw them.
#[component]
fn card(
    state: State,
    name: &'static str,
    about: &'static str,
    chosen: fn(&NoticePrefs) -> bool,
    choose: fn(&mut NoticePrefs),
) -> NodeHandle {
    rsx! {
        div {
            class: {move || if chosen(&prefs(state)) { "choice chosen" } else { "choice" }},
            onclick: move || state.set_notices(choose),
            span { class: "choice-name", {name} }
            span { class: "choice-about", {about} }
        }
    }
}

/// A row with words and a switch.
#[component]
fn toggle(
    state: State,
    name: &'static str,
    about: &'static str,
    on: fn(&NoticePrefs) -> bool,
    flip: fn(&mut NoticePrefs),
) -> NodeHandle {
    rsx! {
        div { class: "notice-row",
            div { class: "notice-words",
                span { class: "notice-name", {name} }
                span { class: "notice-note", {about} }
            }
            Switch {
                checked_fn: move || on(&prefs(state)),
                onchange: move || state.set_notices(flip),
            }
        }
    }
}

pub const CSS: &str = r#"
.notices { display: flex; flex-direction: column; gap: 12px; }
/* The page scrolls; nothing on it shrinks to fit. */
.notices > * { flex-shrink: 0; }
.notice-lead { display: flex; align-items: center; gap: 12px; color: var(--ink-2); }
.notice-lead > span { flex: 1; }
.notices-section { display: flex; align-items: center; gap: 12px; margin-top: 18px; }
.notices-heading { margin: 18px 0 0; font-size: 16px; font-weight: 600; color: var(--ink); flex: 1; }
.notices-section .notices-heading { margin: 0; }
.notice-list { border: 1px solid var(--line); border-radius: 10px; overflow: hidden; }
.notice-row {
    display: flex; align-items: center; gap: 12px; padding: 10px 14px;
    border-bottom: 1px solid var(--line-soft);
}
.notice-row:last-child { border-bottom: 0; }
.notice-row.under { padding-left: 32px; }
.notice-row.under2 { padding-left: 52px; }
.notice-name { flex: 1; min-width: 0; color: var(--ink); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.notice-words { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.notice-note { font-size: 12px; color: var(--muted); }
.choices.two { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px; }
.choices.three { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; }
.quiet-row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; color: var(--ink-2); }
.quiet-time { width: 76px; }
.quiet-on { margin-left: 10px; }
.quiet-day {
    height: 26px; padding: 0 10px; border: 1px solid var(--field); border-radius: 13px;
    background: var(--surface); color: var(--muted); font: inherit; font-size: 13px; cursor: pointer;
    display: flex; align-items: center;
}
.quiet-day.on { background: var(--accent-tint); color: var(--accent-ink); border-color: var(--accent-tint); }
"#;
