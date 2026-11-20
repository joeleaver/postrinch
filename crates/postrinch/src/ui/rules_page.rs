//! Settings › Rules, after its mockup: an account's rules in the order
//! they run, each with its switch, and the filters made in Gmail itself.

use postrinch_rules::{Rule, summarize};
use rinch::prelude::*;

use crate::rule_draft::Draft;
use crate::rules_ui::RulesUi;
use crate::state::State;
use crate::ui::account_color;

#[component]
pub fn rules_page(state: State) -> NodeHandle {
    let ui = state.rules;
    let new_rule = move || {
        if let Some(account) = ui.account.get() {
            ui.edit(account, Draft::new());
        }
    };
    rsx! {
        main { class: "settings-main rules-main",
            div { class: "rules-head",
                h1 { class: "settings-heading", "Rules" }
                span { class: "spacer" }
                Button { variant: "filled", size: "xs", onclick: new_rule, "New rule" }
            }
            div { class: "rules-accounts",
                for (i, info) in state.accounts.get().into_iter().enumerate() {
                    div { key: info.account.address.key(),
                        {account_chip(__scope, state, info.account.address.clone(), i)}
                    }
                }
                span { class: "rules-order-hint", "Rules run top to bottom. Drag to reorder." }
            }
            div { class: "rules-list",
                for (i, rule) in rules(ui).into_iter().enumerate() {
                    div { key: format!("{}|{i}|{rule:?}", rule.id),
                        {rule_row(__scope, state, rule.clone(), i)}
                    }
                }
                if rules(ui).is_empty() {
                    p { class: "rules-empty",
                        "No rules yet. A rule files, labels or forwards mail as it arrives, or tidies it once it has been in the inbox a while."
                    }
                }
            }
            if !elsewhere(ui).is_empty() {
                div { class: "rules-elsewhere-head",
                    h2 { class: "settings-subheading", "Made in Gmail" }
                    span { class: "rules-elsewhere-hint",
                        "Filters set up in Gmail\u{2019}s own settings. They keep running; import one to change it here."
                    }
                }
            }
            if !elsewhere(ui).is_empty() {
                div { class: "rules-list",
                    for filter in elsewhere(ui) {
                        div { key: filter.id.clone(), class: "rule-row",
                            span { class: "rule-elsewhere", {filter.line.clone()} }
                            if filter.rule.is_some() {
                                Button {
                                    variant: "default",
                                    size: "xs",
                                    onclick: {
                                        let id = filter.id.clone();
                                        move || ui.import(id.clone())
                                    },
                                    "Import"
                                }
                            }
                        }
                    }
                }
            }
            {crate::ui::rule_run_dialog::run_dialog(__scope, state)}
            if ui.page.get().is_some_and(|p| p.server == Some(false)) {
                div { class: "rules-allow",
                    span {
                        "Let Gmail run your rules while Postrinch is closed. Google asks you to allow Postrinch to manage your mail filters."
                    }
                    Button {
                        variant: "default",
                        size: "xs",
                        onclick: move || state.sign_in(ui.account.get()),
                        "Allow"
                    }
                }
            }
        }
    }
}

fn rules(ui: RulesUi) -> Vec<Rule> {
    ui.page.get().map(|p| p.rules).unwrap_or_default()
}

fn elsewhere(ui: RulesUi) -> Vec<postrinch_rules::Elsewhere> {
    ui.page.get().map(|p| p.elsewhere).unwrap_or_default()
}

#[component]
fn account_chip(state: State, account: postrinch_model::EmailAddress, index: usize) -> NodeHandle {
    let ui = state.rules;
    let chosen = account.clone();
    let dot = format!("background: {}", account_color(index));
    let name = account.to_string();
    rsx! {
        button {
            class: {move || if ui.account.get().as_ref() == Some(&chosen) { "rules-chip chosen" } else { "rules-chip" }},
            onclick: move || {
                ui.account.set(Some(account.clone()));
                ui.load(std::slice::from_ref(&account));
            },
            span { class: "rules-chip-dot", style: {dot.clone()} }
            {name}
        }
    }
}

/// A label's name, or its id when the account no longer has it.
fn label_name(ui: RulesUi, id: &str) -> String {
    ui.labels.with(|labels| {
        labels
            .iter()
            .find(|l| l.label == id)
            .map_or_else(|| id.to_owned(), |l| l.name.clone())
    })
}

fn agent_name(ui: RulesUi, id: &str) -> String {
    ui.agents.with(|agents| {
        agents
            .iter()
            .find(|(a, _)| a == id)
            .map_or_else(|| "an agent".to_owned(), |(_, n)| n.clone())
    })
}

#[component]
fn rule_row(state: State, rule: Rule, index: usize) -> NodeHandle {
    let ui = state.rules;
    let line = summarize(&rule, &|l| label_name(ui, l), &|a| agent_name(ui, a));
    let refused = ui
        .page
        .get()
        .and_then(|p| p.refused.into_iter().find(|(id, _)| *id == rule.id))
        .map(|(_, why)| why);
    let (id, on) = (rule.id, rule.enabled);
    let open = rule.clone();
    let switch_label = format!("{} is {}", rule.name, if on { "on" } else { "off" });
    let grip_label = format!("Move {}", rule.name);
    let name = rule.name.clone();
    let run_label = format!("Run {} now", rule.name);
    let to_run = rule.clone();
    let run_it = move || {
        if let Some(account) = ui.account.get() {
            ui.run.ask(account, to_run.clone());
        }
    };
    let edit = move || {
        if let Some(account) = ui.account.get() {
            ui.edit(account, Draft::of(&open));
        }
    };
    rsx! {
        div {
            class: {move || {
                let over = ui.over.get() == Some(index) && ui.dragging.get().is_some_and(|d| d != id);
                match (over, on) {
                    (true, _) => "rule-row over",
                    (false, true) => "rule-row",
                    (false, false) => "rule-row off",
                }
            }},
            draggable: "true",
            ondragstart: move || ui.dragging.set(Some(id)),
            ondragenter: move || ui.over.set(Some(index)),
            ondragend: move || {
                ui.dragging.set(None);
                ui.over.set(None);
            },
            ondrop: move || ui.drop_at(index),
            span { class: "rule-grip", title: "Drag to reorder", aria-label: {grip_label.clone()},
                "\u{2807}"
            }
            div { class: "rule-text", onclick: edit,
                span { class: "rule-name", {name.clone()} }
                span { class: "rule-line", {line} }
                for why in refused.clone() {
                    span { key: why.clone(), class: "rule-refused", {why.clone()} }
                }
                for said in ui.run.said_of(ui.account.get().as_ref(), id) {
                    span {
                        key: format!("{said:?}"),
                        class: if said.running { "rule-ran" } else { "rule-ran done" },
                        {said.text.clone()}
                    }
                }
            }
            button {
                class: "rule-run",
                aria-label: {run_label.clone()},
                disabled: {move || ui.account.get().is_some_and(|a| ui.run.running(&a, id))},
                onclick: run_it,
                "Run now\u{2026}"
            }
            button {
                class: if on { "rule-switch on" } else { "rule-switch" },
                aria-label: {switch_label.clone()},
                aria-pressed: if on { "true" } else { "false" },
                onclick: move || ui.set_on(id, !on),
                span { class: "rule-knob" }
            }
        }
    }
}

pub const CSS: &str = r#"
.rules-main { max-width: 880px; display: flex; flex-direction: column; gap: 14px; }
.rules-head { display: flex; align-items: center; gap: 12px; }
.rules-head .settings-heading { margin: 0; }
.rules-accounts { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.rules-chip {
    height: 28px; padding: 0 12px; display: flex; align-items: center; gap: 6px;
    border: 1px solid var(--field); border-radius: 14px; background: transparent;
    color: var(--ink-2); font-size: 13px; cursor: pointer;
}
.rules-chip.chosen { background: var(--accent-tint); border-color: var(--notice-line); color: var(--accent-ink); }
.rules-chip-dot { width: 7px; height: 7px; border-radius: 2px; }
.rules-order-hint { margin-left: 8px; font-size: 13px; color: var(--muted); }
.rules-list { border: 1px solid var(--line); border-radius: 10px; background: var(--panel); display: flex; flex-direction: column; }
.rules-empty { margin: 0; padding: 16px; color: var(--muted); }
.rule-row { display: flex; align-items: center; gap: 12px; padding: 10px 12px; border-bottom: 1px solid var(--line-soft); }
.rule-row:last-child { border-bottom: none; }
.rule-row.over { box-shadow: inset 0 2px 0 var(--accent); }
.rule-grip { width: 20px; color: var(--muted); cursor: grab; font-size: 16px; text-align: center; }
.rule-text { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; cursor: pointer; }
.rule-name { font-weight: 600; }
.rule-row.off .rule-name { color: var(--muted); }
.rule-line { font-size: 13px; color: var(--muted); }
.rule-refused { font-size: 12px; color: var(--trouble); }
.rule-elsewhere { flex: 1; font-size: 13px; color: var(--ink-2); }
.rule-switch {
    width: 34px; height: 20px; padding: 2px; border: none; border-radius: 10px;
    background: var(--field); display: flex; justify-content: flex-start; cursor: pointer;
}
.rule-switch.on { background: var(--accent); justify-content: flex-end; }
.rule-knob { width: 16px; height: 16px; border-radius: 8px; background: #ffffff; }
.rules-elsewhere-head { display: flex; align-items: baseline; gap: 10px; margin-top: 6px; }
.rules-elsewhere-head .settings-subheading { margin: 0; }
.rules-elsewhere-hint { font-size: 13px; color: var(--muted); }
.rules-allow {
    display: flex; align-items: center; gap: 12px; padding: 10px 12px;
    border: 1px solid var(--line); border-radius: 10px; font-size: 13px; color: var(--ink-2);
}
.rules-allow span { flex: 1; }
"#;
