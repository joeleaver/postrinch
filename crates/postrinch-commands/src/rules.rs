//! Mail rules, as Settings › Rules shows and changes them: the account's
//! rules in order, the filters made in Gmail itself, and what a rule would
//! match among the mail already here.

use postrinch_model::{EmailAddress, Message, ProviderThreadId, Timestamp};
use postrinch_rules::matching::{Verdict, matches};
use postrinch_rules::{Elsewhere, Rule, Test};
use postrinch_store::Find;
use serde::{Deserialize, Serialize};

use crate::{CommandError, CommandResult, Commands};

/// How far back "already here" looks.
const ALREADY_HERE_DAYS: i64 = 90;
/// Up to how many messages "already here" reads.
const ALREADY_HERE_READ: usize = 5_000;
const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// An account's rules, as the rules page shows them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesPage {
    pub rules: Vec<Rule>,
    /// Why Gmail would not take a rule, by rule id.
    pub refused: Vec<(u32, String)>,
    /// Filters made in Gmail itself.
    pub elsewhere: Vec<Elsewhere>,
    /// Addresses Gmail has verified for forwarding.
    pub forwarding: Vec<String>,
    /// Whether the account lets Postrinch make filters; `None` until sync
    /// has asked.
    pub server: Option<bool>,
}

/// A conversation a rule matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Matched {
    pub thread: ProviderThreadId,
    pub from: String,
    pub subject: String,
    pub received: Timestamp,
}

/// What a rule matches among the mail already here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlreadyHere {
    /// Conversations, newest first.
    pub threads: Vec<Matched>,
    /// Matching messages, for "also do this to these now".
    pub messages: usize,
}

impl Commands {
    pub fn rules_page(&self, account: &EmailAddress) -> CommandResult<RulesPage> {
        self.check(account)?;
        let (settings, local) = self.store.rules(account)?;
        Ok(RulesPage {
            rules: settings.rules.rules,
            refused: settings.refused.into_iter().collect(),
            elsewhere: local.elsewhere,
            forwarding: local.forwarding,
            server: local.server,
        })
    }

    /// Save `rule` in place of the rule with its id, or as a new rule at
    /// the end when its id is 0. Returns its id.
    pub fn save_rule(&self, account: &EmailAddress, mut rule: Rule) -> CommandResult<u32> {
        self.check(account)?;
        rule.check()
            .map_err(|e| CommandError::Invalid(e.to_string()))?;
        Ok(self.store.update_rules(account, |settings, local| {
            if rule.id == 0 {
                rule.id = settings.rules.take_id();
            }
            settings.refused.remove(&rule.id);
            let id = rule.id;
            settings.rules.put(rule);
            local.dirty = true;
            id
        })?)
    }

    pub fn delete_rule(&self, account: &EmailAddress, id: u32) -> CommandResult<()> {
        self.check(account)?;
        self.store.update_rules(account, |settings, local| {
            settings.rules.remove(id);
            settings.refused.remove(&id);
            local.dirty = true;
        })?;
        Ok(())
    }

    pub fn set_rule_on(&self, account: &EmailAddress, id: u32, on: bool) -> CommandResult<()> {
        self.check(account)?;
        self.store.update_rules(account, |settings, local| {
            if let Some(rule) = settings.rules.rules.iter_mut().find(|r| r.id == id) {
                rule.enabled = on;
                local.dirty = true;
            }
        })?;
        Ok(())
    }

    /// Move a rule to `to` in the order rules run.
    pub fn move_rule(&self, account: &EmailAddress, id: u32, to: usize) -> CommandResult<()> {
        self.check(account)?;
        self.store.update_rules(account, |settings, local| {
            settings.rules.reorder(id, to);
            local.dirty = true;
        })?;
        Ok(())
    }

    /// What `rule` matches among the last three months' mail, at `now`.
    pub fn already_here(
        &self,
        account: &EmailAddress,
        rule: &Rule,
        now: Timestamp,
    ) -> CommandResult<AlreadyHere> {
        let found = self.rule_candidates(account, rule, now)?;
        let mut here = AlreadyHere::default();
        for message in found {
            if matches(rule, &message, now) != Verdict::Yes {
                continue;
            }
            here.messages += 1;
            if here.threads.iter().any(|t| t.thread == message.thread) {
                continue;
            }
            here.threads.push(Matched {
                thread: message.thread.clone(),
                from: message
                    .headers
                    .from
                    .first()
                    .map(|m| m.display_name())
                    .unwrap_or_default(),
                subject: message.subject().to_owned(),
                received: message.received,
            });
        }
        Ok(here)
    }

    /// Do `rule`'s actions that change mail (not its agent's) to what it
    /// matches among the last three months' mail. Returns how many
    /// messages changed.
    pub fn apply_to_existing(
        &self,
        account: &EmailAddress,
        rule: &Rule,
        now: Timestamp,
    ) -> CommandResult<usize> {
        let labels = self.store.labels(account)?;
        let mut changed = 0;
        let mut changes = crate::rules_run::Changes::default();
        for message in self.rule_candidates(account, rule, now)? {
            if matches(rule, &message, now) == Verdict::Yes {
                let change = crate::rules_run::change_for(&rule.actions, &message, &labels);
                if !change.is_empty() {
                    changes.add(&message, change);
                    changed += 1;
                }
            }
        }
        changes.apply(&self.store, account)?;
        Ok(changed)
    }

    /// The last three months' messages that might match `rule`, newest
    /// first: narrowed by the store's indexes where the rule says who or
    /// what, then read in full for the matcher.
    fn rule_candidates(
        &self,
        account: &EmailAddress,
        rule: &Rule,
        now: Timestamp,
    ) -> CommandResult<Vec<Message>> {
        self.check(account)?;
        let mut find = Find {
            accounts: vec![account.clone()],
            after: Some(Timestamp::from_millis(
                now.as_millis() - ALREADY_HERE_DAYS * DAY_MILLIS,
            )),
            ..Find::default()
        };
        if rule.matching == postrinch_rules::Match::All {
            narrow(&mut find, rule);
        }
        let mut messages = Vec::new();
        for found in self
            .store
            .find_messages(&find)?
            .iter()
            .take(ALREADY_HERE_READ)
        {
            if let Some(message) = self.store.message(account, &found.id)? {
                messages.push(message);
            }
        }
        Ok(messages)
    }
}

/// Narrow a search to what an "all of" rule's positive conditions name.
/// The store's matches are broader (parts of names, any case); the matcher
/// decides.
fn narrow(find: &mut Find, rule: &Rule) {
    for c in rule.conditions.iter().filter(|c| !c.negate) {
        match &c.test {
            Test::FromAddress(a) | Test::FromDomain(a) => {
                find.from
                    .push(a.trim().trim_start_matches('@').to_lowercase());
            }
            Test::RecipientAddress(a) | Test::ToAddress(a) | Test::CcAddress(a) => {
                find.to.push(a.trim().to_lowercase());
            }
            Test::SubjectWords(w) | Test::SubjectPhrase(w) => {
                find.subject.extend(postrinch_rules::words(w));
            }
            Test::Unread => find.unread = Some(true),
            Test::Starred => find.starred = Some(true),
            Test::HasAttachment => find.attachments = Some(true),
            _ => {}
        }
    }
}
