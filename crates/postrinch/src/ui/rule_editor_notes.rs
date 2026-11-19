//! What the rule editor says beside the rule: an agent's proposal and its
//! reason, and what saving a rule that forwards will do.

use rinch::prelude::*;

use crate::rule_draft::Draft;
use crate::rules_ui::RulesUi;
use crate::ui::rule_editor::draft_get;

/// Above an agent's proposed rule: who, and that nothing happens yet.
pub fn banner(__scope: &mut RenderScope, agent: &str) -> NodeHandle {
    let title = format!("{agent} proposed this rule");
    rsx! {
        div { class: "compose-banner",
            span { class: "compose-banner-title", {title} }
            span { "Change anything you like. Nothing happens until you press Save." }
        }
    }
}

/// The agent's reason, in its words.
pub fn reason(__scope: &mut RenderScope, reason: String) -> NodeHandle {
    rsx! {
        div { class: "compose-aside",
            span { class: "compose-aside-title", "The agent\u{2019}s reason, in its words" }
            span { class: "compose-reason", {reason} }
        }
    }
}

pub fn forward_text(ui: RulesUi) -> String {
    let rule = draft_get(ui, Draft::rule, postrinch_rules::Rule::new(0, ""));
    let to: Vec<String> = rule
        .actions
        .iter()
        .filter_map(|a| match a {
            postrinch_rules::Action::Forward(to) => Some(to.clone()),
            _ => None,
        })
        .collect();
    let line = summarize_conditions(&rule);
    format!(
        "Every message {line} will be forwarded to {} as it arrives, without asking each time.",
        to.join(" and ")
    )
}

fn summarize_conditions(rule: &postrinch_rules::Rule) -> String {
    let whole = postrinch_rules::summarize(rule, &|l| l.to_owned(), &|a| a.to_owned());
    let conditions = whole.split(" \u{2192} ").next().unwrap_or("").to_owned();
    let mut chars = conditions.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}
