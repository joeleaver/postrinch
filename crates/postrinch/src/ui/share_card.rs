//! A calendar shared with the person, at the top of Google's mail saying
//! so (the ShareMail board of the calendar mockup): the calendar, who
//! shared it, and Add calendar; once added, Show in Calendar. Only for
//! mail Google itself signed (postrinch_render::calendar_share).

use postrinch_commands::may_share_calendar;
use postrinch_model::{EmailAddress, Message};
use rinch::prelude::*;

use crate::calendar_shares::{ShareState, Shared};
use crate::state::State;

#[component]
pub fn share_card(state: State, message: Message) -> NodeHandle {
    if !may_share_calendar(&message) {
        return rsx! { span {} };
    }
    let id = message.id.0.clone();
    let account = message.account.clone();
    rsx! {
        div {
            for shared in state.invitations.shares.get().get(&id).cloned() {
                div { key: format!("{:?}", shared.state),
                    {card(__scope, state, account.clone(), id.clone(), shared.clone())}
                }
            }
        }
    }
}

#[component]
fn card(state: State, account: EmailAddress, id: String, shared: Shared) -> NodeHandle {
    let calendar = shared.calendar.id.clone();
    let title = match &shared.state {
        ShareState::Added(name) => name.clone(),
        _ => calendar.clone(),
    };
    let by = match &shared.calendar.by {
        Some(by) => format!("{by} shared it with {account}"),
        None => format!("Shared with {account}"),
    };
    let foot = match shared.state {
        ShareState::Offered => {
            let (account, id) = (account.clone(), id.clone());
            rsx! {
                div { class: "invite-answers",
                    Button { variant: "filled", size: "sm",
                        onclick: move || state.add_shared_calendar(&account, &id),
                        "Add calendar" }
                }
            }
        }
        ShareState::Adding => rsx! {
            div { class: "invite-answers", span { class: "invite-going", "Adding it\u{2026}" } }
        },
        ShareState::Added(_) => {
            let (account, calendar) = (account.clone(), calendar.clone());
            rsx! {
                div { class: "invite-answers",
                    span { class: "invite-going", {format!("In {}\u{2019}s calendars.", account.clone())} }
                    button { class: "link-button",
                        onclick: move || {
                            state.show_calendar(true);
                            state.open_calendar_panel(&account, &calendar);
                        },
                        "Show in Calendar" }
                }
            }
        }
        ShareState::Failed(why) => {
            let (account, id) = (account.clone(), id.clone());
            rsx! {
                div { class: "invite-answers",
                    span { class: "invite-line clash", {why} }
                    Button { variant: "default", size: "sm",
                        onclick: move || state.add_shared_calendar(&account, &id),
                        "Try again" }
                }
            }
        }
    };
    rsx! {
        div { class: "invite",
            div { class: "share-icon",
                svg {
                    xmlns: "http://www.w3.org/2000/svg",
                    viewBox: "0 0 24 24",
                    width: "24",
                    height: "24",
                    fill: "none",
                    stroke: "currentColor",
                    stroke-width: "1.8",
                    stroke-linecap: "round",
                    stroke-linejoin: "round",
                    aria-hidden: "true",
                    path { d: "M3 5h18v16H3zM3 10h18M8 3v4M16 3v4" }
                }
            }
            div { class: "invite-main",
                div { class: "invite-title", span { {title.clone()} } }
                div { class: "invite-line", {by.clone()} }
                {foot}
            }
        }
    }
}

pub const CSS: &str = r#"
.share-icon {
    width: 48px; height: 48px; flex-shrink: 0; border-radius: 8px;
    display: flex; align-items: center; justify-content: center;
    background: var(--hover); color: var(--accent-ink);
}
"#;
