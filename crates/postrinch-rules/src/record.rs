//! What is kept about an account's rules: the settings every computer
//! shares (kept in the account, in the settings message), and what this
//! computer alone knows.

use std::collections::BTreeMap;

use postrinch_model::Timestamp;
use serde::{Deserialize, Serialize};

use crate::gmail::{self, Context, Filter, Part};
use crate::{RuleSet, When};

/// The rules and the filters made for them, as the settings message in the
/// account holds them, so every computer runs the same rules and knows
/// which of Gmail's filters are Postrinch's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub rules: RuleSet,
    /// The filters Postrinch made, by Gmail's id.
    #[serde(default)]
    pub made: Vec<Made>,
    /// Rules Gmail would not take, and why, in a sentence.
    #[serde(default)]
    pub refused: BTreeMap<u32, String>,
    /// Filters made in Gmail itself and imported as rules, by the rule
    /// they became: each is deleted once Gmail has that rule's filters.
    #[serde(default)]
    pub retire: Vec<(u32, String)>,
}

/// A filter Postrinch made in Gmail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Made {
    pub id: String,
    pub filter: Filter,
    /// When it was made: mail that arrived earlier was not filtered by it.
    pub since: Timestamp,
}

/// A filter someone made in Gmail itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Elsewhere {
    pub id: String,
    /// What it does, in a line: Gmail's own words for it.
    pub line: String,
    /// The rule it becomes when imported (id 0, for saving as new).
    #[serde(default)]
    pub rule: Option<crate::Rule>,
}

/// What this computer knows about running an account's rules.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Local {
    /// Mail received up to here has been through the rules.
    #[serde(default)]
    pub cursor: Option<Timestamp>,
    /// Messages after nothing more than their body, before the rules can
    /// decide (by provider id).
    #[serde(default)]
    pub pending: Vec<String>,
    /// When rules for mail that has been in the inbox a while last ran.
    #[serde(default)]
    pub waited_at: Option<Timestamp>,
    /// The settings message the settings were read from or written as.
    #[serde(default)]
    pub message: Option<String>,
    /// The rules changed here and the account doesn't have them yet.
    #[serde(default)]
    pub dirty: bool,
    /// Whether the account lets Postrinch make filters, when last asked.
    #[serde(default)]
    pub server: Option<bool>,
    /// The filters made in Gmail itself, when last listed.
    #[serde(default)]
    pub elsewhere: Vec<Elsewhere>,
    /// Addresses Gmail has verified for forwarding, when last listed.
    #[serde(default)]
    pub forwarding: Vec<String>,
    /// Rules agents proposed, waiting for the person to save or discard.
    #[serde(default)]
    pub proposed: Vec<Proposed>,
}

/// A rule an agent proposed: one that forwards mail or asks an agent,
/// which only the person may save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposed {
    /// Unique among the account's proposals.
    pub id: u32,
    /// The rule, its id 0 for a new one.
    pub rule: crate::Rule,
    /// The agent's name.
    pub agent: String,
    /// Why, in the agent's words.
    pub reason: Option<String>,
}

/// What Postrinch has to do itself for each rule, given what Gmail
/// actually has: a rule whose filters are not all made yet (just saved,
/// or Gmail refused them) runs here whole.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Running {
    pub parts: Vec<(u32, Part)>,
    /// For rules Gmail runs: since when. Mail that arrived before goes
    /// through the rule here.
    pub since: BTreeMap<u32, Timestamp>,
}

impl Running {
    pub fn part(&self, rule: u32) -> Option<&Part> {
        self.parts
            .iter()
            .find(|(id, _)| *id == rule)
            .map(|(_, p)| p)
    }
}

impl Settings {
    /// The plan as Gmail should have it.
    pub fn plan(&self, server: bool, others: usize) -> gmail::Plan {
        gmail::compile(
            &self.rules,
            &Context {
                server,
                refused: self.refused.keys().copied().collect(),
                others,
            },
        )
    }

    /// What Postrinch runs, against the filters Gmail has.
    pub fn running(&self, server: bool, others: usize) -> Running {
        let plan = self.plan(server, others);
        let mut running = Running::default();
        for rule in self.rules.rules.iter().filter(|r| r.enabled) {
            let mut wanted: Vec<&Filter> =
                plan.filters.iter().filter(|f| f.rule == rule.id).collect();
            let mut have: Vec<&Made> = self
                .made
                .iter()
                .filter(|m| m.filter.rule == rule.id)
                .collect();
            let local = plan.part(rule.id).cloned();
            if wanted.is_empty() {
                if let Some(part) = local {
                    running.parts.push((rule.id, part));
                }
                continue;
            }
            wanted.sort();
            have.sort_by(|a, b| a.filter.cmp(&b.filter));
            let made: Vec<&Filter> = have.iter().map(|m| &m.filter).collect();
            if rule.when == When::Arrives && made == wanted {
                if let Some(since) = have.iter().map(|m| m.since).max() {
                    running.since.insert(rule.id, since);
                }
                if let Some(part) = local {
                    running.parts.push((rule.id, part));
                }
            } else {
                running.parts.push((rule.id, Part::Whole));
            }
        }
        running
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, Condition, Rule, Test};

    fn settings() -> Settings {
        let rule = Rule {
            conditions: vec![Condition::is(Test::FromAddress("a@example.com".into()))],
            actions: vec![
                Action::Star,
                Action::AskAgent {
                    agent: "x".into(),
                    prompt: "p".into(),
                },
            ],
            ..Rule::new(1, "R")
        };
        Settings {
            rules: RuleSet {
                rules: vec![rule],
                next_id: 2,
            },
            ..Settings::default()
        }
    }

    #[test]
    fn a_rule_runs_here_whole_until_its_filters_are_made() {
        let mut s = settings();
        assert_eq!(s.running(true, 0).part(1), Some(&Part::Whole));
        for (i, filter) in s.plan(true, 0).filters.into_iter().enumerate() {
            s.made.push(Made {
                id: format!("f{i}"),
                filter,
                since: Timestamp::from_millis(500),
            });
        }
        let running = s.running(true, 0);
        assert!(matches!(running.part(1), Some(Part::AfterMarker(_))));
        assert_eq!(running.since.get(&1), Some(&Timestamp::from_millis(500)));
        // Changed since: the filters made no longer match.
        s.rules.rules[0].actions.remove(0);
        assert_eq!(s.running(true, 0).part(1), Some(&Part::Whole));
    }

    #[test]
    fn settings_read_back() {
        let s = settings();
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), s);
        assert_eq!(
            serde_json::from_str::<Local>("{}").unwrap(),
            Local::default()
        );
    }
}
