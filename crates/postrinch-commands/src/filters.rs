//! The search filters a window offers (who it is from and to, or not, when, unread,
//! starred, with attachments, who wrote last, which accounts, which labels),
//! read from and written into the query text, so the text stays the one
//! place a search lives: what the person types, what an agent sends and
//! what a saved view keeps.

use crate::query::{Last, label_matches, operator, tokens};

/// The operators the panel writes as a group of "any of these"
/// (`{label:a label:b}`, `{from:a from:b}`, `{to:a to:b}`).
const GROUPED: [&str; 3] = ["label", "from", "to"];

/// The groups the panel wrote, found in a query, by operator.
#[derive(Debug, Default)]
struct Groups {
    label: Vec<String>,
    from: Vec<String>,
    to: Vec<String>,
}

impl Groups {
    fn slot(&mut self, name: &str) -> &mut Vec<String> {
        match name {
            "label" => &mut self.label,
            "from" => &mut self.from,
            _ => &mut self.to,
        }
    }

    /// Whether the query had a group of `name`: beside one, a lone
    /// operator of the same name is the person's own.
    fn has(&self, name: &str) -> bool {
        match name {
            "label" => !self.label.is_empty(),
            "from" => !self.from.is_empty(),
            "to" => !self.to.is_empty(),
            _ => false,
        }
    }
}

/// What a filter asks of a label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelPick {
    Has,
    Not,
}

/// Archived mail (out of the inbox): shown with the rest, left out
/// (`in:inbox`), or shown alone (`-in:inbox`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Archived {
    #[default]
    Include,
    LeaveOut,
    Only,
}

/// The filters a query sets, as the filter panel shows them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    pub account: Option<String>,
    /// `newer_than:`'s value: `7d`, `1m`, `1y`.
    pub within: Option<String>,
    /// Senders it is from: any of them (`{from:a from:b}`), by a part of
    /// their address or name.
    pub from: Vec<String>,
    /// Senders it is not from (`-from:a`).
    pub not_from: Vec<String>,
    /// Recipients (To, Cc or Bcc) it is to: any of them.
    pub to: Vec<String>,
    /// Recipients it is not to (`-to:a`).
    pub not_to: Vec<String>,
    /// Labels a conversation has, by their provider's names: any of them
    /// (`{label:a label:b}`), or all of them when `all_labels`.
    pub labels: Vec<String>,
    pub all_labels: bool,
    /// Labels it has none of (`-label:a`).
    pub without: Vec<String>,
    pub unread: bool,
    pub starred: bool,
    pub attachments: bool,
    pub last: Option<Last>,
    pub archived: Archived,
}

impl Filters {
    /// The filters `query` sets. Text that does not read is no filter.
    pub fn read(query: &str) -> Self {
        let mut filters = Filters::default();
        let (rest, groups) = take_groups(query);
        let grouped_labels = groups.has("label");
        for (token, top) in levels(tokens(&rest).unwrap_or_default()) {
            if !top || !manages(&token, &groups) {
                continue;
            }
            let Some((negated, name, value)) = operator(&token) else {
                continue;
            };
            let value = value.trim_matches('"').to_owned();
            if name == "in" {
                filters.archived = if negated {
                    Archived::Only
                } else {
                    Archived::LeaveOut
                };
                continue;
            }
            if negated {
                match name {
                    "label" => filters.without.push(value),
                    "from" => filters.not_from.push(value),
                    _ => filters.not_to.push(value),
                }
                continue;
            }
            let lower = value.to_lowercase();
            match (name, lower.as_str()) {
                ("account", _) => filters.account = Some(value),
                ("newer_than", _) => filters.within = Some(value),
                ("from", _) => filters.from.push(value),
                ("to", _) => filters.to.push(value),
                ("label", _) => filters.labels.push(value),
                ("is", "unread") => filters.unread = true,
                ("is", "starred") => filters.starred = true,
                ("has", "attachment" | "attachments") => filters.attachments = true,
                ("last", "me") => filters.last = Some(Last::Me),
                ("last", "them") => filters.last = Some(Last::Them),
                _ => {}
            }
        }
        // Labels side by side, outside a group, are all of them.
        filters.all_labels = !grouped_labels && filters.labels.len() > 1;
        let Groups { label, from, to } = groups;
        filters.labels.splice(0..0, label);
        filters.from.splice(0..0, from);
        filters.to.splice(0..0, to);
        filters
    }

    /// What the filter asks of the label named `name`, if anything.
    pub fn label(&self, name: &str) -> Option<LabelPick> {
        let among = |list: &[String]| list.iter().any(|l| label_matches(l, name));
        if among(&self.labels) {
            Some(LabelPick::Has)
        } else if among(&self.without) {
            Some(LabelPick::Not)
        } else {
            None
        }
    }

    /// Ask `pick` of the label named `name`, or nothing.
    pub fn pick_label(&mut self, name: &str, pick: Option<LabelPick>) {
        self.labels.retain(|l| !label_matches(l, name));
        self.without.retain(|l| !label_matches(l, name));
        match pick {
            Some(LabelPick::Has) => self.labels.push(name.to_owned()),
            Some(LabelPick::Not) => self.without.push(name.to_owned()),
            None => {}
        }
    }

    /// `query` with these filters in place of the ones it had; its words
    /// and other operators stay as they were.
    pub fn apply(&self, query: &str) -> String {
        let (rest, groups) = take_groups(query);
        let mut kept: Vec<String> = levels(tokens(&rest).unwrap_or_else(|_| vec![rest.clone()]))
            .into_iter()
            .filter(|(token, top)| !top || !manages(token, &groups))
            .map(|(token, _)| token)
            .collect();
        let mut add = |name: &str, value: &Option<String>| {
            if let Some(value) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                kept.push(format!("{name}:{}", quote(value)));
            }
        };
        add("account", &self.account);
        add("newer_than", &self.within);
        any_of(&mut kept, "from", &self.from, false);
        not(&mut kept, "from", &self.not_from);
        any_of(&mut kept, "to", &self.to, false);
        not(&mut kept, "to", &self.not_to);
        match self.archived {
            Archived::Include => {}
            Archived::LeaveOut => kept.push("in:inbox".to_owned()),
            Archived::Only => kept.push("-in:inbox".to_owned()),
        }
        any_of(&mut kept, "label", &self.labels, self.all_labels);
        not(&mut kept, "label", &self.without);
        let last = match self.last {
            Some(Last::Me) => "last:me",
            Some(Last::Them) => "last:them",
            None => "",
        };
        for (on, token) in [
            (self.unread, "is:unread"),
            (self.starred, "is:starred"),
            (self.attachments, "has:attachment"),
            (self.last.is_some(), last),
        ] {
            if on {
                kept.push(token.to_owned());
            }
        }
        kept.join(" ")
    }
}

/// Each token, and whether it stands at the top of the query rather than
/// inside the person's own `{ }` or `( )`, which the panel leaves alone.
fn levels(tokens: Vec<String>) -> Vec<(String, bool)> {
    let mut depth: i32 = 0;
    tokens
        .into_iter()
        .map(|token| {
            let top = depth == 0 && !token.starts_with(['{', '(']);
            for c in token.chars() {
                match c {
                    '{' | '(' => depth += 1,
                    '}' | ')' => depth -= 1,
                    _ => {}
                }
            }
            depth = depth.max(0);
            (token, top)
        })
        .collect()
}

/// Whether `token` is one of the filters the panel sets. Beside a group
/// the panel wrote, a lone operator of its kind is the person's own.
fn manages(token: &str, groups: &Groups) -> bool {
    let Some((negated, name, value)) = operator(token) else {
        return false;
    };
    let value = value.trim_matches('"').to_lowercase();
    if name == "in" {
        return value == "inbox";
    }
    if negated {
        return GROUPED.contains(&name);
    }
    match name {
        "label" | "from" | "to" => !groups.has(name),
        "account" | "newer_than" => true,
        "is" => matches!(value.as_str(), "unread" | "starred"),
        "has" => matches!(value.as_str(), "attachment" | "attachments"),
        "last" => matches!(value.as_str(), "me" | "them"),
        _ => false,
    }
}

/// Write `values` as `name:` terms: one alone, several as a group of any
/// of them (`{from:a from:b}`), or side by side when all are needed.
fn any_of(kept: &mut Vec<String>, name: &str, values: &[String], all: bool) {
    let terms: Vec<String> = values
        .iter()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(|v| format!("{name}:{}", quote(v)))
        .collect();
    if terms.len() > 1 && !all {
        kept.push(format!("{{{}}}", terms.join(" ")));
    } else {
        kept.extend(terms);
    }
}

/// Write `values` as `-name:` terms, each left out.
fn not(kept: &mut Vec<String>, name: &str, values: &[String]) {
    kept.extend(
        values
            .iter()
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(|v| format!("-{name}:{}", quote(v))),
    );
}

/// The query without the groups the panel writes (`{label:a label:b}`,
/// `{from:a from:b}`, `{to:a to:b}`: one of each at most), and what was in
/// them. A group with anything else in it, or a second of a kind, is the
/// person's own and is left as it is.
fn take_groups(query: &str) -> (String, Groups) {
    let mut groups = Groups::default();
    let mut rest = String::new();
    let mut at = 0;
    while let Some(open) = query[at..].find('{').map(|i| at + i) {
        let Some(close) = query[open..].find('}').map(|i| open + i) else {
            break;
        };
        let mut kind: Option<&'static str> = None;
        let mut values = Vec::new();
        for token in tokens(&query[open + 1..close]).unwrap_or_default() {
            let one = match operator(&token) {
                Some((false, name, value)) => GROUPED
                    .into_iter()
                    .find(|g| *g == name)
                    .filter(|g| kind.is_none_or(|k| k == *g))
                    .map(|g| (g, value.trim_matches('"').to_owned())),
                _ => None,
            };
            match one {
                Some((g, value)) => {
                    kind = Some(g);
                    values.push(value);
                }
                None => {
                    kind = None;
                    break;
                }
            }
        }
        rest.push_str(&query[at..open]);
        match kind {
            Some(name) if !groups.has(name) => *groups.slot(name) = values,
            _ => rest.push_str(&query[open..=close]),
        }
        rest.push(' ');
        at = close + 1;
    }
    rest.push_str(&query[at..]);
    (rest.split_whitespace().collect::<Vec<_>>().join(" "), groups)
}

fn quote(value: &str) -> String {
    if value.contains(char::is_whitespace) {
        format!("\"{}\"", value.replace('"', ""))
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archived_mail_is_in_inbox_or_not() {
        let read = |q: &str| Filters::read(q).archived;
        assert_eq!(read("label:clients"), Archived::Include);
        assert_eq!(read("in:inbox category:primary"), Archived::LeaveOut);
        assert_eq!(read("label:clients -in:inbox"), Archived::Only);
        let mut f = Filters::read("in:inbox category:primary");
        f.archived = Archived::Only;
        assert_eq!(f.apply("in:inbox category:primary"), "category:primary -in:inbox");
        f.archived = Archived::Include;
        assert_eq!(f.apply("in:inbox category:primary"), "category:primary");
        // Other folders stay the person's own.
        assert_eq!(Filters::read("in:sent").archived, Archived::Include);
        assert_eq!(Filters::default().apply("in:sent"), "in:sent");
    }

    #[test]
    fn filters_round_trip_through_the_text() {
        let query = r#"cover shift from:"Ada Lovelace" is:unread in:inbox"#;
        let mut filters = Filters::read(query);
        assert_eq!(filters.from, ["Ada Lovelace"]);
        assert!(filters.unread);
        filters.unread = false;
        filters.within = Some("1m".into());
        filters.labels = vec!["Work/Clients".into()];
        assert_eq!(
            filters.apply(query),
            r#"cover shift newer_than:1m from:"Ada Lovelace" in:inbox label:Work/Clients"#
        );
        assert_eq!(Filters::default().apply(query), "cover shift");
    }

    #[test]
    fn labels_are_any_of_and_last_is_kept() {
        let mut filters = Filters::default();
        filters.pick_label("Clients/Acme", Some(LabelPick::Has));
        filters.pick_label("Receipts 2024", Some(LabelPick::Has));
        filters.last = Some(Last::Me);
        filters.attachments = true;
        let query = filters.apply("has:attachment");
        assert_eq!(
            query,
            r#"{label:Clients/Acme label:"Receipts 2024"} has:attachment last:me"#
        );
        let read = Filters::read(&query);
        assert_eq!(read.labels, ["Clients/Acme", "Receipts 2024"]);
        assert!(read.attachments && read.label("clients-acme") == Some(LabelPick::Has));
        assert!(!read.all_labels);
        assert_eq!(read.last, Some(Last::Me));
        filters.pick_label("clients/acme", None);
        assert_eq!(filters.labels, ["Receipts 2024"]);
        // A group of the person's own stays.
        let theirs = "{from:ann label:y} label:x";
        assert_eq!(Filters::default().apply(theirs), "{from:ann label:y}");
    }

    #[test]
    fn senders_and_recipients_any_of_or_not() {
        let query = "invoice {from:ann from:\"Bob Smith\"} -from:noreply to:me@x.org";
        let mut f = Filters::read(query);
        assert_eq!(f.from, ["ann", "Bob Smith"]);
        assert_eq!((f.not_from.as_slice(), f.to.as_slice()), (&["noreply".to_owned()][..], &["me@x.org".to_owned()][..]));
        assert_eq!(f.apply(query), query);
        f.from.retain(|p| p != "ann");
        f.not_to.push("list@x.org".into());
        assert_eq!(
            f.apply(query),
            "invoice from:\"Bob Smith\" -from:noreply to:me@x.org -to:list@x.org"
        );
        // Groups of each kind are read side by side.
        let both = Filters::read("{label:a label:b} {to:c to:d}");
        assert_eq!((both.labels.len(), both.to.len()), (2, 2));
        assert_eq!(Filters::default().apply("{label:a label:b} {to:c to:d} word"), "word");
    }

    #[test]
    fn labels_all_of_and_left_out() {
        let query = "invoice label:Clients label:Receipts -label:Newsletters";
        let mut filters = Filters::read(query);
        assert_eq!(filters.labels, ["Clients", "Receipts"]);
        assert!(filters.all_labels);
        assert_eq!(filters.label("newsletters"), Some(LabelPick::Not));
        assert_eq!(filters.apply(query), query);
        filters.all_labels = false;
        filters.pick_label("Receipts", Some(LabelPick::Not));
        assert_eq!(
            filters.apply(query),
            "invoice label:Clients -label:Newsletters -label:Receipts"
        );
        filters.pick_label("Old", Some(LabelPick::Has));
        assert_eq!(
            filters.apply(query),
            "invoice {label:Clients label:Old} -label:Newsletters -label:Receipts"
        );
        // A label beside a group is the person's own, and stays.
        let theirs = "{label:a label:b} label:c -label:d";
        let read = Filters::read(theirs);
        assert_eq!((read.labels.len(), read.without.len()), (2, 1));
        assert_eq!(Filters::default().apply(theirs), "label:c");
    }
}
