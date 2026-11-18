//! The reader's More menu, after its mockup: "Make a rule" for mail like
//! the open conversation's latest message, each suggestion with how many
//! conversations it would have caught, and starting from scratch.

use postrinch_model::{EmailAddress, Message};
use postrinch_rules::{Condition, Rule, Test};
use rinch::prelude::*;

use crate::actions::commands;
use crate::rule_draft::Draft;
use crate::rules_ui::now;
use crate::state::{SettingsSection, State, View};
use crate::ui::icons::{Icon, icon};

/// One suggestion: what it says, and the condition it starts the rule with.
#[derive(Debug, Clone, PartialEq)]
struct Suggestion {
    line: String,
    condition: Condition,
    /// Conversations it would have caught, once counted.
    count: Option<usize>,
}

/// The latest message of the open conversation that someone else sent.
fn latest(state: State) -> Option<(EmailAddress, Message)> {
    let thread = state.thread.get()?;
    let account = thread.summary.account.clone();
    let message = thread
        .messages
        .iter()
        .rev()
        .find(|m| {
            m.headers
                .from
                .iter()
                .all(|f| f.address.key() != account.key())
        })
        .or_else(|| thread.messages.last())?
        .clone();
    Some((account, message))
}

/// The subject without Re:/Fwd: and at most four words, for "with … in
/// the subject".
fn subject_words(subject: &str) -> Option<String> {
    let mut s = subject.trim();
    loop {
        let lower = s.to_lowercase();
        let cut = ["re:", "fwd:", "fw:"]
            .iter()
            .find(|p| lower.starts_with(*p));
        match cut {
            Some(p) => s = s[p.len()..].trim_start(),
            None => break,
        }
    }
    let words: Vec<&str> = s.split_whitespace().take(4).collect();
    (!words.is_empty()).then(|| words.join(" "))
}

fn suggestions(message: &Message) -> Vec<Suggestion> {
    let mut all = Vec::new();
    if let Some(from) = message.headers.from.first() {
        let address = from.address.to_string();
        all.push(Suggestion {
            line: format!("From {address}"),
            condition: Condition::is(Test::FromAddress(address)),
            count: None,
        });
        let domain = from.address.domain().to_owned();
        all.push(Suggestion {
            line: format!("From anyone at {domain}"),
            condition: Condition::is(Test::FromDomain(domain)),
            count: None,
        });
    }
    if let Some(words) = subject_words(message.subject()) {
        all.push(Suggestion {
            line: format!("With \u{201c}{words}\u{201d} in the subject"),
            condition: Condition::is(Test::SubjectPhrase(words)),
            count: None,
        });
    }
    all
}

#[component]
pub fn rule_menu(state: State) -> NodeHandle {
    let open = Signal::new(false);
    let found: Signal<Vec<Suggestion>> = Signal::new(Vec::new());
    let toggle = move || {
        let now_open = !open.get();
        open.set(now_open);
        if !now_open {
            return;
        }
        let Some((account, message)) = latest(state) else {
            found.set(Vec::new());
            return;
        };
        let first = suggestions(&message);
        found.set(first.clone());
        // Count each on a thread: it reads the last three months' mail.
        if let Some(commands) = commands() {
            let commands = (*commands).clone();
            std::thread::spawn(move || {
                let counted: Vec<Suggestion> = first
                    .into_iter()
                    .map(|mut s| {
                        let rule = Rule {
                            conditions: vec![s.condition.clone()],
                            ..Rule::new(0, "")
                        };
                        s.count = commands
                            .already_here(&account, &rule, now())
                            .ok()
                            .map(|h| h.threads.len());
                        s
                    })
                    .collect();
                found.send(counted);
            });
        }
    };
    let start = move |condition: Option<Condition>| {
        open.set(false);
        let Some((account, message)) = latest(state) else {
            return;
        };
        let mut draft = Draft::new();
        if let Some(c) = condition {
            let named = Rule {
                conditions: vec![c],
                ..Rule::new(0, "")
            };
            let from = Draft::of(&named);
            draft.looks = from.looks;
            draft.rows = from.rows + 1;
            draft.does[0].id = draft.rows;
            draft.name = message
                .headers
                .from
                .first()
                .map(|f| f.display_name())
                .unwrap_or_default();
        }
        state.settings_section.set(SettingsSection::Rules);
        state.view.set(View::Settings);
        state.rules.edit(account, draft);
    };
    rsx! {
        Popover {
            opened_fn: move || open.get(),
            onclose: move || open.set(false),
            position: "bottom-end",
            width: "320px",
            PopoverTarget {
                button {
                    class: {move || if open.get() { "icon-button open" } else { "icon-button" }},
                    title: "More",
                    aria-label: "More",
                    aria-haspopup: "menu",
                    onclick: toggle,
                    {icon(__scope, Icon::More)}
                }
            }
            PopoverDropdown {
                div { class: "rule-menu", role: "menu", aria-label: "Make a rule",
                    span { class: "rule-menu-head", "Make a rule for mail like this" }
                    for s in found.get() {
                        div {
                            key: s.line.clone(),
                            class: "rule-menu-item",
                            role: "menuitem",
                            onclick: {
                                let c = s.condition.clone();
                                move || start(Some(c.clone()))
                            },
                            span { {s.line.clone()} }
                            span { class: "rule-menu-count", {move || count_line(found, &s.line)} }
                        }
                    }
                    div { class: "rule-menu-gap" }
                    div { class: "rule-menu-item", role: "menuitem", onclick: move || start(None), "Start from scratch\u{2026}" }
                }
            }
        }
    }
}

fn count_line(found: Signal<Vec<Suggestion>>, line: &str) -> String {
    match found.with(|all| all.iter().find(|s| s.line == line).and_then(|s| s.count)) {
        None => "Counting\u{2026}".into(),
        Some(1) => "1 conversation so far".into(),
        Some(n) => format!("{n} conversations so far"),
    }
}

pub const CSS: &str = r#"
.rule-menu { display: flex; flex-direction: column; padding: 5px; }
.rule-menu-head { padding: 6px 12px 4px; font-size: 12px; color: var(--muted); }
.rule-menu-item { display: flex; flex-direction: column; gap: 1px; padding: 8px 12px; border-radius: 6px; font-size: 13px; cursor: pointer; }
.rule-menu-item:hover { background: var(--hover); }
.rule-menu-count { font-size: 12px; color: var(--muted); }
.rule-menu-gap { height: 1px; margin: 4px 6px; background: var(--line); }
"#;

#[cfg(test)]
mod tests {
    use super::subject_words;

    #[test]
    fn subjects_lose_reply_prefixes_and_run_to_four_words() {
        assert_eq!(
            subject_words("Re: Fwd: Field trip forms due Friday next week").as_deref(),
            Some("Field trip forms due")
        );
        assert_eq!(subject_words("  "), None);
    }
}
