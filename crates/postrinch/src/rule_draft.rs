//! A rule as the editor holds it while the person changes it: rows of
//! what to look at and what to do, in the editor's words, turned into a
//! [`Rule`] and back.

use postrinch_rules::{Action, Category, Condition, Match, Rule, Test, When};

/// What a condition row looks at, and the ways it can.
pub const FIELDS: &[(&str, &[&str])] = &[
    ("From", &["is", "domain is", "has the words", "isn't"]),
    ("Sent to", &["includes", "To is", "Cc is"]),
    (
        "Subject",
        &["has the words", "has the phrase", "doesn't have the words"],
    ),
    (
        "Anywhere",
        &["has the words", "has the phrase", "doesn't have the words"],
    ),
    ("Attachment", &["any", "file type is", "is named", "none"]),
    ("Size", &["larger than (MB)", "smaller than (MB)"]),
    (
        "Mail is",
        &[
            "unread",
            "read",
            "starred",
            "in the inbox",
            "labelled",
            "not labelled",
            "in the category",
            "not in the category",
            "older than (days)",
        ],
    ),
    ("Gmail search", &["matches"]),
];

/// What an action row can do.
pub const DOES: &[&str] = &[
    "Add the label",
    "Skip the inbox",
    "Mark read",
    "Star",
    "Mark important",
    "Never mark important",
    "Put in the category",
    "Never send to spam",
    "Move to trash",
    "Forward to",
    "Ask an agent",
];

/// One condition, as the editor shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Looks {
    pub id: u32,
    pub field: &'static str,
    pub how: &'static str,
    pub value: String,
}

/// One action, as the editor shows it: `value` is a label id, a category,
/// an address or an agent's id; `prompt` what to ask the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Does {
    pub id: u32,
    pub what: &'static str,
    pub value: String,
    pub prompt: String,
}

/// The rule being edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// 0 for a new rule.
    pub id: u32,
    pub enabled: bool,
    pub name: String,
    pub when: When,
    pub matching: Match,
    pub looks: Vec<Looks>,
    pub does: Vec<Does>,
    pub stop: bool,
    /// Also do it to the matching mail already here.
    pub existing: bool,
    /// Rows made so far, for the next row's id.
    pub rows: u32,
    /// The agent's proposal it came from, settled when it is saved or
    /// discarded.
    pub proposal: Option<FromAgent>,
}

/// Who proposed a rule, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FromAgent {
    pub id: u32,
    pub agent: String,
    pub reason: Option<String>,
}

/// Whether the row needs a value typed or picked.
pub fn needs_value(how: &str) -> bool {
    !matches!(
        how,
        "any" | "none" | "unread" | "read" | "starred" | "in the inbox"
    )
}

/// The ways `field` can look.
pub fn hows(field: &str) -> &'static [&'static str] {
    FIELDS
        .iter()
        .find(|(f, _)| *f == field)
        .map_or(&[], |(_, h)| h)
}

/// The `&'static` spelling of a field, or the first one.
pub fn field_named(name: &str) -> &'static str {
    FIELDS
        .iter()
        .find(|(f, _)| *f == name)
        .map_or(FIELDS[0].0, |(f, _)| f)
}

pub fn how_named(field: &str, name: &str) -> &'static str {
    let all = hows(field);
    all.iter().find(|h| **h == name).copied().unwrap_or(all[0])
}

pub fn does_named(name: &str) -> &'static str {
    DOES.iter()
        .find(|d| **d == name)
        .copied()
        .unwrap_or(DOES[0])
}

impl Draft {
    /// A new rule, with an empty row of each.
    pub fn new() -> Self {
        let mut d = Draft {
            id: 0,
            enabled: true,
            name: String::new(),
            when: When::Arrives,
            matching: Match::All,
            looks: Vec::new(),
            does: Vec::new(),
            stop: false,
            existing: true,
            rows: 0,
            proposal: None,
        };
        d.add_looks("From", "is", String::new());
        d.add_does("Add the label", String::new());
        d
    }

    fn next(&mut self) -> u32 {
        self.rows += 1;
        self.rows
    }

    pub fn add_looks(&mut self, field: &'static str, how: &'static str, value: String) {
        let id = self.next();
        self.looks.push(Looks {
            id,
            field,
            how,
            value,
        });
    }

    pub fn add_does(&mut self, what: &'static str, value: String) {
        let id = self.next();
        self.does.push(Does {
            id,
            what,
            value,
            prompt: String::new(),
        });
    }

    /// The draft of a saved rule.
    pub fn of(rule: &Rule) -> Self {
        let mut d = Draft {
            id: rule.id,
            enabled: rule.enabled,
            name: rule.name.clone(),
            when: rule.when,
            matching: rule.matching,
            looks: Vec::new(),
            does: Vec::new(),
            stop: rule.stop,
            existing: false,
            rows: 0,
            proposal: None,
        };
        for c in &rule.conditions {
            let (field, how, value) = looks_of(c);
            d.add_looks(field, how, value);
        }
        for a in &rule.actions {
            let id = d.next();
            d.does.push(does_of(a, id));
        }
        d
    }

    /// The rule the draft says, rows left blank or unreadable left out.
    pub fn rule(&self) -> Rule {
        Rule {
            id: self.id,
            name: self.name.trim().to_owned(),
            enabled: self.enabled,
            when: self.when,
            matching: self.matching,
            conditions: self.looks.iter().filter_map(condition).collect(),
            actions: self.does.iter().filter_map(action).collect(),
            stop: self.stop,
        }
    }

    /// Whether every row the person filled in reads.
    pub fn trouble(&self) -> Option<String> {
        if let Some(row) = self
            .looks
            .iter()
            .find(|l| needs_value(l.how) && !l.value.trim().is_empty() && condition(l).is_none())
        {
            return Some(format!(
                "\u{201c}{}\u{201d} is not a number.",
                row.value.trim()
            ));
        }
        self.rule().check().err().map(|e| e.to_string())
    }
}

impl Default for Draft {
    fn default() -> Self {
        Self::new()
    }
}

fn condition(l: &Looks) -> Option<Condition> {
    let v = l.value.trim().to_owned();
    if needs_value(l.how) && v.is_empty() {
        return None;
    }
    let mb = || {
        v.parse::<f64>()
            .ok()
            .filter(|n| *n > 0.0)
            .map(|n| (n * 1_048_576.0) as u64)
    };
    Some(match (l.field, l.how) {
        ("From", "is") => Condition::is(Test::FromAddress(v)),
        ("From", "domain is") => Condition::is(Test::FromDomain(v)),
        ("From", "has the words") => Condition::is(Test::FromWords(v)),
        ("From", "isn't") => Condition::not(Test::FromAddress(v)),
        ("Sent to", "includes") => Condition::is(Test::RecipientAddress(v)),
        ("Sent to", "To is") => Condition::is(Test::ToAddress(v)),
        ("Sent to", "Cc is") => Condition::is(Test::CcAddress(v)),
        ("Subject", "has the words") => Condition::is(Test::SubjectWords(v)),
        ("Subject", "has the phrase") => Condition::is(Test::SubjectPhrase(v)),
        ("Subject", _) => Condition::not(Test::SubjectWords(v)),
        ("Anywhere", "has the words") => Condition::is(Test::Words(v)),
        ("Anywhere", "has the phrase") => Condition::is(Test::Phrase(v)),
        ("Anywhere", _) => Condition::not(Test::Words(v)),
        ("Attachment", "any") => Condition::is(Test::HasAttachment),
        ("Attachment", "none") => Condition::not(Test::HasAttachment),
        ("Attachment", "file type is") => Condition::is(Test::AttachmentType(v)),
        ("Attachment", _) => Condition::is(Test::AttachmentName(v)),
        ("Size", "larger than (MB)") => Condition::is(Test::LargerThan(mb()?)),
        ("Size", _) => Condition::is(Test::SmallerThan(mb()?)),
        ("Mail is", "unread") => Condition::is(Test::Unread),
        ("Mail is", "read") => Condition::not(Test::Unread),
        ("Mail is", "starred") => Condition::is(Test::Starred),
        ("Mail is", "in the inbox") => Condition::is(Test::InInbox),
        ("Mail is", "labelled") => Condition::is(Test::HasLabel(v)),
        ("Mail is", "not labelled") => Condition::not(Test::HasLabel(v)),
        ("Mail is", "in the category") => Condition::is(Test::InCategory(category_named(&v)?)),
        ("Mail is", "not in the category") => Condition::not(Test::InCategory(category_named(&v)?)),
        ("Mail is", _) => Condition::is(Test::OlderThanDays(v.parse().ok()?)),
        _ => Condition::is(Test::GmailSearch(v)),
    })
}

fn category_named(name: &str) -> Option<Category> {
    Category::ALL.into_iter().find(|c| c.name() == name)
}

fn mb_text(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    let text = format!("{mb:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn looks_of(c: &Condition) -> (&'static str, &'static str, String) {
    let n = c.negate;
    match &c.test {
        Test::FromAddress(a) if n => ("From", "isn't", a.clone()),
        Test::FromAddress(a) => ("From", "is", a.clone()),
        Test::FromDomain(d) => ("From", "domain is", d.clone()),
        Test::FromWords(w) => ("From", "has the words", w.clone()),
        Test::RecipientAddress(a) => ("Sent to", "includes", a.clone()),
        Test::ToAddress(a) => ("Sent to", "To is", a.clone()),
        Test::CcAddress(a) => ("Sent to", "Cc is", a.clone()),
        Test::SubjectWords(w) if n => ("Subject", "doesn't have the words", w.clone()),
        Test::SubjectWords(w) => ("Subject", "has the words", w.clone()),
        Test::SubjectPhrase(p) => ("Subject", "has the phrase", p.clone()),
        Test::Words(w) if n => ("Anywhere", "doesn't have the words", w.clone()),
        Test::Words(w) => ("Anywhere", "has the words", w.clone()),
        Test::Phrase(p) => ("Anywhere", "has the phrase", p.clone()),
        Test::HasAttachment if n => ("Attachment", "none", String::new()),
        Test::HasAttachment => ("Attachment", "any", String::new()),
        Test::AttachmentType(t) => ("Attachment", "file type is", t.clone()),
        Test::AttachmentName(name) => ("Attachment", "is named", name.clone()),
        Test::LargerThan(b) => ("Size", "larger than (MB)", mb_text(*b)),
        Test::SmallerThan(b) => ("Size", "smaller than (MB)", mb_text(*b)),
        Test::Unread if n => ("Mail is", "read", String::new()),
        Test::Unread => ("Mail is", "unread", String::new()),
        Test::Starred => ("Mail is", "starred", String::new()),
        Test::InInbox => ("Mail is", "in the inbox", String::new()),
        Test::HasLabel(l) if n => ("Mail is", "not labelled", l.clone()),
        Test::HasLabel(l) => ("Mail is", "labelled", l.clone()),
        Test::InCategory(c) if n => ("Mail is", "not in the category", c.name().to_owned()),
        Test::InCategory(c) => ("Mail is", "in the category", c.name().to_owned()),
        Test::OlderThanDays(d) => ("Mail is", "older than (days)", d.to_string()),
        Test::GmailSearch(q) => ("Gmail search", "matches", q.clone()),
    }
}

fn action(d: &Does) -> Option<Action> {
    let v = d.value.trim().to_owned();
    Some(match d.what {
        "Add the label" if !v.is_empty() => Action::AddLabel(v),
        "Skip the inbox" => Action::SkipInbox,
        "Mark read" => Action::MarkRead,
        "Star" => Action::Star,
        "Mark important" => Action::MarkImportant,
        "Never mark important" => Action::NeverImportant,
        "Put in the category" => Action::Category(category_named(&v)?),
        "Never send to spam" => Action::NeverSpam,
        "Move to trash" => Action::Trash,
        "Forward to" if !v.is_empty() => Action::Forward(v),
        "Ask an agent" if !v.is_empty() => Action::AskAgent {
            agent: v,
            prompt: d.prompt.trim().to_owned(),
        },
        _ => return None,
    })
}

fn does_of(a: &Action, id: u32) -> Does {
    let row = |what: &'static str, value: &str| Does {
        id,
        what,
        value: value.to_owned(),
        prompt: String::new(),
    };
    match a {
        Action::AddLabel(l) => row("Add the label", l),
        Action::SkipInbox => row("Skip the inbox", ""),
        Action::MarkRead => row("Mark read", ""),
        Action::Star => row("Star", ""),
        Action::MarkImportant => row("Mark important", ""),
        Action::NeverImportant => row("Never mark important", ""),
        Action::Category(c) => row("Put in the category", c.name()),
        Action::NeverSpam => row("Never send to spam", ""),
        Action::Trash => row("Move to trash", ""),
        Action::Forward(to) => row("Forward to", to),
        Action::AskAgent { agent, prompt } => Does {
            prompt: prompt.clone(),
            ..row("Ask an agent", agent)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_reads_back_through_its_draft() {
        let rule = Rule {
            matching: Match::Any,
            conditions: vec![
                Condition::is(Test::FromDomain("westfield.edu".into())),
                Condition::not(Test::SubjectWords("lunch menu".into())),
                Condition::is(Test::LargerThan(5 * 1_048_576)),
                Condition::not(Test::HasAttachment),
                Condition::not(Test::HasLabel("Label_2".into())),
                Condition::is(Test::InCategory(Category::Promotions)),
                Condition::not(Test::InCategory(Category::Primary)),
            ],
            actions: vec![
                Action::AddLabel("Label_1".into()),
                Action::Category(Category::Updates),
                Action::AskAgent {
                    agent: "a1".into(),
                    prompt: "Summarise it".into(),
                },
            ],
            stop: true,
            ..Rule::new(7, "School")
        };
        assert_eq!(Draft::of(&rule).rule(), rule);
    }

    #[test]
    fn blank_rows_are_left_out_and_trouble_is_named() {
        let mut d = Draft::new();
        assert_eq!(d.rule().conditions, []);
        d.name = "Big".into();
        d.looks[0] = Looks {
            id: 1,
            field: "Size",
            how: "larger than (MB)",
            value: "lots".into(),
        };
        assert!(d.trouble().unwrap().contains("not a number"));
        d.looks[0].value = "2.5".into();
        d.does[0].value = "Label_1".into();
        assert_eq!(d.trouble(), None);
        assert_eq!(
            d.rule().conditions,
            [Condition::is(Test::LargerThan(2_621_440))]
        );
    }
}
