//! The calendar's middle pane, after the calendar mockup: a bar over it
//! (Today, back, on, the stretch's name, Day / Week / Month, + Event), then
//! a day or a week as columns of hours with whole-day events in a band on
//! top, or a month (calendar_month.rs). A click on an event opens it in the
//! pane on the right.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use jiff::tz::TimeZone;
use postrinch_commands::Happening;
use postrinch_model::Answer;
use rinch::prelude::*;

use crate::calendar_layout::{Column, HOUR, Placed, columns, now_top};
use crate::calendar_ui::{CalSpan, hide_key, stretch, today};
use crate::state::State;

#[component]
pub fn calendar_view(state: State) -> NodeHandle {
    rsx! {
        div { class: "cal-main",
            {bar(__scope, state)}
            if state.calendar.span.get() == CalSpan::Month {
                {crate::ui::calendar_month::month(__scope, state)}
            } else {
                {days(__scope, state)}
            }
        }
    }
}

#[component]
fn bar(state: State) -> NodeHandle {
    let day_b = seg(__scope, state, CalSpan::Day, "Day");
    let week_b = seg(__scope, state, CalSpan::Week, "Week");
    let month_b = seg(__scope, state, CalSpan::Month, "Month");
    rsx! {
        div { class: "cal-bar",
            button { class: "cal-button", onclick: move || state.calendar_go(today(), None), "Today" }
            button { class: "cal-button square", aria-label: "Back", onclick: move || state.calendar_step(false), "\u{2039}" }
            button { class: "cal-button square", aria-label: "On", onclick: move || state.calendar_step(true), "\u{203a}" }
            span { class: "cal-title", {move || title(state)} }
            div { class: "segs", {day_b} {week_b} {month_b} }
            button { class: "cal-new", onclick: move || state.new_event(), "+ Event" }
        }
    }
}

#[component]
fn seg(state: State, span: CalSpan, label: &'static str) -> NodeHandle {
    rsx! {
        button {
            class: {move || if state.calendar.span.get() == span { "seg on" } else { "seg" }},
            onclick: move || state.set_calendar_span(span),
            {label}
        }
    }
}

/// The stretch's name: "Tuesday 24 November 2026", "23 – 29 November
/// 2026", "November 2026".
fn title(state: State) -> String {
    let day = state.calendar.day.get();
    let span = state.calendar.span.get();
    let (first, last) = stretch(day, span);
    match span {
        CalSpan::Day => day.strftime("%A %-d %B %Y").to_string(),
        CalSpan::Month => day.strftime("%B %Y").to_string(),
        CalSpan::Week if first.month() == last.month() => {
            format!("{} \u{2013} {}", first.day(), last.strftime("%-d %B %Y"))
        }
        CalSpan::Week if first.year() == last.year() => format!(
            "{} \u{2013} {}",
            first.strftime("%-d %b"),
            last.strftime("%-d %b %Y")
        ),
        CalSpan::Week => format!(
            "{} \u{2013} {}",
            first.strftime("%-d %b %Y"),
            last.strftime("%-d %b %Y")
        ),
    }
}

/// What shows: the events of calendars not hidden, each once though two
/// accounts follow its calendar (the holidays, a shared calendar).
pub fn shown(state: State) -> Vec<Happening> {
    let hidden = state.calendar.hidden.get();
    let mut seen = std::collections::HashSet::new();
    state
        .calendar
        .happenings
        .get()
        .into_iter()
        .filter(|h| !hidden.contains(&hide_key(&h.account, &h.event.calendar)))
        .filter(|h| seen.insert((h.event.calendar.clone(), h.event.id.clone())))
        .collect()
}

/// Each calendar's colour, by [`hide_key`].
pub fn colours(state: State) -> HashMap<String, String> {
    state
        .calendar
        .calendars
        .get()
        .into_iter()
        .map(|(a, c)| {
            let colour = c.color.unwrap_or_else(|| "#6f9cf0".to_owned());
            (hide_key(&a, &c.id), colour)
        })
        .collect()
}

fn day_columns(state: State) -> Vec<Column> {
    let span = state.calendar.span.get();
    let (first, last) = stretch(state.calendar.day.get(), span);
    let n = first.until(last).map(|s| s.get_days()).unwrap_or(0) + 1;
    columns(&shown(state), first, i64::from(n), &TimeZone::system())
}

/// A column's key: its day and what is in it, so a column is built again
/// only when its events change.
fn column_key(c: &Column) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for h in c.all_day.iter().chain(c.timed.iter().map(|p| &p.happening)) {
        format!("{:?}", h.event).hash(&mut hasher);
    }
    for p in &c.timed {
        (p.lane, p.lanes).hash(&mut hasher);
    }
    format!("{}|{:x}", c.day, hasher.finish())
}

#[component]
fn days(state: State) -> NodeHandle {
    let hours: Vec<String> = (0..24).map(|h| format!("{h:02}:00")).collect();
    let grid = rsx! {
        div { class: "cal-scroll",
            div { class: "cal-hours-grid",
                div { class: "cal-gutter",
                    for hour in hours.clone() {
                        span { key: hour.clone(), class: "cal-hour", {hour.clone()} }
                    }
                }
                for col in day_columns(state) {
                    div { key: column_key(col), class: "cal-col-slot",
                        {day_column(__scope, state, col.clone())}
                    }
                }
            }
        }
    };
    // The working day in view, not midnight.
    grid.set_scroll_top(7.5 * HOUR);
    rsx! {
        div { class: "cal-days",
            div { class: "cal-head",
                span { class: "cal-gutter-head" }
                for col in day_columns(state) {
                    div { key: column_key(col), class: "cal-day-head",
                        span { class: "cal-weekday", {col.day.strftime("%a").to_string()} }
                        span {
                            class: {if col.day == today() { "cal-daynum today" } else { "cal-daynum" }},
                            {col.day.day().to_string()}
                        }
                        div { class: "cal-allday",
                            for h in col.all_day.clone() {
                                {all_day_chip(__scope, state, h.clone())}
                            }
                        }
                    }
                }
            }
            {grid}
        }
    }
}

#[component]
fn day_column(state: State, col: Column) -> NodeHandle {
    let style = column_style(&col);
    let is_today = col.day == today();
    let blocks: Vec<NodeHandle> = col
        .timed
        .iter()
        .map(|p| event_block(__scope, state, p.clone()))
        .collect();
    rsx! {
        div { class: "cal-col", style: {style.clone()},
            {blocks}
            if is_today {
                div { class: "cal-now", style: {move || { state.revision.get(); format!("top: {}px", now_top(&TimeZone::system())) }} }
            }
        }
    }
}

fn column_style(col: &Column) -> String {
    use jiff::civil::Weekday::{Saturday, Sunday};
    let weekend = matches!(col.day.weekday(), Saturday | Sunday);
    format!(
        "height: {}px{}",
        24.0 * HOUR,
        if weekend {
            "; background: var(--row-hover)"
        } else {
            ""
        }
    )
}

/// How an event looks: its calendar's colour; an invitation not yet
/// answered dashed; one declined faint and struck through.
pub fn look(state: State, h: &Happening) -> (String, &'static str) {
    let colour = colours(state)
        .get(&hide_key(&h.account, &h.event.calendar))
        .cloned()
        .unwrap_or_else(|| "#6f9cf0".to_owned());
    let class = match (h.event.guest, h.event.answer) {
        (true, None) => "cal-event waiting",
        (_, Some(Answer::No)) => "cal-event declined",
        _ => "cal-event",
    };
    (colour, class)
}

#[component]
fn event_block(state: State, placed: Placed) -> NodeHandle {
    let (colour, class) = look(state, &placed.happening);
    // Too short for two lines: one, the time after the name.
    let short = placed.height < 40.0;
    let class = if short {
        format!("{class} short")
    } else {
        class.to_owned()
    };
    let width = 100.0 / placed.lanes as f64;
    let style = format!(
        "top: {}px; height: {}px; left: calc({}% + 2px); width: calc({}% - 4px); --cal-colour: {colour}; --cal-tint: {}",
        placed.top,
        placed.height - 2.0,
        width * placed.lane as f64,
        width,
        tint(&colour),
    );
    let tz = TimeZone::system();
    let hours = crate::dates::event_hours(&placed.happening.event.when, &tz);
    let summary = summary(&placed.happening);
    let tip = format!("{summary}, {hours}");
    let time_text = if short {
        format!("\u{b7} {hours}")
    } else {
        hours.clone()
    };
    let open = placed.happening.clone();
    let selected = placed.happening.clone();
    rsx! {
        div {
            key: placed.happening.event.id.clone(),
            class: {move || {
                let on = state.calendar.open.get().is_some_and(|o| o.account == selected.account && o.event.id == selected.event.id);
                if on { format!("{class} selected") } else { class.clone() }
            }},
            style: {style.clone()},
            title: {tip.clone()},
            onclick: move || state.open_event(Some(open.clone())),
            span { class: "cal-event-title", {summary.clone()} }
            span { class: "cal-event-time", {time_text.clone()} }
        }
    }
}

#[component]
pub fn all_day_chip(state: State, h: Happening) -> NodeHandle {
    let (colour, class) = look(state, &h);
    let summary = summary(&h);
    let tip = summary.clone();
    let style = format!("--cal-colour: {colour}; --cal-tint: {}", tint(&colour));
    let open = h.clone();
    rsx! {
        div {
            key: h.event.id.clone(),
            class: {format!("{class} chip")},
            style: {style.clone()},
            title: {tip.clone()},
            onclick: move || state.open_event(Some(open.clone())),
            {summary.clone()}
        }
    }
}

/// A calendar's colour, faint, for an event's ground: `#rrggbb` at a
/// quarter.
pub fn tint(colour: &str) -> String {
    let hex = colour.trim_start_matches('#');
    let channel =
        |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("80"), 16).unwrap_or(128);
    format!("rgba({}, {}, {}, 0.25)", channel(0), channel(2), channel(4))
}

pub fn summary(h: &Happening) -> String {
    h.event
        .summary
        .clone()
        .unwrap_or_else(|| "(no title)".to_owned())
}

pub const CSS: &str = r#"
.cal-button, .seg, .cal-new, .mode, .mini-day, .mini-step, .month-num, .invite-answer {
    display: flex; align-items: center; justify-content: center;
}
.cal-main { flex: 1; min-width: 0; display: flex; flex-direction: column; background: var(--surface); }
.cal-bar {
    height: 52px; flex-shrink: 0; box-sizing: border-box; padding: 0 16px;
    display: flex; align-items: center; gap: 8px; border-bottom: 1px solid var(--line);
}
.cal-button {
    height: 30px; padding: 0 12px; border: 1px solid var(--field); border-radius: 6px;
    background: var(--surface); color: var(--ink); font: inherit; cursor: pointer;
}
.cal-button.square { width: 30px; padding: 0; font-size: 16px; }
.cal-button:hover { background: var(--hover); }
.cal-title { font-size: 16px; font-weight: 600; color: var(--ink); margin-left: 4px; }
.segs { margin-left: auto; display: flex; gap: 2px; padding: 3px; border-radius: 8px; background: var(--panel); }
.seg {
    height: 26px; padding: 0 10px; border: 0; border-radius: 6px;
    background: transparent; color: var(--muted); font: inherit; cursor: pointer;
}
.seg.on { background: var(--hover); color: var(--ink); }
.cal-new {
    height: 30px; padding: 0 12px; border: 0; border-radius: 6px;
    background: var(--accent); color: var(--surface); font: inherit; font-weight: 600; cursor: pointer;
}
.cal-days { flex: 1; min-height: 0; display: flex; flex-direction: column; }
.cal-head { display: flex; border-bottom: 1px solid var(--line); flex-shrink: 0; }
.cal-gutter-head { width: 56px; flex-shrink: 0; }
.cal-day-head {
    flex: 1; min-width: 0; padding: 8px 4px 6px; border-left: 1px solid var(--line-soft);
    display: flex; flex-direction: column; gap: 2px;
}
.cal-weekday { font-size: 12px; color: var(--muted); padding-left: 2px; }
.cal-daynum { font-size: 18px; font-weight: 600; height: 30px; display: flex; align-items: center; padding-left: 2px; color: var(--ink); }
.cal-daynum.today {
    width: 30px; justify-content: center; padding: 0; border-radius: 15px;
    background: var(--accent); color: var(--surface);
}
.cal-allday { display: flex; flex-direction: column; gap: 2px; min-height: 20px; }
.cal-scroll { flex: 1; min-height: 0; overflow-y: auto; }
.cal-hours-grid { display: flex; }
.cal-gutter { width: 56px; flex-shrink: 0; display: flex; flex-direction: column; }
.cal-hour {
    height: 48px; flex-shrink: 0; box-sizing: border-box; padding-right: 8px;
    text-align: right; font-size: 11px; color: var(--faint); transform: translateY(-7px);
}
.cal-col-slot { flex: 1; min-width: 0; display: flex; }
.cal-col {
    flex: 1; min-width: 0; position: relative; border-left: 1px solid var(--line-soft);
    background-image: repeating-linear-gradient(to bottom, var(--line-soft) 0, var(--line-soft) 1px, transparent 1px, transparent 48px);
}
.cal-now { position: absolute; left: 0; right: 0; height: 2px; background: var(--danger); z-index: 2; }
.cal-event {
    position: absolute; box-sizing: border-box; padding: 3px 6px; border-radius: 6px;
    border-left: 3px solid var(--cal-colour);
    background: var(--cal-tint);
    color: var(--ink); font-size: 12px; overflow: hidden; cursor: pointer;
    display: flex; flex-direction: column; gap: 1px;
}
.cal-event:hover { box-shadow: 0 0 0 1px var(--cal-colour); }
.cal-event.waiting { border: 2px dashed var(--cal-colour); background: var(--surface); }
.cal-event.declined { opacity: 0.55; text-decoration: line-through; }
.cal-event.selected { box-shadow: 0 0 0 2px var(--ink); z-index: 3; }
.cal-event.chip {
    position: static; padding: 2px 6px; border-radius: 4px;
    white-space: nowrap; text-overflow: ellipsis; display: block;
}
.cal-event.short { flex-direction: row; gap: 4px; padding: 1px 6px; align-items: center; }
.cal-event.short .cal-event-title { flex-shrink: 1; min-width: 0; }
.cal-event-title { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.cal-event-time { opacity: 0.85; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
"#;
