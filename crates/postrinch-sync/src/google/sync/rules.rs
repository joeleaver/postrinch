//! Keeping Gmail in step with the account's rules (docs/rules.md): the
//! settings message every computer reads them from, and the filters Gmail
//! runs for them.

use std::collections::HashMap;

use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address;
use postrinch_model::EmailAddress;
use postrinch_rules::gmail::Filter;
use postrinch_rules::{Elsewhere, Made, Settings};

use super::GmailSync;
use super::outbox::now;
use crate::google::api::FilterResource;
use crate::{SyncError, SyncResult};

/// The hidden label the settings message is kept under.
pub const SETTINGS_LABEL: &str = "[Postrinch]/Settings";
/// Marks the settings message among the account's mail.
const SETTINGS_HEADER: &str = "X-Postrinch-Settings";
/// How many times making the filters is tried in one pass, as each refusal
/// changes what the rest should be.
const ROUNDS: usize = 4;

/// How often Gmail's filters and the settings message are looked at when
/// nothing changed here.
const CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// What one pass did about rules.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RulesReport {
    /// The rules came from another computer.
    pub read: bool,
    /// Filters made and deleted in Gmail.
    pub made: usize,
    pub deleted: usize,
}

impl GmailSync {
    /// Whether rules should be brought in step this pass: they changed
    /// here, or [`CHECK_EVERY`] has gone by.
    pub(super) async fn rules_due(&self, account: &EmailAddress) -> bool {
        let dirty = self.rules_here(account).await.is_ok_and(|(_, l)| l.dirty);
        let checked = self.rules_checked.lock().unwrap_or_else(|e| e.into_inner());
        dirty
            || checked
                .get(&account.key())
                .is_none_or(|t| t.elapsed() >= CHECK_EVERY)
    }

    /// Look at the account's rules again next pass, as after it signs in
    /// again and may now let Postrinch make filters.
    pub fn recheck_rules(&self, account: &EmailAddress) {
        self.rules_checked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&account.key());
    }

    pub(super) fn rules_checked(&self, account: &EmailAddress) {
        self.rules_checked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(account.key(), std::time::Instant::now());
    }

    /// Bring the account's settings message and Gmail's filters in step
    /// with its rules. Cheap when nothing changed: one listing of filters
    /// and one of the settings label.
    pub async fn sync_rules(&self, account: &EmailAddress) -> SyncResult<RulesReport> {
        let mut report = RulesReport::default();
        let labels = self.api.labels(account).await?;
        let mut label_ids: HashMap<String, String> = labels
            .iter()
            .map(|l| (l.name.clone(), l.id.clone()))
            .collect();
        // The settings label is made only when there are rules to keep:
        // an account nobody wrote rules for is left as it was.
        let mut settings_label = label_ids.get(SETTINGS_LABEL).cloned();
        if let Some(id) = &settings_label {
            self.note_settings_label(account, id);
        }

        // The rules another computer wrote, unless this one has changes of
        // its own to write.
        let found = match &settings_label {
            Some(id) => self.api.labelled(account, id).await?,
            None => Vec::new(),
        };
        let (_, local) = self.rules_here(account).await?;
        if let Some(newest) = found.first()
            && local.message.as_deref() != Some(newest)
            && !local.dirty
        {
            let raw = self
                .api
                .raw_message(account, newest, crate::google::api::Priority::Foreground)
                .await?;
            match read_settings(&raw.bytes()?) {
                Some(settings) => {
                    let newest = newest.clone();
                    let account = account.clone();
                    self.with_store(move |s| {
                        s.update_rules(&account, |stored, l| {
                            *stored = settings;
                            l.message = Some(newest);
                        })
                    })
                    .await?;
                    report.read = true;
                }
                None => tracing::warn!("{account}: the rules settings message does not read"),
            }
        }

        // Gmail's filters, which the mail scope lists; making them needs
        // the settings scope.
        let listed = match self.api.filters(account).await {
            Ok(f) => Some(f),
            Err(SyncError::Api { status: 403, .. }) => None,
            Err(e) => return Err(e),
        };
        let server = listed.is_some() && self.api.may_make_filters(account).await?;
        let forwarding = match &listed {
            Some(_) => self.api.forwarding(account).await.unwrap_or_default(),
            None => Vec::new(),
        };
        let filters = listed.clone().filter(|_| server);
        let (mut settings, local) = self.rules_here(account).await?;
        let before = settings.clone();
        if let Some(filters) = &filters {
            self.make_filters(account, &mut settings, filters, &mut label_ids, &mut report)
                .await?;
        }
        let elsewhere = match &listed {
            Some(all) => {
                // Ours now, and ours before this pass (deleted in it).
                let ours: Vec<&str> = settings
                    .made
                    .iter()
                    .chain(&before.made)
                    .map(|m| m.id.as_str())
                    .collect();
                let names: HashMap<&str, &str> = label_ids
                    .iter()
                    .map(|(n, i)| (i.as_str(), n.as_str()))
                    .collect();
                all.iter()
                    .filter(|f| !ours.contains(&f.id.as_str()))
                    .map(|f| Elsewhere {
                        id: f.id.clone(),
                        line: describe(f, &names),
                        rule: Some(as_rule(f, &names)),
                    })
                    .collect()
            }
            None => Vec::new(),
        };

        // Markers of rules that are gone, once no filter adds them.
        if filters.is_some() {
            for (name, id) in &label_ids {
                let Some(n) = name
                    .strip_prefix(postrinch_rules::MARKER_PREFIX)
                    .and_then(|rest| rest.strip_prefix('r'))
                    .and_then(|n| n.parse::<u32>().ok())
                else {
                    continue;
                };
                if settings.rules.get(n).is_none() {
                    self.api.delete_label(account, id).await?;
                }
            }
        }

        // Write the settings message when anything in it changed.
        let mut message = local.message.clone();
        if local.dirty || settings != before {
            let label = match settings_label.take() {
                Some(id) => id,
                None => {
                    let id = self
                        .api
                        .create_hidden_label(account, SETTINGS_LABEL)
                        .await?;
                    self.note_settings_label(account, &id);
                    id
                }
            };
            let id = self
                .api
                .insert(account, &write_settings(account, &settings), &label)
                .await?;
            for old in found.iter().filter(|old| **old != id) {
                self.api.delete_message(account, old).await?;
            }
            message = Some(id);
        }
        let account = account.clone();
        self.with_store(move |s| {
            s.update_rules(&account, |stored, l| {
                // Rules saved here while this pass ran win; the filters
                // made for the old ones are put right next pass.
                if !l.dirty || settings.rules == stored.rules {
                    l.dirty = false;
                }
                stored.made = settings.made;
                stored.refused = settings.refused;
                l.message = message;
                l.server = Some(server);
                l.elsewhere = elsewhere;
                l.forwarding = forwarding;
            })
        })
        .await?;
        Ok(report)
    }

    /// Make and delete filters until Gmail has exactly the plan's, as far
    /// as it takes them.
    async fn make_filters(
        &self,
        account: &EmailAddress,
        settings: &mut Settings,
        filters: &[FilterResource],
        label_ids: &mut HashMap<String, String>,
        report: &mut RulesReport,
    ) -> SyncResult<()> {
        // A filter deleted in Gmail is made again.
        settings
            .made
            .retain(|m| filters.iter().any(|f| f.id == m.id));
        for _ in 0..ROUNDS {
            let others = filters.len().saturating_sub(settings.made.len());
            let plan = settings.plan(true, others);
            let (keep, stale): (Vec<Made>, Vec<Made>) = std::mem::take(&mut settings.made)
                .into_iter()
                .partition(|m| plan.filters.contains(&m.filter));
            settings.made = keep;
            for gone in stale {
                self.api.delete_filter(account, &gone.id).await?;
                report.deleted += 1;
            }
            let mut refused = false;
            for filter in plan.filters {
                if settings.made.iter().any(|m| m.filter == filter) {
                    continue;
                }
                match self
                    .make_filter(account, &filter, settings, label_ids)
                    .await
                {
                    Ok(made) => {
                        settings.made.push(made);
                        report.made += 1;
                    }
                    Err(SyncError::Api {
                        status: 400 | 403,
                        message,
                        ..
                    }) => {
                        tracing::info!(
                            "{account}: Gmail would not take rule {}: {message}",
                            filter.rule
                        );
                        settings
                            .refused
                            .insert(filter.rule, refusal(&filter, &message));
                        refused = true;
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
            if !refused {
                break;
            }
        }
        // Filters imported as rules go once Gmail runs the rule instead.
        let plan = settings.plan(true, filters.len().saturating_sub(settings.made.len()));
        let mut keep = Vec::new();
        for (rule, old) in std::mem::take(&mut settings.retire) {
            let wanted: Vec<&Filter> = plan.filters.iter().filter(|f| f.rule == rule).collect();
            let made = !wanted.is_empty()
                && wanted
                    .iter()
                    .all(|w| settings.made.iter().any(|m| &m.filter == *w));
            if made {
                self.api.delete_filter(account, &old).await?;
                report.deleted += 1;
            } else if settings.rules.get(rule).is_some() && !settings.refused.contains_key(&rule) {
                keep.push((rule, old));
            }
        }
        settings.retire = keep;
        Ok(())
    }

    async fn make_filter(
        &self,
        account: &EmailAddress,
        filter: &Filter,
        settings: &Settings,
        label_ids: &mut HashMap<String, String>,
    ) -> SyncResult<Made> {
        let mut add = filter.add.clone();
        if filter.marker {
            let name = settings
                .rules
                .get(filter.rule)
                .map(postrinch_rules::Rule::marker)
                .unwrap_or_default();
            let id = match label_ids.get(&name) {
                Some(id) => id.clone(),
                None => {
                    let id = self.api.create_hidden_label(account, &name).await?;
                    label_ids.insert(name, id.clone());
                    id
                }
            };
            add.push(id);
        }
        let since = now();
        let id = self
            .api
            .create_filter(
                account,
                &filter.query,
                &add,
                &filter.remove,
                filter.forward.as_deref(),
            )
            .await?;
        Ok(Made {
            id,
            filter: filter.clone(),
            since,
        })
    }

    async fn rules_here(
        &self,
        account: &EmailAddress,
    ) -> SyncResult<(Settings, postrinch_rules::Local)> {
        let account = account.clone();
        self.with_store(move |s| s.rules(&account)).await
    }
}

/// Why Gmail would not take a rule, in a sentence for its row.
fn refusal(filter: &Filter, gmail: &str) -> String {
    match &filter.forward {
        Some(to) if gmail.to_lowercase().contains("forward") => format!(
            "Gmail stopped accepting the forward: {to} is not a verified forwarding address in Gmail."
        ),
        _ => format!("Gmail would not take this rule ({gmail}), so Postrinch runs it."),
    }
}

/// A filter made elsewhere, in a line: its search, then what it does.
fn describe(f: &FilterResource, names: &HashMap<&str, &str>) -> String {
    let c = &f.criteria;
    let mut what = Vec::new();
    for (op, value) in [("from", &c.from), ("to", &c.to), ("subject", &c.subject)] {
        if let Some(v) = value {
            what.push(format!("{op}:({v})"));
        }
    }
    if let Some(q) = &c.query {
        what.push(q.clone());
    }
    if let Some(q) = &c.negated_query {
        what.push(format!("-{{{q}}}"));
    }
    if c.has_attachment {
        what.push("has:attachment".into());
    }
    if let (Some(size), Some(cmp)) = (c.size, &c.size_comparison) {
        what.push(format!(
            "{}:{size}",
            if cmp == "larger" { "larger" } else { "smaller" }
        ));
    }
    let a = &f.action;
    let mut does = Vec::new();
    for id in &a.add_label_ids {
        does.push(match id.as_str() {
            "STARRED" => "star".to_owned(),
            "IMPORTANT" => "mark important".to_owned(),
            "TRASH" => "trash".to_owned(),
            other => format!("label {}", names.get(other).copied().unwrap_or(other)),
        });
    }
    for id in &a.remove_label_ids {
        does.push(match id.as_str() {
            "INBOX" => "skip the inbox".to_owned(),
            "UNREAD" => "mark read".to_owned(),
            "SPAM" => "never send to spam".to_owned(),
            "IMPORTANT" => "never important".to_owned(),
            other => format!("remove {}", names.get(other).copied().unwrap_or(other)),
        });
    }
    if let Some(to) = &a.forward {
        does.push(format!("forward to {to}"));
    }
    format!("{} \u{2192} {}", what.join(" "), does.join(", "))
}

/// A filter made in Gmail, as the rule importing it makes: what the
/// vocabulary says exactly where it can, the rest as Gmail search.
fn as_rule(f: &FilterResource, names: &HashMap<&str, &str>) -> postrinch_rules::Rule {
    use postrinch_rules::{Action, Category, Condition, Rule, Test};
    let c = &f.criteria;
    let mut conditions = Vec::new();
    let exact = |a: &str| EmailAddress::parse(a).is_ok() && !a.contains(['*', '(', ')', ' ']);
    if let Some(from) = &c.from {
        conditions.push(Condition::is(if exact(from) {
            Test::FromAddress(from.trim().into())
        } else {
            Test::GmailSearch(format!("from:({from})"))
        }));
    }
    if let Some(to) = &c.to {
        conditions.push(Condition::is(if exact(to) {
            Test::RecipientAddress(to.trim().into())
        } else {
            Test::GmailSearch(format!("to:({to})"))
        }));
    }
    if let Some(subject) = &c.subject {
        let plain = postrinch_rules::words(subject).join(" ") == subject.trim().to_lowercase();
        conditions.push(Condition::is(if plain {
            Test::SubjectWords(subject.trim().into())
        } else {
            Test::GmailSearch(format!("subject:({subject})"))
        }));
    }
    if let Some(q) = &c.query {
        conditions.push(Condition::is(Test::GmailSearch(q.clone())));
    }
    if let Some(q) = &c.negated_query {
        conditions.push(Condition::not(Test::GmailSearch(q.clone())));
    }
    if c.has_attachment {
        conditions.push(Condition::is(Test::HasAttachment));
    }
    if let (Some(size), Some(cmp)) = (c.size, &c.size_comparison) {
        conditions.push(Condition::is(if cmp == "larger" {
            Test::LargerThan(size)
        } else {
            Test::SmallerThan(size)
        }));
    }
    let mut actions = Vec::new();
    let mut name = None;
    for id in &f.action.add_label_ids {
        actions.push(match id.as_str() {
            "STARRED" => Action::Star,
            "IMPORTANT" => Action::MarkImportant,
            "TRASH" => Action::Trash,
            other => match Category::ALL.iter().find(|c| c.label_id() == other) {
                Some(c) => Action::Category(*c),
                None => {
                    name = name.or_else(|| names.get(other).map(|n| (*n).to_owned()));
                    Action::AddLabel(other.to_owned())
                }
            },
        });
    }
    for id in &f.action.remove_label_ids {
        match id.as_str() {
            "INBOX" => actions.push(Action::SkipInbox),
            "UNREAD" => actions.push(Action::MarkRead),
            "SPAM" => actions.push(Action::NeverSpam),
            "IMPORTANT" => actions.push(Action::NeverImportant),
            _ => {}
        }
    }
    if let Some(to) = &f.action.forward {
        actions.push(Action::Forward(to.clone()));
    }
    Rule {
        conditions,
        actions,
        ..Rule::new(0, &name.unwrap_or_else(|| "Made in Gmail".to_owned()))
    }
}

/// The settings message: the rules as JSON, from and to the account.
fn write_settings(account: &EmailAddress, settings: &Settings) -> Vec<u8> {
    let json = serde_json::to_string_pretty(settings).expect("settings serialize");
    let me = Address::new_address(Some("Postrinch"), account.to_string());
    let millis = now().as_millis();
    MessageBuilder::new()
        .from(me.clone())
        .to(me)
        .subject("Postrinch settings (your mail rules)")
        .date(millis / 1000)
        .header(SETTINGS_HEADER, mail_builder::headers::raw::Raw::new("1"))
        .text_body(format!(
            "Postrinch keeps your mail rules here so every computer runs the same ones.\n\
             Deleting this message deletes the rules.\n\n{json}\n"
        ))
        .write_to_vec()
        .expect("writing to memory cannot fail")
}

/// The settings in a settings message, if it reads as one.
fn read_settings(raw: &[u8]) -> Option<Settings> {
    let parsed = mail_parser::MessageParser::default().parse(raw)?;
    if !parsed.headers().contains(SETTINGS_HEADER) {
        return None;
    }
    let body: String = parsed.text_bodies().collect::<Vec<_>>().join("\n");
    let start = body.find('{')?;
    let end = body.rfind('}')?;
    serde_json::from_str(&body[start..=end]).ok()
}

#[cfg(test)]
mod tests {
    use postrinch_rules::{Action, Condition, Rule, RuleSet, Test};

    use super::*;
    use crate::google::api::Criteria;

    #[test]
    fn the_settings_message_reads_back() {
        let me = EmailAddress::parse("me@example.com").unwrap();
        let rule = Rule {
            conditions: vec![Condition::is(Test::FromAddress("a@example.com".into()))],
            actions: vec![Action::Star],
            ..Rule::new(1, "Stars")
        };
        let settings = Settings {
            rules: RuleSet {
                rules: vec![rule],
                next_id: 2,
            },
            ..Settings::default()
        };
        let raw = write_settings(&me, &settings);
        assert_eq!(read_settings(&raw), Some(settings));
        assert_eq!(
            read_settings(b"Subject: hello\r\n\r\n{}"),
            None,
            "not marked"
        );
    }

    #[test]
    fn filters_made_elsewhere_read_as_a_line() {
        let mut f = FilterResource {
            id: "f".into(),
            criteria: Criteria {
                from: Some("notifications@github.com".into()),
                ..Criteria::default()
            },
            action: Default::default(),
        };
        f.action.add_label_ids = vec!["Label_5".into()];
        f.action.remove_label_ids = vec!["INBOX".into()];
        let names = HashMap::from([("Label_5", "GitHub")]);
        assert_eq!(
            describe(&f, &names),
            "from:(notifications@github.com) \u{2192} label GitHub, skip the inbox"
        );
        let rule = as_rule(&f, &names);
        assert_eq!(rule.name, "GitHub");
        assert_eq!(
            rule.conditions,
            [Condition::is(Test::FromAddress(
                "notifications@github.com".into()
            ))]
        );
        assert_eq!(
            rule.actions,
            [Action::AddLabel("Label_5".into()), Action::SkipInbox]
        );
        assert!(rule.check().is_ok());
    }
}
