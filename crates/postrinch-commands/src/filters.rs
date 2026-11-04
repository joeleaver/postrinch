//! The search filters a window offers (account, when, from, label, and
//! unread, starred or with attachments), read from and written into the
//! query text, so the text stays the one place a search lives: what the
//! person types, what an agent sends and what a saved view keeps.

use crate::query::{operator, tokens};

/// The filters a query sets, as the filter panel shows them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    pub account: Option<String>,
    /// `newer_than:`'s value: `7d`, `1m`, `1y`.
    pub within: Option<String>,
    pub from: Option<String>,
    pub label: Option<String>,
    pub unread: bool,
    pub starred: bool,
    pub attachments: bool,
}

impl Filters {
    /// The filters `query` sets. Text that does not read is no filter.
    pub fn read(query: &str) -> Self {
        let mut filters = Filters::default();
        for token in tokens(query).unwrap_or_default() {
            let Some((false, name, value)) = operator(&token) else {
                continue;
            };
            let value = value.trim_matches('"').to_owned();
            let lower = value.to_lowercase();
            match (name, lower.as_str()) {
                ("account", _) => filters.account = Some(value),
                ("newer_than", _) => filters.within = Some(value),
                ("from", _) => filters.from = Some(value),
                ("label", _) => filters.label = Some(value),
                ("is", "unread") => filters.unread = true,
                ("is", "starred") => filters.starred = true,
                ("has", "attachment" | "attachments") => filters.attachments = true,
                _ => {}
            }
        }
        filters
    }

    /// `query` with these filters in place of the ones it had; its words
    /// and other operators stay as they were.
    pub fn apply(&self, query: &str) -> String {
        let mut kept: Vec<String> = tokens(query)
            .unwrap_or_else(|_| vec![query.to_owned()])
            .into_iter()
            .filter(|token| !self.manages(token))
            .collect();
        let mut add = |name: &str, value: &Option<String>| {
            if let Some(value) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                kept.push(format!("{name}:{}", quote(value)));
            }
        };
        add("account", &self.account);
        add("newer_than", &self.within);
        add("from", &self.from);
        add("label", &self.label);
        for (on, token) in [
            (self.unread, "is:unread"),
            (self.starred, "is:starred"),
            (self.attachments, "has:attachment"),
        ] {
            if on {
                kept.push(token.to_owned());
            }
        }
        kept.join(" ")
    }

    /// Whether `token` is one of the filters the panel sets.
    fn manages(&self, token: &str) -> bool {
        let Some((false, name, value)) = operator(token) else {
            return false;
        };
        let value = value.trim_matches('"').to_lowercase();
        match name {
            "account" | "newer_than" | "from" | "label" => true,
            "is" => matches!(value.as_str(), "unread" | "starred"),
            "has" => matches!(value.as_str(), "attachment" | "attachments"),
            _ => false,
        }
    }
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
    fn filters_round_trip_through_the_text() {
        let query = r#"cover shift from:"Ada Lovelace" is:unread in:inbox"#;
        let mut filters = Filters::read(query);
        assert_eq!(filters.from.as_deref(), Some("Ada Lovelace"));
        assert!(filters.unread);
        filters.unread = false;
        filters.within = Some("1m".into());
        filters.label = Some("Work/Clients".into());
        assert_eq!(
            filters.apply(query),
            r#"cover shift in:inbox newer_than:1m from:"Ada Lovelace" label:Work/Clients"#
        );
        assert_eq!(Filters::default().apply(query), "cover shift in:inbox");
    }
}
