//! What a rule says: when it runs, what it looks for, what it does.

use postrinch_model::EmailAddress;
use serde::{Deserialize, Serialize};

/// The start of every label Postrinch makes for itself in an account
/// (rule markers, the settings message's label). Kept out of the sidebar,
/// the label chips and menus, and agents' label lists.
pub const MARKER_PREFIX: &str = "[Postrinch]/";

/// One account's rules, in the order they run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSet {
    pub rules: Vec<Rule>,
    /// The id the next new rule gets. Ids are never reused, so a marker
    /// left on a message by a deleted rule never reads as a newer rule's.
    #[serde(default = "first_id")]
    pub next_id: u32,
}

fn first_id() -> u32 {
    1
}

impl RuleSet {
    /// A new rule's id, taken.
    pub fn take_id(&mut self) -> u32 {
        let after = self.rules.iter().map(|r| r.id + 1).max().unwrap_or(1);
        let id = self.next_id.max(after);
        self.next_id = id + 1;
        id
    }

    pub fn get(&self, id: u32) -> Option<&Rule> {
        self.rules.iter().find(|r| r.id == id)
    }

    /// Put `rule` in place of the rule with its id, or at the end.
    pub fn put(&mut self, rule: Rule) {
        self.next_id = self.next_id.max(rule.id + 1);
        match self.rules.iter_mut().find(|r| r.id == rule.id) {
            Some(slot) => *slot = rule,
            None => self.rules.push(rule),
        }
    }

    pub fn remove(&mut self, id: u32) {
        self.rules.retain(|r| r.id != id);
    }

    /// Move the rule `id` to position `to` (clamped to the end).
    pub fn reorder(&mut self, id: u32, to: usize) {
        if let Some(from) = self.rules.iter().position(|r| r.id == id) {
            let rule = self.rules.remove(from);
            let to = to.min(self.rules.len());
            self.rules.insert(to, rule);
        }
    }
}

/// When a rule looks at mail.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum When {
    /// As each message arrives.
    #[default]
    Arrives,
    /// Over mail already here, as it ages: "archive newsletters read more
    /// than a week ago". Always run by Postrinch.
    Waited,
}

/// How a rule's conditions combine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Match {
    #[default]
    All,
    Any,
}

/// One condition: a test, or its opposite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Condition {
    pub test: Test,
    #[serde(default)]
    pub negate: bool,
}

impl Condition {
    pub fn is(test: Test) -> Self {
        Self {
            test,
            negate: false,
        }
    }

    pub fn not(test: Test) -> Self {
        Self { test, negate: true }
    }
}

/// What a condition looks at. Words are compared whole, in any case
/// ([`crate::words`]); addresses by their lowercased form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Test {
    /// From exactly this address.
    FromAddress(String),
    /// From this domain, not its subdomains.
    FromDomain(String),
    /// The From header (name or address) has these words.
    FromWords(String),
    /// To, Cc or Bcc is this address.
    RecipientAddress(String),
    /// To (not Cc or Bcc) is this address.
    ToAddress(String),
    /// Cc is this address.
    CcAddress(String),
    SubjectWords(String),
    SubjectPhrase(String),
    /// The subject, the body, a sender or recipient, or an attachment's
    /// name has these words: what Gmail searches.
    Words(String),
    Phrase(String),
    HasAttachment,
    /// An attachment's name ends in `.<this>`, in any case.
    AttachmentType(String),
    /// An attachment is named exactly this, in any case.
    AttachmentName(String),
    /// The whole message is larger than this many bytes.
    LargerThan(u64),
    SmallerThan(u64),
    // What has happened to it since it arrived; always local.
    Unread,
    Starred,
    /// Carries this label (provider id).
    HasLabel(String),
    /// In this inbox category, as Gmail sorted it.
    InCategory(Category),
    InInbox,
    /// Arrived more than this many days ago.
    OlderThanDays(u32),
    /// Gmail's own search, as a filter imported from Gmail had it: only
    /// Gmail can run it, so here it matches nothing.
    GmailSearch(String),
}

impl Test {
    /// Whether it describes the message as it arrived, rather than what
    /// has happened to it since.
    pub fn is_arrival(&self) -> bool {
        !matches!(
            self,
            Test::Unread
                | Test::Starred
                | Test::HasLabel(_)
                | Test::InCategory(_)
                | Test::InInbox
                | Test::OlderThanDays(_)
        )
    }

    /// Whether it reads the body or attachments, which a message has only
    /// once it is downloaded.
    pub fn needs_content(&self) -> bool {
        matches!(
            self,
            Test::Words(_)
                | Test::Phrase(_)
                | Test::HasAttachment
                | Test::AttachmentType(_)
                | Test::AttachmentName(_)
        )
    }
}

/// A Gmail inbox category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category {
    Primary,
    Social,
    Promotions,
    Updates,
    Forums,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::Primary,
        Category::Social,
        Category::Promotions,
        Category::Updates,
        Category::Forums,
    ];

    /// Gmail's label id for it.
    pub fn label_id(self) -> &'static str {
        match self {
            Category::Primary => "CATEGORY_PERSONAL",
            Category::Social => "CATEGORY_SOCIAL",
            Category::Promotions => "CATEGORY_PROMOTIONS",
            Category::Updates => "CATEGORY_UPDATES",
            Category::Forums => "CATEGORY_FORUMS",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Category::Primary => "Primary",
            Category::Social => "Social",
            Category::Promotions => "Promotions",
            Category::Updates => "Updates",
            Category::Forums => "Forums",
        }
    }
}

/// What a rule does to the mail it matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    /// Add this label (provider id).
    AddLabel(String),
    SkipInbox,
    MarkRead,
    Star,
    MarkImportant,
    NeverImportant,
    Category(Category),
    /// Only on arrival, and only Gmail can do it.
    NeverSpam,
    Trash,
    /// Forward to this address, which Gmail has verified for forwarding.
    Forward(String),
    /// Ask an agent (by its registry id) to do something with the message.
    AskAgent {
        agent: String,
        prompt: String,
    },
}

impl Action {
    /// Whether it sends mail out of the account, which saving asks about.
    pub fn sends_mail(&self) -> bool {
        matches!(self, Action::Forward(_))
    }
}

/// One rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// Unique in its account; names its marker label.
    pub id: u32,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub when: When,
    #[serde(default)]
    pub matching: Match,
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
    /// Later rules don't see mail this rule matched.
    #[serde(default)]
    pub stop: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuleError {
    #[error("A rule needs a name.")]
    NoName,
    #[error("A rule needs at least one condition.")]
    NoConditions,
    #[error("A rule needs at least one thing to do.")]
    NoActions,
    #[error("{0}")]
    Condition(String),
    #[error("{0}")]
    Action(String),
}

impl Rule {
    /// A new, empty rule that arrives and is on.
    pub fn new(id: u32, name: &str) -> Self {
        Self {
            id,
            name: name.to_owned(),
            enabled: true,
            when: When::Arrives,
            matching: Match::All,
            conditions: Vec::new(),
            actions: Vec::new(),
            stop: false,
        }
    }

    /// The label Gmail adds for Postrinch to finish a split rule.
    pub fn marker(&self) -> String {
        marker_name(self.id)
    }

    /// Whether every condition and action is complete and makes sense, in
    /// the editor's words when not.
    pub fn check(&self) -> Result<(), RuleError> {
        if self.name.trim().is_empty() {
            return Err(RuleError::NoName);
        }
        if self.conditions.is_empty() {
            return Err(RuleError::NoConditions);
        }
        if self.actions.is_empty() {
            return Err(RuleError::NoActions);
        }
        for condition in &self.conditions {
            check_test(&condition.test, self.when)?;
        }
        for action in &self.actions {
            check_action(action, self.when)?;
        }
        Ok(())
    }

    /// Whether saving it should ask first: it sends mail out.
    pub fn sends_mail(&self) -> bool {
        self.actions.iter().any(Action::sends_mail)
    }
}

/// The marker label's name for rule `id`.
pub fn marker_name(id: u32) -> String {
    format!("{MARKER_PREFIX}r{id}")
}

fn check_test(test: &Test, when: When) -> Result<(), RuleError> {
    let bad = |s: &str| Err(RuleError::Condition(s.to_owned()));
    match test {
        Test::FromAddress(a)
        | Test::RecipientAddress(a)
        | Test::ToAddress(a)
        | Test::CcAddress(a) => {
            if EmailAddress::parse(a).is_err() {
                return bad(&format!("\u{201c}{a}\u{201d} is not an email address."));
            }
        }
        Test::FromDomain(d) => {
            let d = d.trim().trim_start_matches('@');
            if d.is_empty() || d.contains(char::is_whitespace) || !d.contains('.') {
                return bad(&format!("\u{201c}{d}\u{201d} is not a domain."));
            }
        }
        Test::FromWords(w)
        | Test::SubjectWords(w)
        | Test::SubjectPhrase(w)
        | Test::Words(w)
        | Test::Phrase(w) => {
            if crate::words(w).is_empty() {
                return bad("Words need at least one letter or digit.");
            }
        }
        Test::AttachmentType(t) => {
            let t = t.trim().trim_start_matches('.');
            if t.is_empty() || !t.chars().all(char::is_alphanumeric) {
                return bad("A file type is letters and digits, such as pdf.");
            }
        }
        Test::AttachmentName(n) => {
            if n.trim().is_empty() {
                return bad("An attachment name can't be empty.");
            }
        }
        Test::LargerThan(n) | Test::SmallerThan(n) => {
            if *n == 0 {
                return bad("A size needs to be more than nothing.");
            }
        }
        Test::OlderThanDays(n) => {
            if *n == 0 {
                return bad("Older than needs at least one day.");
            }
            if when == When::Arrives {
                return bad("\u{201c}Older than\u{201d} is for mail that has been here a while.");
            }
        }
        Test::HasLabel(l) => {
            if l.is_empty() {
                return bad("Pick a label.");
            }
        }
        Test::GmailSearch(q) => {
            if q.trim().is_empty() {
                return bad("A Gmail search can't be empty.");
            }
        }
        Test::HasAttachment
        | Test::Unread
        | Test::Starred
        | Test::InInbox
        | Test::InCategory(_) => {}
    }
    Ok(())
}

fn check_action(action: &Action, when: When) -> Result<(), RuleError> {
    let bad = |s: &str| Err(RuleError::Action(s.to_owned()));
    match action {
        Action::AddLabel(l) if l.is_empty() => bad("Pick a label."),
        Action::Forward(a) if EmailAddress::parse(a).is_err() => {
            bad("Pick an address to forward to.")
        }
        Action::AskAgent { agent, prompt } if agent.is_empty() || prompt.trim().is_empty() => {
            bad("Pick an agent and say what to ask it.")
        }
        Action::NeverSpam | Action::Forward(_) if when == When::Waited => {
            bad("That only happens as mail arrives.")
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_never_reused() {
        let mut set = RuleSet::default();
        let a = set.take_id();
        let b = set.take_id();
        assert_eq!((a, b), (1, 2));
        set.put(Rule::new(a, "A"));
        set.put(Rule::new(b, "B"));
        set.remove(b);
        assert_eq!(set.take_id(), 3);
    }

    #[test]
    fn reorder_moves_one_rule() {
        let mut set = RuleSet::default();
        for name in ["A", "B", "C"] {
            let id = set.take_id();
            set.put(Rule::new(id, name));
        }
        set.reorder(3, 0);
        let names: Vec<_> = set.rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["C", "A", "B"]);
        set.reorder(3, 99);
        let names: Vec<_> = set.rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["A", "B", "C"]);
    }

    #[test]
    fn check_names_what_is_missing() {
        let mut rule = Rule::new(1, "School");
        assert_eq!(rule.check(), Err(RuleError::NoConditions));
        rule.conditions
            .push(Condition::is(Test::FromDomain("westfield.edu".into())));
        assert_eq!(rule.check(), Err(RuleError::NoActions));
        rule.actions.push(Action::AddLabel("Label_1".into()));
        assert_eq!(rule.check(), Ok(()));
        rule.conditions.push(Condition::is(Test::OlderThanDays(7)));
        assert!(matches!(rule.check(), Err(RuleError::Condition(_))));
        rule.when = When::Waited;
        assert_eq!(rule.check(), Ok(()));
        rule.actions
            .push(Action::Forward("books@example.com".into()));
        assert!(matches!(rule.check(), Err(RuleError::Action(_))));
    }

    #[test]
    fn old_records_read_with_defaults() {
        let json = r#"{"rules":[{"id":4,"name":"R","conditions":[{"test":"InInbox"}],"actions":["Star"]}]}"#;
        let set: RuleSet = serde_json::from_str(json).unwrap();
        let rule = &set.rules[0];
        assert!(rule.enabled && !rule.stop && !rule.conditions[0].negate);
        assert_eq!((rule.when, rule.matching), (When::Arrives, Match::All));
        assert_eq!(set.next_id, 1);
        assert_eq!(rule.marker(), "[Postrinch]/r4");
    }
}
