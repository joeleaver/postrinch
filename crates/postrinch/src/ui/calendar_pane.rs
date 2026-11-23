//! A calendar in the calendar's right pane, after the Share board of the
//! calendar mockup: its colour, name and account; for one of the
//! account's own, who it is shared with and how, each to change or stop,
//! and Share with someone; for one shared with the account, what it may
//! do, and Remove from my calendars.

use postrinch_model::{CalendarInfo, CalendarRole, CalendarShare, EmailAddress, ShareWho};
use rinch::prelude::*;

use crate::calendar_panel::Sharing;
use crate::state::State;

#[component]
pub fn calendar_pane(state: State) -> NodeHandle {
    rsx! {
        div { class: "event-detail",
            for open in opened(state) {
                div { key: format!("{}|{}|{:?}", open.0.key(), open.1.id, open.1.role),
                    {pane(__scope, state, open.0.clone(), open.1.clone())}
                }
            }
        }
    }
}

fn opened(state: State) -> Option<(EmailAddress, CalendarInfo)> {
    let (account, id) = state.panel.open.get()?;
    state
        .calendar
        .calendars
        .get()
        .into_iter()
        .find(|(a, c)| *a == account && c.id == id)
}

fn roles() -> Vec<SelectOption> {
    CalendarRole::ALL
        .into_iter()
        .map(|r| SelectOption::new(r.word(), r.says()))
        .collect()
}

/// The roles in a row's narrow select.
fn short_roles() -> Vec<SelectOption> {
    CalendarRole::ALL
        .into_iter()
        .map(|r| {
            let says = match r {
                CalendarRole::FreeBusy => "Free/busy",
                CalendarRole::Reader => "All details",
                CalendarRole::Writer => "Make changes",
                CalendarRole::Owner => "Manage sharing",
            };
            SelectOption::new(r.word(), says)
        })
        .collect()
}

#[component]
fn pane(state: State, account: EmailAddress, info: CalendarInfo) -> NodeHandle {
    let dot = format!(
        "background: {}",
        info.color.clone().unwrap_or_else(|| "var(--accent)".into())
    );
    let owner = info.role == CalendarRole::Owner;
    let whose = if owner {
        format!("{account} \u{b7} yours")
    } else {
        format!("{account} \u{b7} shared with you")
    };
    let body = if owner {
        shared_with(__scope, state, account.clone())
    } else {
        theirs(__scope, state, &info)
    };
    rsx! {
        div { class: "event-detail-inner",
            div { class: "event-top",
                span { class: "event-swatch", style: {dot.clone()} }
                div { class: "event-what",
                    span { class: "event-title", {info.name.clone()} }
                    span { class: "event-sub", {whose.clone()} }
                }
            }
            {body}
            if let Some(why) = state.panel.error.get() {
                div { class: "event-line clash", {why} }
            }
        }
    }
}

/// One shared with the account: who shared it (a person's calendar is
/// named by their address), what the account may do, and Remove.
#[component]
fn theirs(state: State, info: &CalendarInfo) -> NodeHandle {
    let from = EmailAddress::parse(&info.id)
        .ok()
        .map(|a| format!("Shared with you by {a}."));
    let can = format!("You can: {}.", info.role.says().to_lowercase());
    let primary = info.primary;
    rsx! {
        div { class: "cal-pane-theirs",
            for line in from.clone() {
                div { key: line.clone(), class: "event-line", {line.clone()} }
            }
            div { class: "event-line", {can.clone()} }
            if !primary {
                div { class: "event-foot",
                    button {
                        class: "link-button danger",
                        disabled: {move || state.panel.busy.get()},
                        onclick: move || state.remove_open_calendar(),
                        "Remove from my calendars"
                    }
                }
            }
        }
    }
}

/// One of the account's own: who it is shared with, and Share.
#[component]
fn shared_with(state: State, account: EmailAddress) -> NodeHandle {
    let me = account.key();
    rsx! {
        div { class: "cal-pane-share",
            span { class: "cal-pane-head", "Shared with" }
            match state.panel.sharing.get() {
                Sharing::Loading => div { class: "event-line muted", "Asking Google\u{2026}" },
                Sharing::NotOwner => div { class: "event-line muted", "Only its owners see who it is shared with." },
                Sharing::Failed(why) => div { class: "event-line clash", {why} },
                Sharing::Shares(shares) => {
                    let me = me.clone();
                    {shares_list(__scope, state, shares, me)}
                }
            }
            span { class: "cal-pane-head", "Share with someone" }
            TextInput {
                placeholder: "Address",
                value_fn: move || state.panel.who.get(),
                oninput: move |v: String| state.panel.who.set(v),
                onsubmit: move || state.share_open_calendar(),
            }
            div { class: "cal-share-add",
                Select {
                    data: roles(),
                    value_fn: move || state.panel.role.get().word().to_owned(),
                    onchange: move |v: String| {
                        if let Some(role) = CalendarRole::from_word(&v) {
                            state.panel.role.set(role);
                        }
                    },
                }
                Button {
                    variant: "filled",
                    disabled: {move || state.panel.busy.get() || state.panel.who.with(|w| w.trim().is_empty())},
                    onclick: move || state.share_open_calendar(),
                    "Share"
                }
            }
            span { class: "event-sub",
                "Google emails them that you shared it. Free/busy shows only when you are busy; all details shows what every event says."
            }
        }
    }
}

#[component]
fn shares_list(state: State, shares: Vec<CalendarShare>, me: String) -> NodeHandle {
    let others: Vec<CalendarShare> = shares
        .into_iter()
        .filter(|s| !matches!(&s.who, ShareWho::Person(a) if EmailAddress::parse(a).is_ok_and(|a| a.key() == me)))
        .collect();
    let empty = others.is_empty();
    let rows: Vec<NodeHandle> = others
        .into_iter()
        .map(|share| share_row(__scope, state, share))
        .collect();
    rsx! {
        div { class: "cal-shares",
            if empty {
                div { class: "cal-share muted", "Nobody else yet." }
            }
            {rows}
        }
    }
}

#[component]
fn share_row(state: State, share: CalendarShare) -> NodeHandle {
    let (name, sub) = match &share.who {
        ShareWho::Person(a) => (a.clone(), String::new()),
        ShareWho::Group(a) => (a.clone(), "A group".to_owned()),
        ShareWho::Domain(d) => (share.who.says(), d.clone()),
        ShareWho::Anyone => (
            share.who.says(),
            "Anyone with the link can see it".to_owned(),
        ),
    };
    let role = share.role.word().to_owned();
    let (changed, stopped) = (share.clone(), share);
    rsx! {
        div { class: "cal-share",
            div { class: "cal-share-who",
                span { class: "cal-share-name", {name.clone()} }
                if !sub.is_empty() {
                    span { class: "event-sub", {sub.clone()} }
                }
            }
            Select {
                size: "sm",
                class: "cal-share-role",
                data: short_roles(),
                value_fn: move || role.clone(),
                onchange: move |v: String| {
                    if let Some(role) = CalendarRole::from_word(&v) {
                        state.change_share(changed.clone(), role);
                    }
                },
            }
            button {
                class: "icon-button cal-share-stop",
                aria-label: "Stop sharing with them",
                disabled: {move || state.panel.busy.get()},
                onclick: move || state.stop_share(stopped.clone()),
                "\u{d7}"
            }
        }
    }
}

pub const CSS: &str = r#"
.cal-pane-share, .cal-pane-theirs { display: flex; flex-direction: column; gap: 10px; }
.cal-pane-head {
    margin-top: 6px; font-size: 12px; letter-spacing: 0.06em; text-transform: uppercase; color: var(--muted);
}
.cal-shares { border: 1px solid var(--line); border-radius: 10px; display: flex; flex-direction: column; }
.cal-share { display: flex; align-items: center; gap: 10px; padding: 8px 10px; border-bottom: 1px solid var(--line-soft); }
.cal-share:last-child { border-bottom: 0; }
.cal-share.muted { color: var(--muted); }
.cal-share-who { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.cal-share-name { color: var(--ink); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.cal-share-stop { flex-shrink: 0; }
.cal-share-role { width: 140px; flex-shrink: 0; }
.cal-share-add { display: flex; gap: 8px; align-items: center; }
.cal-share-add > :first-child { flex: 1; min-width: 0; }
.event-line.muted { color: var(--muted); }
"#;
