//! A rule in one line, for the rules list: "From anyone at westfield.edu →
//! label School, ask Claude Code for a summary".

use crate::{Action, Condition, Match, Rule, Test, When};

/// `rule` in a line. `label` names a label by its provider id, `agent` an
/// agent by its id.
pub fn summarize(
    rule: &Rule,
    label: &dyn Fn(&str) -> String,
    agent: &dyn Fn(&str) -> String,
) -> String {
    let joiner = match rule.matching {
        Match::All => " and ",
        Match::Any => " or ",
    };
    let conditions: Vec<String> = rule
        .conditions
        .iter()
        .map(|c| condition(c, label))
        .collect();
    let mut actions: Vec<String> = rule
        .actions
        .iter()
        .map(|a| action(a, label, agent))
        .collect();
    if rule.stop {
        actions.push("stop here".into());
    }
    let mut text = capitalize(&conditions.join(joiner));
    if rule.when == When::Waited
        && !rule
            .conditions
            .iter()
            .any(|c| matches!(c.test, Test::OlderThanDays(_)))
    {
        text.push_str(", once here a while");
    }
    format!("{text} \u{2192} {}", actions.join(", "))
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn quoted(s: &str) -> String {
    format!("\u{201c}{}\u{201d}", s.trim())
}

fn condition(c: &Condition, label: &dyn Fn(&str) -> String) -> String {
    let (yes, no) = match &c.test {
        Test::FromAddress(a) => (format!("from {a}"), format!("not from {a}")),
        Test::FromDomain(d) => {
            let d = d.trim_start_matches('@');
            (format!("from anyone at {d}"), format!("not from {d}"))
        }
        Test::FromWords(w) => (
            format!("from has {}", quoted(w)),
            format!("from doesn't have {}", quoted(w)),
        ),
        Test::RecipientAddress(a) => (format!("to {a}"), format!("not to {a}")),
        Test::ToAddress(a) => (format!("To is {a}"), format!("To isn't {a}")),
        Test::CcAddress(a) => (format!("Cc is {a}"), format!("Cc isn't {a}")),
        Test::SubjectWords(w) | Test::SubjectPhrase(w) => (
            format!("subject has {}", quoted(w)),
            format!("subject doesn't have {}", quoted(w)),
        ),
        Test::Words(w) | Test::Phrase(w) => (
            format!("has {}", quoted(w)),
            format!("doesn't have {}", quoted(w)),
        ),
        Test::HasAttachment => ("has an attachment".into(), "has no attachment".into()),
        Test::AttachmentType(t) => {
            let t = t.trim_start_matches('.').to_uppercase();
            (format!("has a {t}"), format!("has no {t}"))
        }
        Test::AttachmentName(n) => (
            format!("has {}", quoted(n)),
            format!("doesn't have {}", quoted(n)),
        ),
        Test::LargerThan(n) => (
            format!("larger than {}", size(*n)),
            format!("not larger than {}", size(*n)),
        ),
        Test::SmallerThan(n) => (
            format!("smaller than {}", size(*n)),
            format!("not smaller than {}", size(*n)),
        ),
        Test::GmailSearch(q) => (
            format!("Gmail search {}", quoted(q)),
            format!("not Gmail search {}", quoted(q)),
        ),
        Test::Unread => ("unread".into(), "read".into()),
        Test::Starred => ("starred".into(), "not starred".into()),
        Test::HasLabel(l) => (
            format!("labelled {}", label(l)),
            format!("not labelled {}", label(l)),
        ),
        Test::InCategory(c) => (format!("in {}", c.name()), format!("not in {}", c.name())),
        Test::InInbox => ("in the inbox".into(), "not in the inbox".into()),
        Test::OlderThanDays(d) => {
            let days = if *d == 1 {
                "1 day".into()
            } else {
                format!("{d} days")
            };
            (
                format!("older than {days}"),
                format!("not older than {days}"),
            )
        }
    };
    if c.negate { no } else { yes }
}

fn size(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes >= MB && bytes.is_multiple_of(MB) {
        format!("{} MB", bytes / MB)
    } else if bytes >= 1024 && bytes.is_multiple_of(1024) {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} bytes")
    }
}

fn action(a: &Action, label: &dyn Fn(&str) -> String, agent: &dyn Fn(&str) -> String) -> String {
    match a {
        Action::AddLabel(l) => format!("label {}", label(l)),
        Action::SkipInbox => "skip the inbox".into(),
        Action::MarkRead => "mark read".into(),
        Action::Star => "star".into(),
        Action::MarkImportant => "mark important".into(),
        Action::NeverImportant => "never important".into(),
        Action::Category(c) => format!("put in {}", c.name()),
        Action::NeverSpam => "never spam".into(),
        Action::Trash => "trash".into(),
        Action::Forward(to) => format!("forward to {to}"),
        Action::AskAgent { agent: id, .. } => format!("ask {}", agent(id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line() {
        let rule = Rule {
            conditions: vec![
                Condition::is(Test::FromDomain("westfield.edu".into())),
                Condition::not(Test::SubjectWords("lunch menu".into())),
            ],
            actions: vec![
                Action::AddLabel("Label_1".into()),
                Action::AskAgent {
                    agent: "claude-code".into(),
                    prompt: "Summarise".into(),
                },
            ],
            stop: true,
            ..Rule::new(1, "School")
        };
        let text = summarize(&rule, &|_| "School".into(), &|_| "Claude Code".into());
        assert_eq!(
            text,
            "From anyone at westfield.edu and subject doesn't have \u{201c}lunch menu\u{201d} \u{2192} label School, ask Claude Code, stop here"
        );
        assert_eq!(size(5 * 1024 * 1024), "5 MB");
    }
}
