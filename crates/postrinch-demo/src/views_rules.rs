//! The person's own views and mail rules, saved through the commands the
//! window uses, so each is checked as one made there would be.

use postrinch_commands::Commands;
use postrinch_model::{SavedView, Timestamp, ViewCount};
use postrinch_rules::{Action, Condition, Match, Rule, Test};

use crate::DemoResult;
use crate::accounts::{HARBOR, INVOICES, RECEIPTS};
use crate::people::{LEDGERLY, PAPER_CRANE, PRIYA, SAM_HOME, SAM_WORK};

/// Name, search, the account it is of (none for every account), count.
/// As many as a person who files by views keeps: enough that the sidebar
/// is full, as it is in the website's screenshots.
const VIEWS: [(&str, &str, Option<&str>, ViewCount); 9] = [
    (
        "Harbor Coffee",
        "label:clients/harbor-coffee",
        None,
        ViewCount::Unread,
    ),
    (
        "Waiting on others",
        "label:waiting",
        Some(SAM_WORK.address),
        ViewCount::All,
    ),
    (
        "Receipts",
        "label:receipts newer_than:30d",
        Some(SAM_HOME.address),
        ViewCount::None,
    ),
    ("Starred", "is:starred", None, ViewCount::All),
    (
        "Invoices",
        "label:invoices",
        Some(SAM_WORK.address),
        ViewCount::None,
    ),
    (
        "Hiring",
        "label:studio/hiring",
        Some(SAM_WORK.address),
        ViewCount::All,
    ),
    (
        "Travel",
        "label:travel",
        Some(SAM_HOME.address),
        ViewCount::None,
    ),
    (
        "Newsletters",
        "category:updates",
        Some(SAM_HOME.address),
        ViewCount::Unread,
    ),
    (
        "Files this month",
        "has:attachment newer_than:30d",
        None,
        ViewCount::None,
    ),
];

/// Save the views and rules. `made` is when the views were made: each
/// view's key is taken from it, a millisecond apart. Returns how many of
/// each.
pub fn put(commands: &Commands, made: Timestamp) -> DemoResult<(usize, usize)> {
    for (n, (name, query, account, count)) in VIEWS.into_iter().enumerate() {
        let made = Timestamp::from_millis(made.as_millis() + n as i64);
        let mut view = SavedView::new(name, query, made).expect("the demo's views are named");
        view.accounts = account
            .map(|a| postrinch_model::EmailAddress::parse(a).expect("a demo address"))
            .into_iter()
            .collect();
        view.count = count;
        commands.save_view(&view)?;
    }
    let rules = rules();
    for (account, rule) in &rules {
        commands.save_rule(&account.email(), rule.clone())?;
    }
    Ok((VIEWS.len(), rules.len()))
}

fn rule(name: &str, matching: Match, conditions: Vec<Test>, actions: Vec<Action>) -> Rule {
    let mut rule = Rule::new(0, name);
    rule.matching = matching;
    rule.conditions = conditions.into_iter().map(Condition::is).collect();
    rule.actions = actions;
    rule
}

fn rules() -> Vec<(crate::people::Person, Rule)> {
    vec![
        (
            SAM_WORK,
            rule(
                "Harbor Coffee",
                Match::All,
                vec![Test::FromDomain(PRIYA.domain().to_owned())],
                vec![Action::AddLabel(HARBOR.to_owned()), Action::MarkImportant],
            ),
        ),
        (
            SAM_WORK,
            rule(
                "Invoices from Ledgerly",
                Match::All,
                vec![Test::FromAddress(LEDGERLY.address.to_owned())],
                vec![Action::AddLabel(INVOICES.to_owned()), Action::SkipInbox],
            ),
        ),
        (
            SAM_HOME,
            rule(
                "Receipts",
                Match::Any,
                vec![
                    Test::SubjectWords("receipt".to_owned()),
                    Test::FromAddress(PAPER_CRANE.address.to_owned()),
                ],
                vec![Action::AddLabel(RECEIPTS.to_owned())],
            ),
        ),
    ]
}
