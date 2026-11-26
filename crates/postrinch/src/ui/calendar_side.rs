//! The calendar's sidebar, after the calendar mockup: a small month (a
//! click shows that day's stretch) and each account's calendars, each with
//! a tick to show or hide it, its colour, and its name (a click opens it
//! in the right pane).

use jiff::civil::Date;
use postrinch_model::{CalendarInfo, EmailAddress};
use rinch::prelude::*;

use crate::calendar_ui::{CalSpan, hide_key, stretch, today};
use crate::state::State;

#[component]
pub fn calendar_sidebar(state: State) -> NodeHandle {
    rsx! {
        nav {
            class: "sidebar",
            style: {move || format!("width: {}px", state.sidebar_width.get())},
            {crate::ui::outgoing::outgoing_lines(__scope, state)}
            {mini_month(__scope, state)}
            div { class: "cal-list",
                for (account, calendars) in by_account(state) {
                    div { key: format!("{}|{}", account.key(), calendars.len()), class: "cal-account",
                        span { class: "cal-account-name", {account.to_string()} }
                        for info in calendars.clone() {
                            {calendar_row(__scope, state, account.clone(), info.clone())}
                        }
                    }
                }
            }
        }
    }
}

fn by_account(state: State) -> Vec<(EmailAddress, Vec<CalendarInfo>)> {
    let mut out: Vec<(EmailAddress, Vec<CalendarInfo>)> = Vec::new();
    for (account, info) in state.calendar.calendars.get() {
        match out.iter_mut().find(|(a, _)| *a == account) {
            Some((_, list)) => list.push(info),
            None => out.push((account, vec![info])),
        }
    }
    // The account's own calendar first, then by name.
    for (_, list) in &mut out {
        list.sort_by(|a, b| b.primary.cmp(&a.primary).then(a.name.cmp(&b.name)));
    }
    out
}

#[component]
fn calendar_row(state: State, account: EmailAddress, info: CalendarInfo) -> NodeHandle {
    let key = hide_key(&account, &info.id);
    let shown_key = key.clone();
    let dot = format!(
        "background: {}",
        info.color.clone().unwrap_or_else(|| "var(--accent)".into())
    );
    let id = info.id.clone();
    let (opens, open_id) = (account.clone(), info.id.clone());
    let (on_account, on_id) = (account.clone(), info.id.clone());
    rsx! {
        div {
            key: key.clone(),
            class: {move || {
                let on = state.panel.open.get().is_some_and(|(a, c)| a == on_account && c == on_id);
                if on { "cal-row on" } else { "cal-row" }
            }},
            Checkbox {
                size: "sm",
                checked_fn: move || !state.calendar.hidden.get().contains(&shown_key),
                onchange: move || state.toggle_calendar(&account, &id),
            }
            span { class: "cal-dot", style: {dot.clone()} }
            span {
                class: "cal-name",
                onclick: move || state.open_calendar_panel(&opens, &open_id),
                {info.name.clone()}
            }
        }
    }
}

/// One day of the small month.
#[derive(Clone, PartialEq)]
struct Cell {
    day: Date,
    class: &'static str,
}

fn cells(state: State) -> Vec<Cell> {
    let shown = state.calendar.day.get();
    let (from, to) = stretch(shown, state.calendar.span.get());
    let (first, _) = stretch(shown, CalSpan::Month);
    let today = today();
    (0..42)
        .filter_map(|i| first.checked_add(jiff::Span::new().days(i)).ok())
        .map(|day| {
            let class = if day == today {
                "mini-day today"
            } else if day >= from && day <= to {
                "mini-day in"
            } else if day.month() != shown.month() {
                "mini-day other"
            } else {
                "mini-day"
            };
            Cell { day, class }
        })
        .collect()
}

#[component]
fn mini_month(state: State) -> NodeHandle {
    rsx! {
        div { class: "mini-month",
            div { class: "mini-head",
                span { class: "mini-title", {move || state.calendar.day.get().strftime("%B %Y").to_string()} }
                button { class: "mini-step", aria-label: "Last month",
                    onclick: move || month_step(state, false), "\u{2039}" }
                button { class: "mini-step", aria-label: "Next month",
                    onclick: move || month_step(state, true), "\u{203a}" }
            }
            div { class: "mini-grid",
                for name in ["M", "T", "W", "T", "F", "S", "S"].iter().enumerate().map(|(i, n)| (i, *n)).collect::<Vec<_>>() {
                    span { key: name.0, class: "mini-weekday", {name.1} }
                }
                for cell in cells(state) {
                    button {
                        key: format!("{}|{}", cell.day, cell.class),
                        class: {cell.class},
                        onclick: move || state.calendar_go(cell.day, None),
                        {cell.day.day().to_string()}
                    }
                }
            }
        }
    }
}

fn month_step(state: State, on: bool) {
    let day = state.calendar.day.get();
    let step = jiff::Span::new().months(1);
    let moved = if on {
        day.checked_add(step)
    } else {
        day.checked_sub(step)
    };
    if let Ok(moved) = moved {
        state.calendar_go(moved, None);
    }
}

pub const CSS: &str = r#"
.mini-month { display: flex; flex-direction: column; gap: 6px; padding: 0 4px; }
.mini-head { display: flex; align-items: center; gap: 2px; }
.mini-title { flex: 1; font-weight: 600; font-size: 13px; color: var(--ink); }
.mini-step {
    width: 24px; height: 24px; border: 0; border-radius: 6px;
    background: transparent; color: var(--muted); cursor: pointer; font-size: 15px;
}
.mini-step:hover { background: var(--hover); }
.mini-grid { display: grid; grid-template-columns: repeat(7, minmax(0, 1fr)); gap: 1px; }
.mini-weekday { text-align: center; font-size: 11px; color: var(--faint); padding: 2px 0; }
.mini-day {
    height: 24px; border: 0; border-radius: 5px; padding: 0;
    background: transparent; color: var(--ink-2); font: inherit; font-size: 12px; cursor: pointer;
}
.mini-day:hover { background: var(--hover); }
.mini-day.other { color: var(--faint); }
.mini-day.in { background: var(--hover); color: var(--ink); }
.mini-day.today { background: var(--accent); color: var(--surface); font-weight: 600; }
.cal-list { display: flex; flex-direction: column; gap: 10px; overflow-y: auto; min-height: 0; }
.cal-account { display: flex; flex-direction: column; gap: 1px; }
.cal-account-name {
    font-size: 11px; letter-spacing: 0.06em; text-transform: uppercase;
    color: var(--muted); padding: 0 8px 4px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.cal-row {
    display: flex; align-items: center; gap: 8px; padding: 4px 8px;
    border-radius: 6px; color: var(--ink-2); font-size: 13px; cursor: pointer;
}
.cal-row:hover { background: var(--hover); }
.cal-row.on { background: var(--accent-tint); color: var(--ink); }
.cal-dot { width: 10px; height: 10px; border-radius: 3px; flex-shrink: 0; }
.cal-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
"#;
