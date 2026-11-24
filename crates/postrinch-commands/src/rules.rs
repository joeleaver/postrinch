//! Mail rules, as Settings › Rules shows and changes them: the account's
//! rules in order, the filters made in Gmail itself, and what a rule would
//! match among the mail already here.

use postrinch_model::{EmailAddress, ProviderMessageId, ProviderThreadId, Timestamp};
use postrinch_rules::matching::{Verdict, matches};
use postrinch_rules::{Action, Elsewhere, Rule, Test};
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
    /// The rules Gmail runs, all their filters made, by id.
    #[serde(default)]
    pub in_gmail: Vec<u32>,
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
        let running = settings.running(local.server == Some(true), local.elsewhere.len());
        Ok(RulesPage {
            in_gmail: running.since.keys().copied().collect(),
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

    /// Turn a filter made in Gmail into a rule at the end of the list;
    /// the old filter goes once Gmail has the rule's. Returns its id.
    pub fn import_filter(&self, account: &EmailAddress, filter: &str) -> CommandResult<u32> {
        self.check(account)?;
        let (_, local) = self.store.rules(account)?;
        let Some(mut rule) = local
            .elsewhere
            .iter()
            .find(|e| e.id == filter)
            .and_then(|e| e.rule.clone())
        else {
            return Err(CommandError::Invalid("That Gmail filter is gone.".into()));
        };
        rule.check()
            .map_err(|e| CommandError::Invalid(e.to_string()))?;
        Ok(self.store.update_rules(account, |settings, local| {
            rule.id = settings.rules.take_id();
            let id = rule.id;
            settings.rules.put(rule);
            settings.retire.push((id, filter.to_owned()));
            local.elsewhere.retain(|e| e.id != filter);
            local.dirty = true;
            id
        })?)
    }

    /// Point the rules that ask an agent which has been replaced at the
    /// agent that replaced it: `moved` gives the new id for an old one.
    /// Returns how many rules changed; with none, nothing is written.
    pub fn move_agent_asks(
        &self,
        account: &EmailAddress,
        moved: impl Fn(&str) -> Option<String>,
    ) -> CommandResult<usize> {
        self.check(account)?;
        let asks_moved = |rule: &Rule| {
            rule.actions
                .iter()
                .any(|a| matches!(a, Action::AskAgent { agent, .. } if moved(agent).is_some()))
        };
        let (settings, _) = self.store.rules(account)?;
        if !settings.rules.rules.iter().any(asks_moved) {
            return Ok(0);
        }
        Ok(self.store.update_rules(account, |settings, local| {
            let mut changed = 0;
            for rule in &mut settings.rules.rules {
                if !asks_moved(rule) {
                    continue;
                }
                for action in &mut rule.actions {
                    if let Action::AskAgent { agent, .. } = action
                        && let Some(new) = moved(agent)
                    {
                        *agent = new;
                    }
                }
                changed += 1;
            }
            if changed > 0 {
                local.dirty = true;
            }
            changed
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
        let found = self.rule_candidates(account, rule, now, false)?;
        let mut here = AlreadyHere::default();
        for id in found {
            let Some(message) = self.store.message(account, &id)? else {
                continue;
            };
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
        self.apply_rule(account, rule, now, false)
    }

    /// [`Commands::apply_to_existing`] over every message here, however
    /// old: for mail the person cleans up once.
    pub fn apply_to_all_existing(
        &self,
        account: &EmailAddress,
        rule: &Rule,
        now: Timestamp,
    ) -> CommandResult<usize> {
        self.apply_rule(account, rule, now, true)
    }

    fn apply_rule(
        &self,
        account: &EmailAddress,
        rule: &Rule,
        now: Timestamp,
        all: bool,
    ) -> CommandResult<usize> {
        let labels = self.store.labels(account)?;
        let mut changed = 0;
        let mut changes = crate::rules_run::Changes::default();
        for id in self.rule_candidates(account, rule, now, all)? {
            let Some(message) = self.store.message(account, &id)? else {
                continue;
            };
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

    /// The last three months' messages that might match `rule` (with
    /// `all`, every message), newest first: narrowed by the store's indexes
    /// where the rule says who or what, then read in full for the matcher.
    fn rule_candidates(
        &self,
        account: &EmailAddress,
        rule: &Rule,
        now: Timestamp,
        all: bool,
    ) -> CommandResult<Vec<ProviderMessageId>> {
        self.check(account)?;
        let mut find = Find {
            accounts: vec![account.clone()],
            after: (!all)
                .then(|| Timestamp::from_millis(now.as_millis() - ALREADY_HERE_DAYS * DAY_MILLIS)),
            ..Find::default()
        };
        if rule.matching == postrinch_rules::Match::All {
            narrow(&mut find, rule, account);
        }
        Ok(self
            .store
            .find_messages(&find)?
            .into_iter()
            .take(if all { usize::MAX } else { ALREADY_HERE_READ })
            .map(|found| found.id)
            .collect())
    }
}

/// Narrow a search to what an "all of" rule's positive conditions name.
/// The store's matches are broader (parts of names, any case); the matcher
/// decides.
fn narrow(find: &mut Find, rule: &Rule, account: &EmailAddress) {
    let label = |id: &str| vec![(account.clone(), id.to_owned())];
    for c in rule.conditions.iter().filter(|c| c.negate) {
        match &c.test {
            Test::HasLabel(l) => find.without.push(label(l)),
            Test::InCategory(cat) => find.without.push(label(cat.label_id())),
            Test::InInbox => find.without.push(label("INBOX")),
            _ => {}
        }
    }
    for c in rule.conditions.iter().filter(|c| !c.negate) {
        match &c.test {
            Test::HasLabel(l) => find.labels.push(label(l)),
            Test::InCategory(cat) => find.labels.push(label(cat.label_id())),
            Test::InInbox => find.labels.push(label("INBOX")),
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
