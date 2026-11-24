//! The rules calls' answers. An agent may make, change, switch, move and
//! delete rules as the person can on the Rules page, with one difference:
//! a rule that forwards mail out of the account or asks an agent is only
//! proposed, and the person saves it or not in the rule editor.

use postrinch_commands::{Commands, LabelChoice};
use postrinch_model::{EmailAddress, Timestamp};
use postrinch_rules::{Action, Rule, Test, When};
use serde_json::{Value, json};
use std::sync::atomic::AtomicUsize;

use crate::later::{self, Finished};
use crate::registry::Agents;
use crate::rule_shape::{Names, RuleShape};
use crate::views::{Asker, say};
use crate::wire::Call;
use crate::wire::Changed;

pub(crate) fn run(commands: &Commands, call: Call, asker: &Asker) -> Result<Value, String> {
    let (agent, agents) = (asker.agent, asker.agents);
    match call {
        Call::Rules { account } => list(commands, &account, agents),
        Call::SaveRule {
            account,
            rule_id: id,
            rule,
            apply_to_existing,
            reason,
        } => {
            let names = AccountNames::new(commands, &account, agents)?;
            save(
                commands,
                &account,
                id,
                &rule,
                &names,
                apply_to_existing,
                (agent, reason),
            )
        }
        Call::DeleteRule {
            account,
            rule_id: id,
        } => {
            known(commands, &account, id)?;
            commands.delete_rule(&account, id).map_err(say)?;
            Ok(json!({ "deleted": id }))
        }
        Call::SetRuleOn {
            account,
            rule_id: id,
            on,
        } => {
            known(commands, &account, id)?;
            commands.set_rule_on(&account, id, on).map_err(say)?;
            Ok(json!({ "rule": id, "on": on }))
        }
        Call::MoveRule {
            account,
            rule_id: id,
            to,
        } => {
            known(commands, &account, id)?;
            commands.move_rule(&account, id, to).map_err(say)?;
            list(commands, &account, agents)
        }
        Call::RunRule {
            account,
            rule_id: id,
            all,
        } => {
            let rule = known(commands, &account, id)?;
            let work = {
                let commands = commands.clone();
                move |_: &AtomicUsize| {
                    let run = if all {
                        commands.apply_to_all_existing(&account, &rule, now())
                    } else {
                        commands.apply_to_existing(&account, &rule, now())
                    };
                    run.map_err(say)
                }
            };
            Ok(match later::within(asker.on_change, Changed::Mail, work)? {
                Finished::Done(n) => json!({ "rule": id, "changed": n }),
                Finished::Going(_) => json!({
                    "rule": id,
                    "running": true,
                    "note": "Postrinch is still running the rule over the mail and will finish by itself; the window shows the changes when done.",
                }),
            })
        }
        _ => unreachable!("only rules calls come here"),
    }
}

fn list(commands: &Commands, account: &EmailAddress, agents: &Agents) -> Result<Value, String> {
    let page = commands.rules_page(account).map_err(say)?;
    let names = AccountNames::new(commands, account, agents)?;
    let summary = |rule: &Rule| {
        postrinch_rules::summarize(rule, &|id| names.label_name(id), &|id| names.agent_name(id))
    };
    let rules: Vec<Value> = page
        .rules
        .iter()
        .map(|rule| {
            let refused = page
                .refused
                .iter()
                .find(|(id, _)| *id == rule.id)
                .map(|(_, why)| why.clone());
            json!({
                "id": rule.id,
                "on": rule.enabled,
                "summary": summary(rule),
                "runs_in": if page.in_gmail.contains(&rule.id) { "gmail" } else { "postrinch" },
                "gmail_refused": refused,
                "rule": RuleShape::of(rule, &names),
            })
        })
        .collect();
    let proposed: Vec<Value> = commands
        .rule_proposals()
        .map_err(say)?
        .into_iter()
        .filter(|p| &p.account == account)
        .map(|p| {
            json!({
                "agent": p.proposed.agent,
                "reason": p.proposed.reason,
                "summary": summary(&p.proposed.rule),
            })
        })
        .collect();
    Ok(json!({
        "account": account,
        "rules": rules,
        // Gmail runs a rule only when Postrinch may make filters there;
        // otherwise Postrinch runs it while it is open.
        "gmail_filters_allowed": page.server,
        "gmail_filters": page.elsewhere.iter().map(|e| e.line.clone()).collect::<Vec<_>>(),
        "forwarding_addresses": page.forwarding,
        "proposed": proposed,
    }))
}

fn save(
    commands: &Commands,
    account: &EmailAddress,
    id: Option<u32>,
    shape: &RuleShape,
    names: &AccountNames,
    apply_to_existing: bool,
    (agent, reason): (&str, Option<String>),
) -> Result<Value, String> {
    let before = match id {
        Some(id) => Some(known(commands, account, id)?),
        None => None,
    };
    let mut rule = shape.to_rule(id.unwrap_or(0), names)?;
    if let Some(before) = &before {
        rule.enabled = before.enabled;
    }
    // As in the editor: Gmail forwards only to addresses verified in its
    // own settings.
    let verified = commands.rules_page(account).map_err(say)?.forwarding;
    for action in &rule.actions {
        if let Action::Forward(to) = action
            && !verified.iter().any(|v| v.eq_ignore_ascii_case(to))
        {
            return Err(if verified.is_empty() {
                format!(
                    "{account} has no address verified for forwarding. The person adds one in Gmail's settings (Forwarding and POP/IMAP) first."
                )
            } else {
                format!(
                    "Gmail forwards only to addresses verified in its settings: {}.",
                    verified.join(", ")
                )
            });
        }
    }
    if rule.when == When::Waited
        && rule
            .conditions
            .iter()
            .any(|c| matches!(c.test, Test::GmailSearch(_)))
    {
        return Err("A rule that waits is run by Postrinch, which can't run gmail_search: use the other conditions (category, label, in_inbox, from...).".into());
    }
    if needs_person(&rule, before.as_ref()) {
        commands
            .propose_rule(account, rule, agent, reason)
            .map_err(say)?;
        return Ok(json!({
            "proposed": true,
            "note": "A rule that forwards mail or asks an agent is the person's to save: it waits for them in Postrinch's sidebar. Nothing changes until they save it; they may change it or discard it.",
        }));
    }
    let id = commands.save_rule(account, rule.clone()).map_err(say)?;
    let mut answer = json!({ "saved": id });
    if apply_to_existing {
        let saved = Rule { id, ..rule };
        let changed = commands
            .apply_to_existing(account, &saved, now())
            .map_err(say)?;
        answer["changed_now"] = json!(changed);
    }
    Ok(answer)
}

/// Whether saving `rule` is the person's to do: it forwards mail or asks
/// an agent, and that is new, or now happens to different mail.
fn needs_person(rule: &Rule, before: Option<&Rule>) -> bool {
    let sensitive = |r: &Rule| -> Vec<Action> {
        r.actions
            .iter()
            .filter(|a| matches!(a, Action::Forward(_) | Action::AskAgent { .. }))
            .cloned()
            .collect()
    };
    let now = sensitive(rule);
    if now.is_empty() {
        return false;
    }
    !before.is_some_and(|b| {
        sensitive(b) == now
            && b.when == rule.when
            && b.matching == rule.matching
            && b.conditions == rule.conditions
    })
}

/// The account's rule `id`.
fn known(commands: &Commands, account: &EmailAddress, id: u32) -> Result<Rule, String> {
    commands
        .rules_page(account)
        .map_err(say)?
        .rules
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| format!("{account} has no rule {id}. list_rules shows the ones it has."))
}

/// An account's labels and the agents a rule can ask.
struct AccountNames {
    account: EmailAddress,
    labels: Vec<LabelChoice>,
    /// Agents connected to Claude, by id and name: the ones a rule can
    /// start.
    agents: Vec<(String, String)>,
}

impl AccountNames {
    fn new(commands: &Commands, account: &EmailAddress, agents: &Agents) -> Result<Self, String> {
        Ok(Self {
            account: account.clone(),
            labels: commands.label_choices(account).map_err(say)?,
            agents: crate::connect::askable(agents)
                .into_iter()
                .map(|a| (a.id, a.name))
                .collect(),
        })
    }
}

impl Names for AccountNames {
    fn label_id(&self, name: &str) -> Result<String, String> {
        self.labels
            .iter()
            .find(|c| c.label == name)
            .or_else(|| {
                self.labels
                    .iter()
                    .find(|c| c.name.eq_ignore_ascii_case(name))
            })
            .map(|c| c.label.clone())
            .ok_or_else(|| {
                format!(
                    "{} has no label {name:?}. list_labels shows the ones it has.",
                    self.account
                )
            })
    }

    fn label_name(&self, id: &str) -> String {
        self.labels
            .iter()
            .find(|c| c.label == id)
            .map_or_else(|| id.to_owned(), |c| c.name.clone())
    }

    fn agent_id(&self, name: &str) -> Result<String, String> {
        self.agents
            .iter()
            .find(|(id, n)| id == name || n.eq_ignore_ascii_case(name))
            .map(|(id, _)| id.clone())
            .ok_or_else(|| {
                let known: Vec<&str> = self.agents.iter().map(|(_, n)| n.as_str()).collect();
                if known.is_empty() {
                    "A rule can ask only an agent connected to Claude from Postrinch, and there is none."
                        .to_owned()
                } else {
                    format!(
                        "A rule can ask only an agent connected to Claude from Postrinch: {}.",
                        known.join(", ")
                    )
                }
            })
    }

    fn agent_name(&self, id: &str) -> String {
        self.agents
            .iter()
            .find(|(i, _)| i == id)
            .map_or_else(|| id.to_owned(), |(_, n)| n.clone())
    }
}

fn now() -> Timestamp {
    Timestamp::from_millis(jiff::Timestamp::now().as_millisecond())
}

#[cfg(test)]
mod tests {
    use super::*;
    use postrinch_rules::{Condition, Test};

    fn rule(actions: Vec<Action>) -> Rule {
        Rule {
            conditions: vec![Condition::is(Test::FromAddress("a@example.com".into()))],
            actions,
            ..Rule::new(1, "R")
        }
    }

    #[test]
    fn forwarding_and_agents_are_the_persons_to_save() {
        let forward = Action::Forward("me@example.com".into());
        assert!(!needs_person(&rule(vec![Action::Star]), None));
        assert!(needs_person(&rule(vec![forward.clone()]), None));
        // Renaming a forwarding rule the person saved, or adding a label.
        let before = rule(vec![forward.clone()]);
        let mut after = rule(vec![forward.clone(), Action::Star]);
        after.name = "Renamed".into();
        assert!(!needs_person(&after, Some(&before)));
        // Forwarding different mail.
        after.conditions[0].test = Test::FromDomain("example.com".into());
        assert!(needs_person(&after, Some(&before)));
    }
}
