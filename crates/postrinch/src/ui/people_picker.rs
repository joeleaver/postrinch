//! Senders or recipients to look for, as labels are (label_picker.rs):
//! each one the mail is from (Has: any of them) or not (Not), chosen from
//! the people mail was from or to, found as a name or address is typed,
//! or as the typed text itself (a domain, say). They write into the
//! view's search (`from:`, `-from:`, `{from:a from:b}`; `to:` likewise).

use postrinch_commands::{Filters, PersonEntry};
use rinch::prelude::*;

use crate::state::State;
use crate::ui::view_panel_mail::{change, filters};

/// Which people a picker chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    From,
    To,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::From => "From",
            Side::To => "To",
        }
    }

    /// The filter's lists for this side: any of, and none of.
    fn lists(self, f: &mut Filters) -> (&mut Vec<String>, &mut Vec<String>) {
        match self {
            Side::From => (&mut f.from, &mut f.not_from),
            Side::To => (&mut f.to, &mut f.not_to),
        }
    }
}

/// Has or Not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    Has,
    Not,
}

/// Ask `pick` of `who` on `side`, or nothing.
fn set(state: State, side: Side, who: String, pick: Option<Pick>) {
    change(state, move |f| {
        let (has, not) = side.lists(f);
        let same = |p: &String| p.eq_ignore_ascii_case(&who);
        has.retain(|p| !same(p));
        not.retain(|p| !same(p));
        match pick {
            Some(Pick::Has) => has.push(who),
            Some(Pick::Not) => not.push(who),
            None => {}
        }
    });
}

/// What `side` asks of `who` now.
fn asked(state: State, side: Side, who: &str) -> Option<Pick> {
    let mut f = filters(state);
    let (has, not) = side.lists(&mut f);
    if has.iter().any(|p| p.eq_ignore_ascii_case(who)) {
        Some(Pick::Has)
    } else if not.iter().any(|p| p.eq_ignore_ascii_case(who)) {
        Some(Pick::Not)
    } else {
        None
    }
}

#[component]
pub fn people_picker(state: State, side: Side) -> NodeHandle {
    let find = Signal::new(String::new());
    let chosen = Memo::new(move || {
        let mut f = filters(state);
        let (has, not) = side.lists(&mut f);
        let mut all: Vec<(String, Pick)> = has.drain(..).map(|p| (p, Pick::Has)).collect();
        all.extend(not.drain(..).map(|p| (p, Pick::Not)));
        all
    });
    // What was found for this picker's text: the typed text itself first,
    // then the people found by it (once the loader answers for it).
    let offered = Memo::new(move || {
        let typed = find.get().trim().to_owned();
        if typed.is_empty() {
            return Vec::new();
        }
        let mut rows = vec![(typed.clone(), format!("Anyone matching \u{201c}{typed}\u{201d}"))];
        state.people.with(|(part, people)| {
            if part.trim() == typed {
                rows.extend(people.iter().map(|p: &PersonEntry| {
                    let shown = match &p.name {
                        Some(name) => format!("{name} \u{2039}{}\u{203a}", p.address),
                        None => p.address.clone(),
                    };
                    (p.address.clone(), shown)
                }));
            }
        });
        rows
    });
    rsx! {
        div { class: "panel-field panel-people",
            span { class: "panel-name", {side.name()} }
            div { class: "people",
                div {
                    class: "picker-chips",
                    style: {move || if chosen.get().is_empty() { "display: none" } else { "" }},
                    for (who, pick) in chosen.get() {
                        div { key: format!("{who}|{pick:?}"), class: "picker-chip-slot",
                            {chip(__scope, state, side, who.clone(), pick)}
                        }
                    }
                }
                TextInput {
                    placeholder: "Add a name, address or domain",
                    value_fn: move || find.get(),
                    oninput: move |v: String| {
                        find.set(v.clone());
                        state.people_find.set(v);
                    },
                }
                div {
                    class: "people-list",
                    style: {move || if offered.get().is_empty() { "display: none" } else { "" }},
                    for (who, shown) in offered.get() {
                        div { key: who.clone(),
                            {person_row(__scope, state, side, who.clone(), shown.clone())}
                        }
                    }
                }
            }
        }
    }
}

/// A chosen sender or recipient, to take off.
#[component]
fn chip(state: State, side: Side, who: String, pick: Pick) -> NodeHandle {
    let class = match pick {
        Pick::Has => "picker-chip has",
        Pick::Not => "picker-chip not",
    };
    let (title, label) = (format!("Take {who} off"), format!("Take {who} off"));
    rsx! {
        span { class: class,
            span { class: "picker-chip-name", {who.clone()} }
            button {
                class: "picker-x",
                title: title,
                aria-label: label,
                onclick: move || set(state, side, who.clone(), None),
                "×"
            }
        }
    }
}

/// One person (or the typed text) offered, with Has and Not.
#[component]
fn person_row(state: State, side: Side, who: String, shown: String) -> NodeHandle {
    let now = {
        let who = who.clone();
        Memo::new(move || asked(state, side, &who))
    };
    let button = |__scope: &mut RenderScope, pick: Pick, name: &'static str| {
        let who = who.clone();
        let on = match pick {
            Pick::Has => "has",
            Pick::Not => "not",
        };
        rsx! {
            button {
                class: {move || if now.get() == Some(pick) { on } else { "" }},
                aria-pressed: {move || if now.get() == Some(pick) { "true" } else { "false" }},
                onclick: move || {
                    let again = now.get() == Some(pick);
                    set(state, side, who.clone(), (!again).then_some(pick));
                },
                {name}
            }
        }
    };
    let (has, not) = (button(__scope, Pick::Has, "Has"), button(__scope, Pick::Not, "Not"));
    rsx! {
        div { class: "people-row",
            span { class: "name-text", {shown} }
            div { class: "picker-seg", {has} {not} }
        }
    }
}

pub const CSS: &str = r#"
.panel-people { align-items: start; }
.panel-people .panel-name { line-height: 32px; }
.people { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.people-list { display: flex; flex-direction: column; max-height: 220px; overflow-y: auto; border: 1px solid var(--line); border-radius: 6px; padding: 4px; }
.people-row { display: flex; align-items: center; gap: 8px; height: 30px; padding: 0 4px 0 8px; border-radius: 5px; font-size: 13px; }
.people-row:hover { background: var(--row-hover); }
.people-row .name-text { flex: 1; }
"#;
