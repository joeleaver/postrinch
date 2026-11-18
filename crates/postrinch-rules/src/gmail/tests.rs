use super::*;
use crate::{Category, Condition, Rule, RuleSet, Test};

fn server() -> Context {
    Context {
        server: true,
        ..Context::default()
    }
}

fn rule(id: u32, conditions: Vec<Condition>, actions: Vec<Action>) -> Rule {
    Rule {
        conditions,
        actions,
        ..Rule::new(id, "R")
    }
}

fn set(rules: Vec<Rule>) -> RuleSet {
    RuleSet {
        rules,
        next_id: 100,
    }
}

fn from(a: &str) -> Condition {
    Condition::is(Test::FromAddress(a.into()))
}

#[test]
fn a_rule_gmail_can_run_is_one_filter() {
    let r = rule(
        1,
        vec![
            from("billing@acme.example"),
            Condition::is(Test::HasAttachment),
        ],
        vec![
            Action::AddLabel("Label_3".into()),
            Action::SkipInbox,
            Action::MarkRead,
        ],
    );
    let plan = compile(&set(vec![r]), &server());
    assert_eq!(
        plan.filters,
        [Filter {
            rule: 1,
            query: "from:(billing@acme.example) has:attachment".into(),
            add: vec!["Label_3".into()],
            remove: vec!["INBOX".into(), "UNREAD".into()],
            forward: None,
            marker: false,
        }]
    );
    assert!(plan.local.is_empty());
}

#[test]
fn local_conditions_keep_the_whole_rule_here() {
    let r = rule(
        2,
        vec![Condition::is(Test::FromDomain("westfield.edu".into()))],
        vec![Action::AddLabel("Label_1".into())],
    );
    let plan = compile(&set(vec![r]), &server());
    assert!(plan.filters.is_empty());
    assert_eq!(plan.part(2), Some(&Part::Whole));
}

#[test]
fn without_the_scope_everything_is_here() {
    let r = rule(1, vec![from("a@example.com")], vec![Action::Star]);
    let plan = compile(&set(vec![r]), &Context::default());
    assert!(plan.filters.is_empty());
    assert_eq!(plan.part(1), Some(&Part::Whole));
}

#[test]
fn an_agent_action_splits_the_rule_with_a_marker() {
    let ask = Action::AskAgent {
        agent: "claude-code".into(),
        prompt: "Summarise it".into(),
    };
    let r = rule(
        5,
        vec![Condition::is(Test::SubjectWords("Field trip".into()))],
        vec![
            Action::AddLabel("Label_1".into()),
            ask.clone(),
            Action::Star,
        ],
    );
    let plan = compile(&set(vec![r]), &server());
    let queries: Vec<_> = plan.filters.iter().map(|f| f.query.as_str()).collect();
    assert_eq!(queries, ["subject:(field trip)", "subject:(field trip)"]);
    assert_eq!(plan.filters[0].add, ["Label_1", "STARRED"]);
    assert!(!plan.filters[0].marker, "one user label per filter");
    assert!(plan.filters[1].marker);
    assert_eq!(plan.part(5), Some(&Part::AfterMarker(vec![ask])));
}

#[test]
fn the_marker_shares_a_filter_without_a_user_label() {
    let r = rule(
        6,
        vec![from("a@example.com")],
        vec![
            Action::Star,
            Action::AskAgent {
                agent: "x".into(),
                prompt: "p".into(),
            },
        ],
    );
    let plan = compile(&set(vec![r]), &server());
    assert_eq!(plan.filters.len(), 1);
    assert!(plan.filters[0].marker);
}

#[test]
fn each_extra_label_is_another_filter() {
    let r = rule(
        1,
        vec![from("a@example.com")],
        vec![
            Action::AddLabel("Label_1".into()),
            Action::AddLabel("Label_2".into()),
            Action::Category(Category::Updates),
        ],
    );
    let plan = compile(&set(vec![r]), &server());
    let adds: Vec<_> = plan.filters.iter().map(|f| f.add.clone()).collect();
    assert_eq!(
        adds,
        [
            vec!["CATEGORY_UPDATES".to_owned(), "Label_1".into()],
            vec!["Label_2".into()]
        ]
    );
}

#[test]
fn any_of_and_not() {
    let r = Rule {
        matching: Match::Any,
        ..rule(
            1,
            vec![
                Condition::is(Test::SubjectWords("receipt".into())),
                Condition::is(Test::SubjectPhrase("your order".into())),
                Condition::not(Test::FromWords("noreply".into())),
            ],
            vec![Action::Star],
        )
    };
    let plan = compile(&set(vec![r]), &server());
    assert_eq!(
        plan.filters[0].query,
        "(subject:(receipt) OR subject:\"your order\" OR -from:(noreply))"
    );
    let no_attachment = rule(
        2,
        vec![Condition::not(Test::HasAttachment)],
        vec![Action::Star],
    );
    let plan = compile(&set(vec![no_attachment]), &server());
    assert_eq!(
        plan.part(2),
        Some(&Part::Whole),
        "Gmail can't say 'no attachment'"
    );
}

#[test]
fn stop_leaves_the_earlier_rules_mail_out_of_later_filters() {
    let first = Rule {
        stop: true,
        ..rule(1, vec![from("boss@example.com")], vec![Action::Star])
    };
    let second = rule(
        2,
        vec![Condition::is(Test::SubjectWords("lunch".into()))],
        vec![Action::Trash],
    );
    let plan = compile(&set(vec![first, second]), &server());
    assert_eq!(
        plan.filters[1].query,
        "subject:(lunch) -(from:(boss@example.com))"
    );
}

#[test]
fn a_local_stop_rule_keeps_later_rules_here() {
    let first = Rule {
        stop: true,
        ..rule(
            1,
            vec![Condition::is(Test::FromDomain("example.com".into()))],
            vec![Action::Star],
        )
    };
    let second = rule(2, vec![from("a@example.org")], vec![Action::Trash]);
    let plan = compile(&set(vec![first, second]), &server());
    assert!(plan.filters.is_empty());
    assert_eq!(plan.part(2), Some(&Part::Whole));
}

#[test]
fn off_and_waiting_rules() {
    let off = Rule {
        enabled: false,
        ..rule(1, vec![from("a@example.com")], vec![Action::Star])
    };
    let waits = Rule {
        when: When::Waited,
        ..rule(
            2,
            vec![Condition::is(Test::OlderThanDays(7))],
            vec![Action::SkipInbox],
        )
    };
    let plan = compile(&set(vec![off, waits]), &server());
    assert!(plan.filters.is_empty());
    assert_eq!(plan.local, [(2, Part::Whole)]);
}

#[test]
fn refused_rules_run_here() {
    let r = rule(
        3,
        vec![from("a@example.com")],
        vec![Action::Forward("b@example.com".into())],
    );
    let context = Context {
        refused: [3].into(),
        ..server()
    };
    let plan = compile(&set(vec![r]), &context);
    assert!(plan.filters.is_empty());
    assert_eq!(plan.part(3), Some(&Part::Whole));
}

#[test]
fn past_the_filter_limit_the_costliest_rule_runs_here() {
    let labels: Vec<Action> = (0..5)
        .map(|i| Action::AddLabel(format!("Label_{i}")))
        .collect();
    let big = rule(1, vec![from("a@example.com")], labels);
    let small = rule(2, vec![from("b@example.com")], vec![Action::Star]);
    let context = Context {
        others: FILTER_LIMIT - 4,
        ..server()
    };
    let plan = compile(&set(vec![big, small]), &context);
    assert_eq!(plan.filters.len(), 1);
    assert_eq!(plan.filters[0].rule, 2);
    assert_eq!(plan.part(1), Some(&Part::Whole));
}

#[test]
fn a_long_any_of_splits_into_several_filters() {
    let conditions: Vec<Condition> = (0..120)
        .map(|i| from(&format!("sender{i:03}@example.com")))
        .collect();
    let r = Rule {
        matching: Match::Any,
        ..rule(1, conditions, vec![Action::Star])
    };
    let plan = compile(&set(vec![r]), &server());
    assert!(plan.filters.len() > 1);
    assert!(plan.filters.iter().all(|f| f.query.len() <= QUERY_LIMIT));
    let total: usize = plan
        .filters
        .iter()
        .map(|f| f.query.matches(" OR ").count() + 1)
        .sum();
    assert_eq!(total, 120);
}

#[test]
fn odd_addresses_and_names_run_here() {
    let r = rule(1, vec![from("\"odd\"(x)@example.com")], vec![Action::Star]);
    assert!(compile(&set(vec![r]), &server()).filters.is_empty());
    let r = rule(
        1,
        vec![Condition::is(Test::AttachmentName("my report.pdf".into()))],
        vec![Action::Star],
    );
    assert!(compile(&set(vec![r]), &server()).filters.is_empty());
    let r = rule(
        1,
        vec![Condition::is(Test::AttachmentType(".PDF".into()))],
        vec![Action::Star],
    );
    assert_eq!(
        compile(&set(vec![r]), &server()).filters[0].query,
        "filename:pdf"
    );
}
