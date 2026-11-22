//! The calendar's month: six weeks of days, each with its first events
//! (a timed one with its start) and how many more; a click on a day's
//! number opens that day.

use jiff::tz::TimeZone;
use postrinch_model::EventWhen;
use rinch::prelude::*;

use crate::calendar_layout::{Column, columns};
use crate::calendar_ui::{CalSpan, stretch, today};
use crate::state::State;
use crate::ui::calendar_view::{all_day_chip, look, shown, summary, tint};

/// Events a day shows before "N more".
const MOST: usize = 3;

fn weeks(state: State) -> Vec<Column> {
    let (first, _) = stretch(state.calendar.day.get(), CalSpan::Month);
    columns(&shown(state), first, 42, &TimeZone::system())
}

#[component]
pub fn month(state: State) -> NodeHandle {
    rsx! {
        div { class: "month",
            div { class: "month-names",
                for name in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] {
                    span { key: name, class: "month-name", {name} }
                }
            }
            div { class: "month-grid",
                for col in weeks(state) {
                    div { key: cell_key(col), class: "month-slot",
                        {day_cell(__scope, state, col.clone())}
                    }
                }
            }
        }
    }
}

#[component]
fn day_cell(state: State, col: Column) -> NodeHandle {
    let shown_month = state.calendar.day.get().month();
    let day = col.day;
    let class = if day == today() {
        "month-day today"
    } else if day.month() != shown_month {
        "month-day other"
    } else {
        "month-day"
    };
    let mut all: Vec<_> = col.all_day.clone();
    all.extend(col.timed.iter().map(|p| p.happening.clone()));
    let more = all.len().saturating_sub(MOST);
    let first: Vec<_> = all.into_iter().take(MOST).collect();
    let tz = TimeZone::system();
    let lines: Vec<NodeHandle> = first
        .into_iter()
        .map(|h| match h.event.when {
            EventWhen::Days { .. } => all_day_chip(__scope, state, h),
            EventWhen::Times { start, .. } => {
                let (colour, class) = look(state, &h);
                let at = jiff::Timestamp::from_millisecond(start.as_millis())
                    .map(|t| t.to_zoned(tz.clone()).strftime("%H:%M").to_string())
                    .unwrap_or_default();
                let text = format!("{at} {}", summary(&h));
                let tip = text.clone();
                let style = format!("--cal-colour: {colour}; --cal-tint: {}", tint(&colour));
                let open = h.clone();
                rsx! {
                    div {
                        class: {format!("{class} chip timed")},
                        style: {style.clone()},
                        title: {tip.clone()},
                        onclick: move || state.open_event(Some(open.clone())),
                        {text.clone()}
                    }
                }
            }
        })
        .collect();
    let more_line = (more > 0).then(|| {
        rsx! {
            button { class: "link-button month-more",
                onclick: move || state.calendar_go(day, Some(CalSpan::Day)),
                {format!("{more} more")}
            }
        }
    });
    rsx! {
        div { class: {class},
            button { class: "month-num",
                onclick: move || state.calendar_go(day, Some(CalSpan::Day)),
                {day.day().to_string()}
            }
            {lines}
            {more_line}
        }
    }
}

/// A day's key: the day and what is in it, so it is built again only
/// when that changes.
fn cell_key(col: &Column) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}{:?}", col.all_day, col.timed).hash(&mut h);
    (col.day == today()).hash(&mut h);
    format!("{}|{:x}", col.day, h.finish())
}

pub const CSS: &str = r#"
.month { flex: 1; min-height: 0; display: flex; flex-direction: column; }
.month-names { display: grid; grid-template-columns: repeat(7, minmax(0, 1fr)); border-bottom: 1px solid var(--line); }
.month-name { padding: 6px 8px; font-size: 12px; color: var(--muted); }
.month-grid {
    flex: 1; min-height: 0; display: grid;
    grid-template-columns: repeat(7, minmax(0, 1fr)); grid-template-rows: repeat(6, minmax(0, 1fr));
}
.month-slot { display: flex; min-height: 0; min-width: 0; }
.month-day {
    flex: 1;
    border-left: 1px solid var(--line-soft); border-bottom: 1px solid var(--line-soft);
    padding: 4px; display: flex; flex-direction: column; gap: 2px; min-height: 0; overflow: hidden;
}
.month-day.other .month-num { color: var(--faint); }
.month-num {
    align-self: flex-start; min-width: 24px; height: 24px; border: 0; border-radius: 12px;
    background: transparent; color: var(--ink); font: inherit; font-size: 13px; font-weight: 600; cursor: pointer;
}
.month-num:hover { background: var(--hover); }
.month-day.today .month-num { background: var(--accent); color: var(--surface); }
.cal-event.chip.timed { border-left-width: 3px; }
.month-more { align-self: flex-start; font-size: 12px; }
"#;
