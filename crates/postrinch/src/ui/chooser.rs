//! Choosing things a view looks for, or leaves out: labels, senders and
//! recipients all the same way. What is chosen shows as chips (Has in
//! blue; Not struck through); typing in the box below offers matches, each
//! with Has and Not. The callers say what is chosen and what matches.

use postrinch_commands::LabelPick;
use rinch::prelude::*;

/// Something offered while finding.
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    /// What choosing it writes into the search.
    pub value: String,
    /// What the row says.
    pub shown: String,
    /// Said faintly before it: a label's parents (`Clients/`).
    pub faint: String,
    /// A colour dot before it, as `background: …`.
    pub dot: Option<String>,
}

/// What a chooser is wired to.
#[derive(Clone, Copy)]
pub struct Wiring<C, O, A, S, F>
where
    C: Fn() -> Vec<(String, LabelPick)> + Copy + 'static,
    O: Fn(&str) -> Vec<Offer> + Copy + 'static,
    A: Fn(&str) -> Option<LabelPick> + Copy + 'static,
    S: Fn(String, Option<LabelPick>) + Copy + 'static,
    F: Fn(String) + Copy + 'static,
{
    /// What is chosen now, in order.
    pub chosen: C,
    /// What matches the text typed.
    pub offered: O,
    /// What is asked of one value now.
    pub asked: A,
    /// Ask this of a value, or nothing.
    pub set: S,
    /// Told of each change to the text typed.
    pub finding: F,
}

/// A panel row: its name, the chips, `extra` beside them (Labels' "any
/// of them / all of them"), the box, and the matches while finding.
pub fn chooser<C, O, A, S, F>(
    __scope: &mut RenderScope,
    name: &'static str,
    placeholder: &'static str,
    wiring: Wiring<C, O, A, S, F>,
    extra: Option<NodeHandle>,
) -> NodeHandle
where
    C: Fn() -> Vec<(String, LabelPick)> + Copy + 'static,
    O: Fn(&str) -> Vec<Offer> + Copy + 'static,
    A: Fn(&str) -> Option<LabelPick> + Copy + 'static,
    S: Fn(String, Option<LabelPick>) + Copy + 'static,
    F: Fn(String) + Copy + 'static,
{
    let find = Signal::new(String::new());
    let chosen = Memo::new(wiring.chosen);
    let offered = Memo::new(move || {
        let typed = find.get();
        let typed = typed.trim();
        if typed.is_empty() {
            Vec::new()
        } else {
            (wiring.offered)(typed)
        }
    });
    rsx! {
        div { class: "panel-field chooser",
            span { class: "panel-name", {name} }
            div { class: "chooser-body",
                div {
                    class: "chooser-chips",
                    style: {move || if chosen.get().is_empty() { "display: none" } else { "" }},
                    for (value, pick) in chosen.get() {
                        span { key: format!("{value}|{pick:?}"), class: "chip-slot",
                            {chip(__scope, wiring.set, value.clone(), pick)}
                        }
                    }
                    {extra}
                }
                TextInput {
                    size: "sm",
                    placeholder: placeholder,
                    value_fn: move || find.get(),
                    oninput: move |v: String| {
                        find.set(v.clone());
                        (wiring.finding)(v);
                    },
                }
                div {
                    class: "chooser-list",
                    style: {move || if offered.get().is_empty() { "display: none" } else { "" }},
                    for offer in offered.get() {
                        div { key: offer.value.clone(),
                            {row(__scope, wiring.asked, wiring.set, offer.clone())}
                        }
                    }
                }
            }
        }
    }
}

/// A chosen value, with the × that takes it off.
fn chip(
    __scope: &mut RenderScope,
    set: impl Fn(String, Option<LabelPick>) + Copy + 'static,
    value: String,
    pick: LabelPick,
) -> NodeHandle {
    let class = match pick {
        LabelPick::Has => "chooser-chip has",
        LabelPick::Not => "chooser-chip not",
    };
    let (title, label) = (format!("Take {value} off"), format!("Take {value} off"));
    let shown = value.clone();
    rsx! {
        span { class: class,
            span { class: "chooser-chip-name", {shown} }
            button {
                class: "chooser-x",
                title: title,
                aria-label: label,
                onclick: move || set(value.clone(), None),
                "\u{d7}"
            }
        }
    }
}

/// One match, with its Has and Not.
fn row(
    __scope: &mut RenderScope,
    asked: impl Fn(&str) -> Option<LabelPick> + Copy + 'static,
    set: impl Fn(String, Option<LabelPick>) + Copy + 'static,
    offer: Offer,
) -> NodeHandle {
    let now = {
        let value = offer.value.clone();
        Memo::new(move || asked(&value))
    };
    let button =
        |__scope: &mut RenderScope, pick: LabelPick, name: &'static str, on: &'static str| {
            let value = offer.value.clone();
            rsx! {
                button {
                    class: {move || if now.get() == Some(pick) { on } else { "" }},
                    aria-pressed: {move || if now.get() == Some(pick) { "true" } else { "false" }},
                    onclick: move || {
                        let again = now.get() == Some(pick);
                        set(value.clone(), (!again).then_some(pick));
                    },
                    {name}
                }
            }
        };
    let has = button(__scope, LabelPick::Has, "Has", "has");
    let not = button(__scope, LabelPick::Not, "Not", "not");
    let dot = offer
        .dot
        .clone()
        .map(|style| rsx! { span { class: "dot", style: {style.clone()} } });
    rsx! {
        div { class: "chooser-row",
            {dot}
            span { class: "name-text",
                span { class: "chooser-faint", {offer.faint.clone()} }
                {offer.shown.clone()}
            }
            div { class: "chooser-seg", {has} {not} }
        }
    }
}

pub const CSS: &str = r#"
.chooser { align-items: start; }
.chooser > .panel-name { line-height: 32px; }
.chooser-body { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.chooser-chips { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
.chooser-chip {
    height: 26px; padding: 0 4px 0 10px; border-radius: 13px; border: 1px solid;
    display: flex; align-items: center; gap: 4px; font-size: 13px;
}
.chooser-chip.has { background: var(--accent-tint); border-color: var(--notice-line); color: var(--accent-ink); }
.chooser-chip.not { background: var(--red-tint); border-color: var(--red-line); color: var(--red-ink); }
.chooser-chip.not .chooser-chip-name { text-decoration: line-through; }
.chooser-x {
    width: 20px; height: 20px; border: 0; border-radius: 10px; padding: 0;
    background: transparent; color: inherit; font: inherit; font-size: 14px; cursor: pointer;
}
.chooser-x:hover { background: var(--hover); }
.chooser-list { display: flex; flex-direction: column; max-height: 220px; overflow-y: auto; border: 1px solid var(--line); border-radius: 6px; padding: 4px; }
.chooser-row { display: flex; align-items: center; gap: 8px; height: 30px; padding: 0 4px 0 8px; border-radius: 5px; font-size: 13px; }
.chooser-row:hover { background: var(--row-hover); }
.chooser-row .name-text { flex: 1; }
.chooser-faint { color: var(--muted); }
.chooser-seg, .chooser-mode { display: flex; border: 1px solid var(--field); border-radius: 6px; overflow: hidden; flex-shrink: 0; }
.chooser-seg button, .chooser-mode button {
    height: 24px; padding: 0 10px; border: 0; background: transparent;
    color: var(--muted); font: inherit; font-size: 12px; cursor: pointer;
}
.chooser-seg button + button, .chooser-mode button + button { border-left: 1px solid var(--field); }
.chooser-seg button:hover, .chooser-mode button:hover { background: var(--hover); }
.chooser-seg .has, .chooser-mode .on { background: var(--accent-tint); color: var(--accent-ink); }
.chooser-seg .not { background: var(--red-tint); color: var(--red-ink); }
.chip-slot { display: contents; }
"#;
