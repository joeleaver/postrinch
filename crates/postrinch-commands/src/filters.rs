//! The search filters a window offers (who from and to, when, unread,
//! starred, with attachments, who wrote last, which accounts, which labels),
//! read from and written into the query text, so the text stays the one
//! place a search lives: what the person types, what an agent sends and
//! what a saved view keeps.

use crate::query::{Last, label_matches, operator, tokens};

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
    pub from: Option<String>,
    pub to: Option<String>,
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
        let (rest, group) = take_label_group(query);
        let grouped = !group.is_empty();
        filters.labels = group;
        for (token, top) in levels(tokens(&rest).unwrap_or_default()) {
            if !top || !manages(&token, grouped) {
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
                filters.without.push(value);
                continue;
            }
            let lower = value.to_lowercase();
            match (name, lower.as_str()) {
                ("account", _) => filters.account = Some(value),
                ("newer_than", _) => filters.within = Some(value),
                ("from", _) => filters.from = Some(value),
                ("to", _) => filters.to = Some(value),
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
        filters.all_labels = !grouped && filters.labels.len() > 1;
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
        let (rest, group) = take_label_group(query);
        let grouped = !group.is_empty();
        let mut kept: Vec<String> = levels(tokens(&rest).unwrap_or_else(|_| vec![rest.clone()]))
            .into_iter()
            .filter(|(token, top)| !top || !manages(token, grouped))
            .map(|(token, _)| token)
            .collect();
        let mut add = |name: &str, value: &Option<String>| {
            if let Some(value) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                kept.push(format!("{name}:{}", quote(value)));
            }
        };
        add("account", &self.account);
        add("newer_than", &self.within);
        add("from", &self.from);
        add("to", &self.to);
        match self.archived {
            Archived::Include => {}
            Archived::LeaveOut => kept.push("in:inbox".to_owned()),
            Archived::Only => kept.push("-in:inbox".to_owned()),
        }
        let labels: Vec<String> = self
            .labels
            .iter()
            .map(|l| format!("label:{}", quote(l)))
            .collect();
        if labels.len() > 1 && !self.all_labels {
            kept.push(format!("{{{}}}", labels.join(" ")));
        } else {
            kept.extend(labels);
        }
        kept.extend(self.without.iter().map(|l| format!("-label:{}", quote(l))));
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
/// of labels (`grouped`), a lone `label:` is the person's own.
fn manages(token: &str, grouped: bool) -> bool {
    let Some((negated, name, value)) = operator(token) else {
        return false;
    };
    let value = value.trim_matches('"').to_lowercase();
    if name == "in" {
        return value == "inbox";
    }
    if negated {
        return name == "label";
    }
    match name {
        "label" => !grouped,
        "account" | "newer_than" | "from" | "to" => true,
        "is" => matches!(value.as_str(), "unread" | "starred"),
        "has" => matches!(value.as_str(), "attachment" | "attachments"),
        "last" => matches!(value.as_str(), "me" | "them"),
        _ => false,
    }
}

/// The query without its group of labels (`{label:a label:b}`, which the
/// panel writes for "any of these"), and the labels in it. A group with
/// anything else in it is the person's own and is left as it is.
fn take_label_group(query: &str) -> (String, Vec<String>) {
    let (Some(open), Some(close)) = (query.find('{'), query.find('}')) else {
        return (query.to_owned(), Vec::new());
    };
    if close < open {
        return (query.to_owned(), Vec::new());
    }
    let inside = &query[open + 1..close];
    let mut labels = Vec::new();
    for token in tokens(inside).unwrap_or_default() {
        match operator(&token) {
            Some((false, "label", value)) => labels.push(value.trim_matches('"').to_owned()),
            _ => return (query.to_owned(), Vec::new()),
        }
    }
    let rest = format!("{} {}", &query[..open], &query[close + 1..]);
    (
        rest.split_whitespace().collect::<Vec<_>>().join(" "),
        labels,
    )
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
        assert_eq!(filters.from.as_deref(), Some("Ada Lovelace"));
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
        let theirs = "{from:ann from:bob} label:x";
        assert_eq!(Filters::default().apply(theirs), "{from:ann from:bob}");
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
