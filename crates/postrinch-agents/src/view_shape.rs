//! A view as agents write and read it: its settings in plain words, as
//! the view panel offers them, rather than Postrinch's own types. Only
//! the settings given are changed.

use postrinch_commands::{ViewEntry, ViewTally, followed};
use postrinch_model::{EmailAddress, ListOrder, SavedView, ViewCount, ViewRows};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ViewShape {
    /// What the sidebar calls it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Which mail it shows, in the search language: `in:inbox
    /// -category:promotions`, `{from:a from:b} -from:noreply`,
    /// `label:clients is:unread`. Empty for all mail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    /// The accounts whose mail it shows, by address; empty for every
    /// account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accounts: Option<Vec<String>>,
    /// What the sidebar counts beside it: `unread` (conversations with
    /// unread mail), `all` or `none`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<String>,
    /// Whether the sidebar lists it: `always`, `when_it_has_mail` or
    /// `hidden`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar: Option<String>,
    /// Its list's order: `newest_first`, `oldest_first` or `unread_first`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<String>,
    /// What a row of its list is: `conversations`, `messages` (one
    /// message at a time), or `window` to follow Settings › Reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<String>,
}

impl ViewShape {
    /// Change `view` as this asks.
    pub fn apply(&self, view: &mut SavedView) -> Result<(), String> {
        if let Some(name) = &self.name {
            view.name = name.trim().to_owned();
        }
        if let Some(search) = &self.search {
            view.query = search.trim().to_owned();
        }
        if let Some(accounts) = &self.accounts {
            view.accounts = accounts
                .iter()
                .map(|a| {
                    EmailAddress::parse(a.trim()).map_err(|_| format!("{a:?} is not an address."))
                })
                .collect::<Result<_, _>>()?;
        }
        if let Some(count) = &self.count {
            view.count = word(
                "count",
                count,
                &[
                    ("unread", ViewCount::Unread),
                    ("all", ViewCount::All),
                    ("none", ViewCount::None),
                ],
            )?;
        }
        if let Some(sidebar) = &self.sidebar {
            (view.hidden, view.hide_when_empty) = word(
                "sidebar",
                sidebar,
                &[
                    ("always", (false, false)),
                    ("when_it_has_mail", (false, true)),
                    ("hidden", (true, false)),
                ],
            )?;
        }
        if let Some(order) = &self.order {
            view.order = word(
                "order",
                order,
                &[
                    ("newest_first", ListOrder::NewestFirst),
                    ("oldest_first", ListOrder::OldestFirst),
                    ("unread_first", ListOrder::UnreadFirst),
                ],
            )?;
        }
        if let Some(rows) = &self.rows {
            view.rows = word(
                "rows",
                rows,
                &[
                    ("conversations", Some(ViewRows::Conversations)),
                    ("messages", Some(ViewRows::Messages)),
                    ("window", None),
                ],
            )?;
        }
        Ok(())
    }
}

/// The value of `name` that `text` says, from `words`.
fn word<T: Copy>(name: &str, text: &str, words: &[(&str, T)]) -> Result<T, String> {
    let text = text.trim().to_lowercase();
    words
        .iter()
        .find(|(w, _)| *w == text)
        .map(|(_, v)| *v)
        .ok_or_else(|| {
            let all: Vec<&str> = words.iter().map(|(w, _)| *w).collect();
            format!("{name} is one of {}, not {text:?}.", all.join(", "))
        })
}

/// A view as list_views shows it: its settings in the words save_view
/// takes, what it is, whether this agent may change it, and how many
/// conversations it shows.
pub fn read(entry: &ViewEntry, tally: Option<&ViewTally>, may_change: bool) -> Value {
    let v = &entry.view;
    let kind = match (&entry.original, followed(&v.key)) {
        (None, _) => "yours",
        (Some(_), Some("inbox")) => "account_inbox",
        (Some(_), Some(_)) => "account_category",
        (Some(_), None) => "built_in",
    };
    let sidebar = match (v.hidden, v.hide_when_empty) {
        (true, _) => "hidden",
        (false, true) => "when_it_has_mail",
        (false, false) => "always",
    };
    let mut shown = json!({
        "view": v.key,
        "name": v.name,
        "search": v.query,
        "accounts": v.accounts,
        "count": match v.count {
            ViewCount::Unread => "unread",
            ViewCount::All => "all",
            ViewCount::None => "none",
        },
        "sidebar": sidebar,
        "order": match v.order {
            ListOrder::NewestFirst => "newest_first",
            ListOrder::OldestFirst => "oldest_first",
            ListOrder::UnreadFirst => "unread_first",
        },
        "rows": match v.rows {
            Some(ViewRows::Conversations) => "conversations",
            Some(ViewRows::Messages) => "messages",
            None => "window",
        },
        "kind": kind,
        "may_change": may_change,
    });
    if entry.original.as_ref().is_some_and(|o| o != v) {
        shown["changed_from_built_in"] = json!(true);
    }
    if let Some(t) = tally {
        shown["conversations"] = json!(t.all);
        shown["unread"] = json!(t.unread);
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;
    use postrinch_model::Timestamp;

    #[test]
    fn changes_only_what_it_names() {
        let mut view = SavedView::new("Bank", "from:bank", Timestamp::from_millis(1)).unwrap();
        let shape = ViewShape {
            sidebar: Some("when_it_has_mail".into()),
            rows: Some("messages".into()),
            ..Default::default()
        };
        shape.apply(&mut view).unwrap();
        assert_eq!(
            (view.name.as_str(), view.query.as_str()),
            ("Bank", "from:bank")
        );
        assert!(view.hide_when_empty && !view.hidden);
        assert_eq!(view.rows, Some(ViewRows::Messages));

        let wrong = ViewShape {
            order: Some("sideways".into()),
            ..Default::default()
        };
        let why = wrong.apply(&mut view).unwrap_err();
        assert!(why.contains("newest_first") && why.contains("sideways"));
    }
}
