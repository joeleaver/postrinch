//! Compiling a rule set to Gmail filters: which rules Gmail runs, which
//! Postrinch runs, and which are split between the two (docs/rules.md, 3).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::words::words;
use crate::{Action, Condition, Match, Rule, RuleSet, Test, When};

/// How many filters Gmail keeps for an account.
pub const FILTER_LIMIT: usize = 1000;
/// The longest query the compiler writes; Gmail took 10,000 characters in
/// testing, but whether so long a query still matches as written was not
/// tried.
pub const QUERY_LIMIT: usize = 1500;

/// One Gmail filter, as Postrinch wants it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Filter {
    /// The rule it is for.
    pub rule: u32,
    /// Gmail search: the filter's `criteria.query`.
    pub query: String,
    /// Label ids to add and remove, sorted.
    pub add: Vec<String>,
    pub remove: Vec<String>,
    pub forward: Option<String>,
    /// Also add the rule's marker label ([`Rule::marker`]), whose id sync
    /// looks up or creates.
    pub marker: bool,
}

/// What Postrinch does itself for one rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// Match and act: none of it runs on Gmail.
    Whole,
    /// Gmail matched and marked it; do these and remove the marker.
    AfterMarker(Vec<Action>),
}

/// The compiler's answer for one account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub filters: Vec<Filter>,
    /// By rule id, for every enabled rule Postrinch has a part in.
    pub local: Vec<(u32, Part)>,
}

impl Plan {
    pub fn part(&self, rule: u32) -> Option<&Part> {
        self.local
            .iter()
            .find(|(id, _)| *id == rule)
            .map(|(_, p)| p)
    }
}

/// What the compiler needs to know about the account.
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// Whether Postrinch may make filters (the grant has the settings
    /// scope). When not, every rule runs here.
    pub server: bool,
    /// Rules Gmail refused, which run here until it takes them.
    pub refused: BTreeSet<u32>,
    /// Filters made elsewhere, which count against [`FILTER_LIMIT`].
    pub others: usize,
}

/// What Gmail runs and what Postrinch runs for `set`.
pub fn compile(set: &RuleSet, context: &Context) -> Plan {
    let mut demoted = context.refused.clone();
    let room = FILTER_LIMIT.saturating_sub(context.others);
    loop {
        let plan = compile_once(set, context.server, &demoted);
        if plan.filters.len() <= room {
            return plan;
        }
        // Past the limit: the rule that costs most filters runs here.
        let Some(costliest) = costliest(&plan.filters) else {
            return plan;
        };
        demoted.insert(costliest);
    }
}

fn costliest(filters: &[Filter]) -> Option<u32> {
    let mut counts: Vec<(usize, u32)> = Vec::new();
    for f in filters {
        match counts.iter_mut().find(|(_, r)| *r == f.rule) {
            Some((n, _)) => *n += 1,
            None => counts.push((1, f.rule)),
        }
    }
    counts.into_iter().max().map(|(_, r)| r)
}

fn compile_once(set: &RuleSet, server: bool, demoted: &BTreeSet<u32>) -> Plan {
    let mut plan = Plan::default();
    // Queries of earlier "stop" rules that Gmail runs, which later rules'
    // filters leave out; `None` once one Gmail can't run stops the way.
    let mut stops: Option<Vec<String>> = Some(Vec::new());
    for rule in set.rules.iter().filter(|r| r.enabled) {
        if rule.when == When::Waited {
            plan.local.push((rule.id, Part::Whole));
            continue;
        }
        let query = if server && !demoted.contains(&rule.id) {
            stops.as_ref().and_then(|s| rule_query(rule, s))
        } else {
            None
        };
        let Some(parts) = query else {
            plan.local.push((rule.id, Part::Whole));
            if rule.stop {
                stops = None;
            }
            continue;
        };
        let (filters, rest) = filters_for(rule, &parts);
        plan.filters.extend(filters);
        if !rest.is_empty() {
            plan.local.push((rule.id, Part::AfterMarker(rest)));
        }
        if rule.stop
            && let (Some(s), Some(q)) = (stops.as_mut(), own_query(rule))
        {
            s.push(q);
        }
    }
    plan
}

/// The rule's conditions as one query, without stops; `None` when any
/// condition is local.
fn own_query(rule: &Rule) -> Option<String> {
    let terms: Vec<String> = rule.conditions.iter().map(term).collect::<Option<_>>()?;
    Some(match (rule.matching, terms.len()) {
        (_, 1) => terms[0].clone(),
        (Match::All, _) => terms.join(" "),
        (Match::Any, _) => format!("({})", terms.join(" OR ")),
    })
}

/// The queries the rule's filters use, each under [`QUERY_LIMIT`]: one,
/// or for a long "any of" several that together match the same mail.
/// `None` when Gmail can't run it.
fn rule_query(rule: &Rule, stops: &[String]) -> Option<Vec<String>> {
    let suffix: String = stops.iter().map(|q| format!(" -({q})")).collect();
    let whole = format!("{}{suffix}", own_query(rule)?);
    if whole.len() <= QUERY_LIMIT {
        return Some(vec![whole]);
    }
    if rule.matching == Match::All || rule.conditions.len() < 2 {
        return None;
    }
    // "Any of": several filters, each with some of the alternatives.
    let terms: Vec<String> = rule.conditions.iter().map(term).collect::<Option<_>>()?;
    let mut queries = Vec::new();
    let mut group: Vec<String> = Vec::new();
    let fits = |g: &[String]| format!("({}){suffix}", g.join(" OR ")).len() <= QUERY_LIMIT;
    for t in terms {
        group.push(t);
        if !fits(&group) {
            let last = group.pop().expect("just pushed");
            if group.is_empty() {
                return None;
            }
            queries.push(format!("({}){suffix}", group.join(" OR ")));
            group = vec![last];
            if !fits(&group) {
                return None;
            }
        }
    }
    if !group.is_empty() {
        queries.push(format!("({}){suffix}", group.join(" OR ")));
    }
    Some(queries)
}

/// The filters for a rule Gmail matches, and the actions left to
/// Postrinch. Gmail adds one user label per filter, so each further label,
/// and the marker, takes a filter of its own.
fn filters_for(rule: &Rule, queries: &[String]) -> (Vec<Filter>, Vec<Action>) {
    let mut first = Filter {
        rule: rule.id,
        query: String::new(),
        add: Vec::new(),
        remove: Vec::new(),
        forward: None,
        marker: false,
    };
    let mut labels = Vec::new();
    let mut rest = Vec::new();
    for action in &rule.actions {
        match action {
            Action::AddLabel(id) => labels.push(id.clone()),
            Action::SkipInbox => first.remove.push("INBOX".into()),
            Action::MarkRead => first.remove.push("UNREAD".into()),
            Action::Star => first.add.push("STARRED".into()),
            Action::MarkImportant => first.add.push("IMPORTANT".into()),
            Action::NeverImportant => first.remove.push("IMPORTANT".into()),
            Action::Category(c) => first.add.push(c.label_id().into()),
            Action::NeverSpam => first.remove.push("SPAM".into()),
            Action::Trash => first.add.push("TRASH".into()),
            Action::Forward(to) => first.forward = Some(to.clone()),
            Action::AskAgent { .. } => rest.push(action.clone()),
        }
    }
    let mut labels = labels.into_iter();
    if let Some(label) = labels.next() {
        first.add.push(label);
    }
    let mut each = vec![first];
    each.extend(labels.map(|label| Filter {
        add: vec![label],
        ..empty(rule.id)
    }));
    if !rest.is_empty() {
        match each.iter_mut().find(|f| !has_user_label(f)) {
            Some(f) => f.marker = true,
            None => each.push(Filter {
                marker: true,
                ..empty(rule.id)
            }),
        }
    }
    each.retain(|f| !f.add.is_empty() || !f.remove.is_empty() || f.forward.is_some() || f.marker);
    let mut filters = Vec::new();
    for query in queries {
        for f in &each {
            let mut f = f.clone();
            f.query.clone_from(query);
            f.add.sort();
            f.add.dedup();
            f.remove.sort();
            f.remove.dedup();
            filters.push(f);
        }
    }
    (filters, rest)
}

fn empty(rule: u32) -> Filter {
    Filter {
        rule,
        query: String::new(),
        add: Vec::new(),
        remove: Vec::new(),
        forward: None,
        marker: false,
    }
}

fn has_user_label(f: &Filter) -> bool {
    f.marker || f.add.iter().any(|l| !is_system(l))
}

fn is_system(id: &str) -> bool {
    matches!(
        id,
        "STARRED" | "IMPORTANT" | "TRASH" | "UNREAD" | "INBOX" | "SPAM"
    ) || id.starts_with("CATEGORY_")
}

/// A condition as a Gmail search term, when Gmail matches exactly the same
/// mail; `None` when it must run here (docs/rules.md, 1).
fn term(condition: &Condition) -> Option<String> {
    let positive = match &condition.test {
        Test::FromAddress(a) => format!("from:({})", address(a)?),
        Test::RecipientAddress(a) => format!("to:({})", address(a)?),
        Test::CcAddress(a) => format!("cc:({})", address(a)?),
        Test::FromWords(w) => format!("from:({})", word_list(w)?),
        Test::SubjectWords(w) => format!("subject:({})", word_list(w)?),
        Test::SubjectPhrase(p) => format!("subject:\"{}\"", word_list(p)?),
        Test::Words(w) => format!("({})", word_list(w)?),
        Test::Phrase(p) => format!("\"{}\"", word_list(p)?),
        Test::HasAttachment if !condition.negate => "has:attachment".into(),
        Test::AttachmentType(t) => format!("filename:{}", word_list(t.trim_start_matches('.'))?),
        Test::AttachmentName(n) => format!("filename:{}", file_name(n)?),
        Test::GmailSearch(q) => format!("({})", q.trim()),
        Test::LargerThan(n) => format!("larger:{n}"),
        Test::SmallerThan(n) => format!("smaller:{n}"),
        // Gmail's meaning is broader (domains), different (To only, its
        // categories), or the test is about what happened since.
        Test::HasAttachment
        | Test::FromDomain(_)
        | Test::ToAddress(_)
        | Test::Unread
        | Test::Starred
        | Test::HasLabel(_)
        | Test::InInbox
        | Test::OlderThanDays(_) => return None,
    };
    Some(if condition.negate {
        format!("-{positive}")
    } else {
        positive
    })
}

/// An address safe to put in a query as it is.
fn address(a: &str) -> Option<String> {
    let a = a.trim().to_lowercase();
    a.chars()
        .all(|c| c.is_alphanumeric() || "@.-_+'".contains(c))
        .then_some(a)
}

/// Words, as the matcher reads them, joined for Gmail. A lone word that
/// Gmail reads as an operator (OR, AND, AROUND) is lowercased by
/// [`words`], which Gmail reads as a word.
fn word_list(text: &str) -> Option<String> {
    let w = words(text);
    (!w.is_empty()).then(|| w.join(" "))
}

fn file_name(n: &str) -> Option<String> {
    let n = n.trim().to_lowercase();
    (!n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || "._-".contains(c))).then_some(n)
}

#[cfg(test)]
mod tests;
