//! The event editor in the calendar's right pane, after the calendar
//! mockup: what, the day and hours (or whole days), the calendar, where,
//! the guests and notes. Save says when it sends the guests word.

use postrinch_model::EmailAddress;
use rinch::prelude::*;

use crate::calendar_edit::Form;
use crate::calendar_ui::hide_key;
use crate::state::State;

/// One field of the form, read.
fn get(state: State, read: impl Fn(&Form) -> String) -> String {
    state
        .calendar
        .editing
        .with(|e| e.as_ref().map(&read).unwrap_or_default())
}

#[component]
pub fn editor(state: State) -> NodeHandle {
    let mut writable: Vec<_> = state
        .calendar
        .calendars
        .get()
        .into_iter()
        .filter(|(_, c)| c.writable)
        .collect();
    // Each account's own calendar first, then by name.
    writable.sort_by(|(a, x), (b, y)| {
        (a.to_string(), !x.primary, x.name.clone()).cmp(&(
            b.to_string(),
            !y.primary,
            y.name.clone(),
        ))
    });
    let calendars: Vec<SelectOption> = writable
        .into_iter()
        .map(|(a, c)| {
            let label = if c.name == a.to_string() {
                c.name.clone()
            } else {
                format!("{} \u{b7} {a}", c.name)
            };
            SelectOption::new(hide_key(&a, &c.id), label)
        })
        .collect();
    let all_day = Memo::new(move || {
        state
            .calendar
            .editing
            .with(|e| e.as_ref().is_some_and(|f| f.all_day))
    });
    let new = state
        .calendar
        .editing
        .with(|e| e.as_ref().is_some_and(|f| f.event.is_none()));
    rsx! {
        div { class: "event-editor",
            span { class: "editor-head", {if new { "New event" } else { "Change the event" }} }
            label { class: "editor-field",
                span { class: "editor-label", "What" }
                TextInput {
                    value_fn: move || get(state, |f| f.summary.clone()),
                    oninput: move |v: String| state.edit_form(|f| f.summary = v),
                }
            }
            if all_day.get() {
                div { class: "editor-row",
                    label { class: "editor-field",
                        span { class: "editor-label", "From" }
                        TextInput { placeholder: "2026-11-25",
                            value_fn: move || get(state, |f| f.day.clone()),
                            oninput: move |v: String| state.edit_form(|f| f.day = v) }
                    }
                    label { class: "editor-field",
                        span { class: "editor-label", "To" }
                        TextInput { placeholder: "2026-11-25",
                            value_fn: move || get(state, |f| f.last_day.clone()),
                            oninput: move |v: String| state.edit_form(|f| f.last_day = v) }
                    }
                }
            } else {
                div { class: "editor-row",
                    label { class: "editor-field wide",
                        span { class: "editor-label", "Day" }
                        TextInput { placeholder: "2026-11-25",
                            value_fn: move || get(state, |f| f.day.clone()),
                            oninput: move |v: String| state.edit_form(|f| f.day = v) }
                    }
                    label { class: "editor-field",
                        span { class: "editor-label", "From" }
                        TextInput { placeholder: "14:00",
                            value_fn: move || get(state, |f| f.from.clone()),
                            oninput: move |v: String| state.edit_form(|f| f.from = v) }
                    }
                    label { class: "editor-field",
                        span { class: "editor-label", "To" }
                        TextInput { placeholder: "15:00",
                            value_fn: move || get(state, |f| f.to.clone()),
                            oninput: move |v: String| state.edit_form(|f| f.to = v) }
                    }
                }
            }
            Checkbox {
                label: "All day",
                checked_fn: move || all_day.get(),
                onchange: move || state.edit_form(|f| f.all_day = !f.all_day),
            }
            div { class: "editor-field",
                span { class: "editor-label", "Calendar" }
                Select {
                    data: calendars,
                    value_fn: move || get(state, |f| hide_key(&f.account, &f.calendar)),
                    onchange: move |v: String| pick_calendar(state, &v),
                }
            }
            label { class: "editor-field",
                span { class: "editor-label", "Where" }
                TextInput {
                    value_fn: move || get(state, |f| f.location.clone()),
                    oninput: move |v: String| state.edit_form(|f| f.location = v),
                }
            }
            label { class: "editor-field",
                span { class: "editor-label", "Guests" }
                TextInput {
                    placeholder: "Ada <ada@example.org>, bob@example.org",
                    value_fn: move || get(state, |f| f.guests.clone()),
                    oninput: move |v: String| state.edit_form(|f| f.guests = v),
                }
            }
            label { class: "editor-field grow",
                span { class: "editor-label", "Notes" }
                Textarea {
                    value_fn: move || get(state, |f| f.notes.clone()),
                    oninput: move |v: String| state.edit_form(|f| f.notes = v),
                }
            }
            div { class: "editor-error",
                {move || get(state, |f| f.error.clone().unwrap_or_default())}
            }
            div { class: "editor-buttons",
                Button { variant: "default", onclick: move || state.cancel_edit(), "Cancel" }
                Button {
                    variant: "filled",
                    disabled: {move || state.calendar.editing.with(|e| e.as_ref().is_some_and(|f| f.busy))},
                    onclick: move || state.save_event(),
                    {move || state.calendar.editing.with(|e| e.as_ref().map_or("Save", Form::save_label))}
                }
            }
        }
    }
}

/// The calendar chosen, by [`hide_key`].
fn pick_calendar(state: State, key: &str) {
    let found = state
        .calendar
        .calendars
        .get()
        .into_iter()
        .find(|(a, c)| hide_key(a, &c.id) == key);
    if let Some((account, info)) = found {
        let account: EmailAddress = account;
        state.edit_form(|f| {
            f.account = account;
            f.calendar = info.id;
        });
    }
}

pub const CSS: &str = r#"
.event-editor { padding: 20px; display: flex; flex-direction: column; gap: 12px; min-height: 100%; box-sizing: border-box; }
.editor-head { font-size: 12px; letter-spacing: 0.06em; text-transform: uppercase; color: var(--muted); }
.editor-field { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
.editor-row .editor-field { flex: 1; }
.editor-row .editor-field.wide { flex: 1.6; }
.editor-field.grow { flex: 1; }
.editor-field input, .editor-field textarea { width: 100%; min-width: 0; box-sizing: border-box; }
.editor-field.grow textarea { min-height: 120px; }
.editor-label { font-size: 12px; color: var(--muted); }
.editor-row { display: flex; gap: 8px; }
.editor-error { color: var(--danger); font-size: 13px; min-height: 18px; }
.editor-buttons { display: flex; gap: 8px; justify-content: flex-end; }
"#;
