//! Running rules here: over mail that arrived since the last run, over
//! mail Gmail marked for Postrinch to finish, and, every so often, over the
//! inbox for rules about mail that has been there a while
//! (docs/rules.md, 3 and 5).

use std::collections::BTreeSet;

use postrinch_model::{
    EmailAddress, Label, Message, MessageChange, ProviderMessageId, ProviderThreadId, SystemLabel,
    Timestamp,
};
use postrinch_rules::gmail::Part;
use postrinch_rules::matching::{Verdict, matches};
use postrinch_rules::{Action, Category, Rule, When};
use postrinch_store::{Find, Store};
use serde::{Deserialize, Serialize};

use crate::{CommandResult, Commands};

/// How often rules about mail that has been in the inbox a while run.
const WAITED_EVERY_MILLIS: i64 = 60 * 60 * 1000;
/// Labels mail sits under that rules leave alone.
const LEFT_ALONE: [&str; 3] = [
    SystemLabel::Drafts.provider_id(),
    SystemLabel::Spam.provider_id(),
    SystemLabel::Trash.provider_id(),
];
const INBOX: &str = SystemLabel::Inbox.provider_id();
const IMPORTANT: &str = SystemLabel::Important.provider_id();

/// An agent to ask about a message, as a rule said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAsk {
    pub account: EmailAddress,
    pub thread: ProviderThreadId,
    pub message: ProviderMessageId,
    pub rule: u32,
    pub agent: String,
    pub prompt: String,
}

/// What a run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ran {
    /// Messages changed.
    pub changed: usize,
    /// Agents to ask, for the window to start.
    pub asks: Vec<AgentAsk>,
}

/// Changes to make, grouped so that messages changed alike go to the
/// provider in one entry.
#[derive(Default)]
pub(crate) struct Changes(Vec<(MessageChange, Vec<ProviderMessageId>)>);

impl Changes {
    pub(crate) fn add(&mut self, message: &Message, change: MessageChange) {
        match self.0.iter_mut().find(|(c, _)| *c == change) {
            Some((_, ids)) => ids.push(message.id.clone()),
            None => self.0.push((change, vec![message.id.clone()])),
        }
    }

    pub(crate) fn apply(self, store: &Store, account: &EmailAddress) -> CommandResult<()> {
        for (change, ids) in self.0 {
            store.change_messages(account, &ids, &change)?;
        }
        Ok(())
    }
}

/// The change `actions` make to `message`. Labels the account no longer
/// has are left out; forwarding and agents are not changes.
pub(crate) fn change_for(actions: &[Action], message: &Message, labels: &[Label]) -> MessageChange {
    let has = |id: &str| labels.iter().any(|l| l.provider_id == id);
    let mut change = MessageChange::default();
    let add = |c: &mut MessageChange, id: &str| {
        if has(id) && !message.has_label(id) {
            c.add_labels.push(id.to_owned());
        }
    };
    for action in actions {
        match action {
            Action::AddLabel(id) => add(&mut change, id),
            Action::SkipInbox if message.has_label(INBOX) => {
                change.remove_labels.push(INBOX.into());
            }
            Action::MarkRead if message.flags.unread => change.unread = Some(false),
            Action::Star if !message.flags.starred => change.starred = Some(true),
            Action::MarkImportant => add(&mut change, IMPORTANT),
            Action::NeverImportant if message.has_label(IMPORTANT) => {
                change.remove_labels.push(IMPORTANT.into());
            }
            Action::Category(category) => {
                add(&mut change, category.label_id());
                for other in Category::ALL.iter().filter(|c| *c != category) {
                    if message.has_label(other.label_id()) {
                        change.remove_labels.push(other.label_id().into());
                    }
                }
            }
            Action::Trash => add(&mut change, SystemLabel::Trash.provider_id()),
            // Gmail only, as mail arrives; and sending is Gmail's to do.
            Action::NeverSpam | Action::Forward(_) => {}
            Action::AskAgent { .. } => {}
            _ => {}
        }
    }
    change
}

fn asks_for(rule: &Rule, actions: &[Action], message: &Message) -> Vec<AgentAsk> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::AskAgent { agent, prompt } => Some(AgentAsk {
                account: message.account.clone(),
                thread: message.thread.clone(),
                message: message.id.clone(),
                rule: rule.id,
                agent: agent.clone(),
                prompt: prompt.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Whether rules look at `message` as arriving mail.
fn arrived(message: &Message) -> bool {
    if LEFT_ALONE.iter().any(|l| message.has_label(l)) {
        return false;
    }
    // Mail this account sent, unless it also came in.
    !message.has_label(SystemLabel::Sent.provider_id()) || message.has_label(INBOX)
}

impl Commands {
    /// Run the account's rules over what is new since the last run, at
    /// `now`. The first run on this computer only notes where to start.
    pub fn run_rules(&self, account: &EmailAddress, now: Timestamp) -> CommandResult<Ran> {
        self.check(account)?;
        let (settings, local) = self.store.rules(account)?;
        let Some(cursor) = local.cursor else {
            self.store
                .update_rules(account, |_, l| l.cursor = Some(now))?;
            return Ok(Ran::default());
        };
        let enabled: Vec<&Rule> = settings.rules.rules.iter().filter(|r| r.enabled).collect();
        let running = settings.running(local.server.unwrap_or(false), local.elsewhere.len());
        let labels = self.store.labels(account)?;
        let mut ran = Ran::default();
        let mut changes = Changes::default();
        let act = |rule: &Rule,
                   actions: &[Action],
                   message: &Message,
                   ran: &mut Ran,
                   changes: &mut Changes| {
            let change = change_for(actions, message, &labels);
            if !change.is_empty() {
                ran.changed += 1;
                changes.add(message, change);
            }
            ran.asks.extend(asks_for(rule, actions, message));
        };

        // New mail, oldest first, with what waited for its body.
        let find = Find {
            accounts: vec![account.clone()],
            after: Some(Timestamp::from_millis(cursor.as_millis() + 1)),
            ..Find::default()
        };
        let mut ids: Vec<ProviderMessageId> = local
            .pending
            .iter()
            .map(|id| ProviderMessageId(id.clone()))
            .collect();
        let mut found = self.store.find_messages(&find)?;
        found.reverse();
        ids.extend(found.into_iter().map(|f| f.id));
        let mut seen = BTreeSet::new();
        let mut pending = Vec::new();
        let mut reached = cursor;
        for id in ids {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(message) = self.store.message(account, &id)? else {
                continue;
            };
            reached = reached.max(message.received);
            if !arrived(&message) {
                continue;
            }
            match self.decide(&enabled, &running, &message, now) {
                None => pending.push(id.0.clone()),
                Some(todo) => {
                    for (rule, actions) in todo {
                        act(rule, &actions, &message, &mut ran, &mut changes);
                    }
                }
            }
        }

        // Mail Gmail marked for Postrinch to finish, whatever its age.
        for rule in &enabled {
            let Some(Part::AfterMarker(actions)) = running.part(rule.id) else {
                continue;
            };
            let marker = rule.marker();
            let Some(label) = labels.iter().find(|l| l.name == marker) else {
                continue;
            };
            for entry in self.store.label_messages(account, &label.provider_id)? {
                let Some(message) = self.store.message(account, &entry.message)? else {
                    continue;
                };
                act(rule, actions, &message, &mut ran, &mut changes);
                changes.add(
                    &message,
                    MessageChange::remove_label(label.provider_id.clone()),
                );
            }
        }

        // Rules about mail that has been in the inbox a while, hourly.
        let waited: Vec<&Rule> = enabled
            .iter()
            .copied()
            .filter(|r| r.when == When::Waited)
            .collect();
        let due = local
            .waited_at
            .is_none_or(|t| now.as_millis() - t.as_millis() >= WAITED_EVERY_MILLIS);
        if !waited.is_empty() && due {
            for entry in self.store.label_messages(account, INBOX)? {
                let Some(message) = self.store.message(account, &entry.message)? else {
                    continue;
                };
                for rule in &waited {
                    if matches(rule, &message, now) == Verdict::Yes {
                        act(rule, &rule.actions, &message, &mut ran, &mut changes);
                        if rule.stop {
                            break;
                        }
                    }
                }
            }
        }

        changes.apply(&self.store, account)?;
        self.store.update_rules(account, |_, l| {
            l.cursor = Some(reached);
            l.pending = pending;
            if !waited.is_empty() && due {
                l.waited_at = Some(now);
            }
        })?;
        Ok(ran)
    }

    /// What the rules do here to one arriving message, in their order;
    /// `None` when a rule that matters can't decide until the message is
    /// downloaded.
    fn decide<'r>(
        &self,
        rules: &[&'r Rule],
        running: &postrinch_rules::Running,
        message: &Message,
        now: Timestamp,
    ) -> Option<Vec<(&'r Rule, Vec<Action>)>> {
        let mut todo = Vec::new();
        for rule in rules.iter().copied().filter(|r| r.when == When::Arrives) {
            let part = running.part(rule.id);
            // Mail from before Gmail had the rule's filters goes through it
            // here, whole.
            let before = running
                .since
                .get(&rule.id)
                .is_some_and(|since| message.received < *since);
            let here = before || matches!(part, Some(Part::Whole));
            if !here && !rule.stop {
                continue;
            }
            match matches(rule, message, now) {
                Verdict::NotYet => return None,
                Verdict::No => {}
                Verdict::Yes => {
                    if here {
                        todo.push((rule, rule.actions.clone()));
                    }
                    if rule.stop {
                        break;
                    }
                }
            }
        }
        Some(todo)
    }
}

#[cfg(test)]
#[path = "rules_run_tests.rs"]
mod tests;
