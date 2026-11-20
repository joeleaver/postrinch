//! A rule as agents write and read it: conditions and actions named in
//! plain words with a value, rather than Postrinch's own types. The MCP
//! server takes it from the agent as it is, and the app turns it into a
//! [`Rule`], naming labels and agents by their ids.

use postrinch_rules::{Action, Category, Condition, Match, Rule, Test, When};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The words for each condition, as the tools' schema lists them.
pub const TESTS: &str = "from (an address), from_domain, from_words, sent_to (To, Cc or Bcc), \
to, cc, subject_words, subject_phrase, words (subject, body, people, attachment names), \
phrase, has_attachment, attachment_type (pdf), attachment_name, larger_than_mb, \
smaller_than_mb, unread, starred, label, category (primary, social, promotions, updates, \
forums), in_inbox, older_than_days (only with when `waited`), gmail_search";

/// The words for each action.
pub const ACTIONS: &str = "add_label, skip_inbox (archive), mark_read, star, mark_important, \
never_important, category (primary, social, promotions, updates, forums), never_spam, \
trash, forward (an address Gmail has verified for forwarding), ask_agent (value: the \
agent's name; prompt: what to ask it)";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RuleShape {
    pub name: String,
    /// `arrives` (the default): as each message arrives. `waited`: over
    /// mail already here, as it ages (with `older_than_days`).
    #[serde(default)]
    pub when: Option<String>,
    /// `all` (the default) or `any` of the conditions.
    #[serde(default)]
    pub matching: Option<String>,
    pub conditions: Vec<ConditionShape>,
    pub actions: Vec<ActionShape>,
    /// Later rules don't see mail this rule matched. Default false.
    #[serde(default)]
    pub stop: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConditionShape {
    /// What it looks at: from, from_domain, from_words, sent_to, to, cc,
    /// subject_words, subject_phrase, words, phrase, has_attachment,
    /// attachment_type, attachment_name, larger_than_mb, smaller_than_mb,
    /// unread, starred, label, category, in_inbox, older_than_days,
    /// gmail_search.
    pub test: String,
    /// The address, words, label, number or search it looks for; none for
    /// has_attachment, unread, starred and in_inbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// The opposite: not from, not unread. Default false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub not: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActionShape {
    /// add_label, skip_inbox, mark_read, star, mark_important,
    /// never_important, category, never_spam, trash, forward or ask_agent.
    pub action: String,
    /// The label (name or id), category, address or agent's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// For ask_agent: what to ask it to do with each message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

/// Names labels and agents both ways: by name or id to an id, and an id to
/// the name shown.
pub trait Names {
    fn label_id(&self, name: &str) -> Result<String, String>;
    fn label_name(&self, id: &str) -> String;
    fn agent_id(&self, name: &str) -> Result<String, String>;
    fn agent_name(&self, id: &str) -> String;
}

impl RuleShape {
    /// The rule it says, with id `id` (0 for a new one), switched on.
    pub fn to_rule(&self, id: u32, names: &dyn Names) -> Result<Rule, String> {
        let when = match self.when.as_deref().map(str::trim) {
            None | Some("" | "arrives") => When::Arrives,
            Some("waited") => When::Waited,
            Some(other) => return Err(format!("`when` is arrives or waited, not {other:?}.")),
        };
        let matching = match self.matching.as_deref().map(str::trim) {
            None | Some("" | "all") => Match::All,
            Some("any") => Match::Any,
            Some(other) => return Err(format!("`matching` is all or any, not {other:?}.")),
        };
        let rule = Rule {
            when,
            matching,
            conditions: self
                .conditions
                .iter()
                .map(|c| c.to_condition(names))
                .collect::<Result<_, _>>()?,
            actions: self
                .actions
                .iter()
                .map(|a| a.to_action(names))
                .collect::<Result<_, _>>()?,
            stop: self.stop,
            ..Rule::new(id, self.name.trim())
        };
        rule.check().map_err(|e| e.to_string())?;
        Ok(rule)
    }

    pub fn of(rule: &Rule, names: &dyn Names) -> Self {
        Self {
            name: rule.name.clone(),
            when: Some(match rule.when {
                When::Arrives => "arrives".into(),
                When::Waited => "waited".into(),
            }),
            matching: Some(match rule.matching {
                Match::All => "all".into(),
                Match::Any => "any".into(),
            }),
            conditions: rule
                .conditions
                .iter()
                .map(|c| ConditionShape::of(c, names))
                .collect(),
            actions: rule
                .actions
                .iter()
                .map(|a| ActionShape::of(a, names))
                .collect(),
            stop: rule.stop,
        }
    }
}

impl ConditionShape {
    fn to_condition(&self, names: &dyn Names) -> Result<Condition, String> {
        let test = self.test.trim();
        let value = || {
            self.value
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("The condition {test} needs a value."))
        };
        let number = |unit: &str| -> Result<f64, String> {
            value()?
                .parse::<f64>()
                .ok()
                .filter(|n| *n > 0.0)
                .ok_or_else(|| format!("{test} needs a number of {unit} above 0."))
        };
        let test = match test {
            "from" => Test::FromAddress(value()?),
            "from_domain" => Test::FromDomain(value()?.trim_start_matches('@').to_owned()),
            "from_words" => Test::FromWords(value()?),
            "sent_to" => Test::RecipientAddress(value()?),
            "to" => Test::ToAddress(value()?),
            "cc" => Test::CcAddress(value()?),
            "subject_words" => Test::SubjectWords(value()?),
            "subject_phrase" => Test::SubjectPhrase(value()?),
            "words" => Test::Words(value()?),
            "phrase" => Test::Phrase(value()?),
            "has_attachment" => Test::HasAttachment,
            "attachment_type" => Test::AttachmentType(value()?.trim_start_matches('.').to_owned()),
            "attachment_name" => Test::AttachmentName(value()?),
            "larger_than_mb" => Test::LargerThan((number("MB")? * MB) as u64),
            "smaller_than_mb" => Test::SmallerThan((number("MB")? * MB) as u64),
            "unread" => Test::Unread,
            "starred" => Test::Starred,
            // Gmail's categories are labels there (CATEGORY_UPDATES); here
            // they have a test of their own.
            "label" if is_category_label(&value()?) => Test::InCategory(category(&value()?)?),
            "label" => Test::HasLabel(names.label_id(&value()?)?),
            "category" => Test::InCategory(category(&value()?)?),
            "in_inbox" => Test::InInbox,
            "older_than_days" => Test::OlderThanDays(number("days")?.ceil() as u32),
            "gmail_search" => Test::GmailSearch(value()?),
            other => return Err(format!("There is no condition {other:?}: use {TESTS}.")),
        };
        Ok(Condition {
            test,
            negate: self.not,
        })
    }

    fn of(c: &Condition, names: &dyn Names) -> Self {
        let (test, value) = match &c.test {
            Test::FromAddress(v) => ("from", Some(v.clone())),
            Test::FromDomain(v) => ("from_domain", Some(v.clone())),
            Test::FromWords(v) => ("from_words", Some(v.clone())),
            Test::RecipientAddress(v) => ("sent_to", Some(v.clone())),
            Test::ToAddress(v) => ("to", Some(v.clone())),
            Test::CcAddress(v) => ("cc", Some(v.clone())),
            Test::SubjectWords(v) => ("subject_words", Some(v.clone())),
            Test::SubjectPhrase(v) => ("subject_phrase", Some(v.clone())),
            Test::Words(v) => ("words", Some(v.clone())),
            Test::Phrase(v) => ("phrase", Some(v.clone())),
            Test::HasAttachment => ("has_attachment", None),
            Test::AttachmentType(v) => ("attachment_type", Some(v.clone())),
            Test::AttachmentName(v) => ("attachment_name", Some(v.clone())),
            Test::LargerThan(b) => ("larger_than_mb", Some(mb_text(*b))),
            Test::SmallerThan(b) => ("smaller_than_mb", Some(mb_text(*b))),
            Test::Unread => ("unread", None),
            Test::Starred => ("starred", None),
            Test::HasLabel(id) => ("label", Some(names.label_name(id))),
            Test::InCategory(c) => ("category", Some(c.name().to_ascii_lowercase())),
            Test::InInbox => ("in_inbox", None),
            Test::OlderThanDays(n) => ("older_than_days", Some(n.to_string())),
            Test::GmailSearch(v) => ("gmail_search", Some(v.clone())),
        };
        Self {
            test: test.to_owned(),
            value,
            not: c.negate,
        }
    }
}

impl ActionShape {
    fn to_action(&self, names: &dyn Names) -> Result<Action, String> {
        let what = self.action.trim();
        let value = || {
            self.value
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("The action {what} needs a value."))
        };
        Ok(match what {
            "add_label" => Action::AddLabel(names.label_id(&value()?)?),
            "skip_inbox" | "archive" => Action::SkipInbox,
            "mark_read" => Action::MarkRead,
            "star" => Action::Star,
            "mark_important" => Action::MarkImportant,
            "never_important" => Action::NeverImportant,
            "category" => Action::Category(category(&value()?)?),
            "never_spam" => Action::NeverSpam,
            "trash" => Action::Trash,
            "forward" => Action::Forward(value()?),
            "ask_agent" => Action::AskAgent {
                agent: names.agent_id(&value()?)?,
                prompt: self
                    .prompt
                    .clone()
                    .filter(|p| !p.trim().is_empty())
                    .ok_or("ask_agent needs a `prompt`: what to ask the agent.")?,
            },
            other => return Err(format!("There is no action {other:?}: use {ACTIONS}.")),
        })
    }

    fn of(a: &Action, names: &dyn Names) -> Self {
        let (action, value, prompt) = match a {
            Action::AddLabel(id) => ("add_label", Some(names.label_name(id)), None),
            Action::SkipInbox => ("skip_inbox", None, None),
            Action::MarkRead => ("mark_read", None, None),
            Action::Star => ("star", None, None),
            Action::MarkImportant => ("mark_important", None, None),
            Action::NeverImportant => ("never_important", None, None),
            Action::Category(c) => ("category", Some(c.name().to_ascii_lowercase()), None),
            Action::NeverSpam => ("never_spam", None, None),
            Action::Trash => ("trash", None, None),
            Action::Forward(to) => ("forward", Some(to.clone()), None),
            Action::AskAgent { agent, prompt } => (
                "ask_agent",
                Some(names.agent_name(agent)),
                Some(prompt.clone()),
            ),
        };
        Self {
            action: action.to_owned(),
            value,
            prompt,
        }
    }
}

/// The editor's megabyte.
const MB: f64 = 1_048_576.0;

fn mb_text(bytes: u64) -> String {
    let text = format!("{:.2}", bytes as f64 / MB);
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn is_category_label(value: &str) -> bool {
    let v = value.trim().to_ascii_lowercase();
    v.starts_with("category_") || v.starts_with("category:")
}

/// An inbox category by its name (`updates`), or as Gmail's label for it
/// reads (`CATEGORY_UPDATES`, `category:updates`).
fn category(name: &str) -> Result<Category, String> {
    let name = name.trim();
    let bare = name
        .strip_prefix("category:")
        .or_else(|| name.strip_prefix("CATEGORY_"))
        .unwrap_or(name);
    Category::ALL
        .into_iter()
        .find(|c| {
            c.name().eq_ignore_ascii_case(bare)
                || c.label_id().eq_ignore_ascii_case(name)
                || (*c == Category::Primary && bare.eq_ignore_ascii_case("personal"))
        })
        .ok_or_else(|| {
            format!(
                "There is no category {name:?}: use primary, social, promotions, updates or forums."
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Plain;

    impl Names for Plain {
        fn label_id(&self, name: &str) -> Result<String, String> {
            match name {
                "School" => Ok("Label_1".into()),
                other => Err(format!("no label {other}")),
            }
        }
        fn label_name(&self, id: &str) -> String {
            if id == "Label_1" {
                "School".into()
            } else {
                id.into()
            }
        }
        fn agent_id(&self, name: &str) -> Result<String, String> {
            Ok(format!("id-{name}"))
        }
        fn agent_name(&self, id: &str) -> String {
            id.trim_start_matches("id-").into()
        }
    }

    fn shape(json: &str) -> RuleShape {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_rule_reads_and_writes_back_the_same() {
        let s = shape(
            r#"{"name":"School","conditions":[{"test":"from_domain","value":"@westfield.edu"},
                {"test":"larger_than_mb","value":"2.5","not":true}],
                "actions":[{"action":"add_label","value":"School"},{"action":"category","value":"Updates"},
                {"action":"ask_agent","value":"Claude","prompt":"Summarise it"}]}"#,
        );
        let rule = s.to_rule(0, &Plain).unwrap();
        assert_eq!(
            rule.conditions[0].test,
            Test::FromDomain("westfield.edu".into())
        );
        assert!(rule.conditions[1].negate);
        assert_eq!(rule.actions[0], Action::AddLabel("Label_1".into()));
        let back = RuleShape::of(&rule, &Plain);
        assert_eq!(back.to_rule(0, &Plain).unwrap(), rule);
        assert_eq!(back.conditions[1].value.as_deref(), Some("2.5"));
        assert_eq!(back.actions[2].value.as_deref(), Some("Claude"));
    }

    #[test]
    fn mistakes_are_said_in_a_sentence() {
        let bad = |json: &str| shape(json).to_rule(0, &Plain).unwrap_err();
        assert!(
            bad(r#"{"name":"R","conditions":[{"test":"form","value":"a@b.c"}],"actions":[{"action":"star"}]}"#)
                .contains("no condition \"form\"")
        );
        assert!(
            bad(r#"{"name":"R","conditions":[{"test":"from"}],"actions":[{"action":"star"}]}"#)
                .contains("needs a value")
        );
        assert!(
            bad(r#"{"name":"R","conditions":[{"test":"unread"}],"actions":[{"action":"add_label","value":"Work"}]}"#)
                .contains("no label Work")
        );
        assert!(
            bad(r#"{"name":"R","conditions":[{"test":"older_than_days","value":"7"}],"actions":[{"action":"skip_inbox"}]}"#)
                .contains("a while")
        );
    }
}
