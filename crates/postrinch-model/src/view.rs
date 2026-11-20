use serde::{Deserialize, Serialize};

use crate::{EmailAddress, Timestamp};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ViewError {
    #[error("A view needs a name.")]
    EmptyName,
    #[error("A view needs a search.")]
    EmptyQuery,
}

/// A view: a named search over the mail, shown in the sidebar in place of
/// labels, such as "Clients" or "Waiting on others".
///
/// The query is in the search box's language (`label:`, `in:`, `OR`,
/// `last:me`…); the commands check that it reads when the view is saved.
/// The window and an agent make and read views the same way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedView {
    /// What names the view for good, whatever it is called: a built-in
    /// view's own (`"primary"`), or one made when the view was.
    pub key: String,
    pub name: String,
    pub query: String,
    /// The accounts it shows; empty for every account.
    #[serde(default)]
    pub accounts: Vec<EmailAddress>,
    #[serde(default)]
    pub count: ViewCount,
    /// Leave it out of the sidebar while nothing is in it.
    #[serde(default)]
    pub hide_when_empty: bool,
    /// Left out of the sidebar by the person.
    #[serde(default)]
    pub hidden: bool,
    /// Its list's order: remembered for the view, saved or not.
    #[serde(default)]
    pub order: ListOrder,
}

/// What the sidebar counts beside a view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewCount {
    /// Conversations with unread mail.
    #[default]
    Unread,
    /// Every conversation in it.
    All,
    /// Nothing.
    None,
}

impl SavedView {
    /// A view made by the person, keyed by `made` (when it was made).
    pub fn new(name: &str, query: &str, made: Timestamp) -> Result<Self, ViewError> {
        let mut view = Self {
            key: format!("v{}", made.as_millis()),
            name: String::new(),
            query: String::new(),
            accounts: Vec::new(),
            count: ViewCount::default(),
            hide_when_empty: false,
            hidden: false,
            order: ListOrder::default(),
        };
        view.rename(name)?;
        view.set_query(query)?;
        Ok(view)
    }

    pub fn rename(&mut self, name: &str) -> Result<(), ViewError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ViewError::EmptyName);
        }
        name.clone_into(&mut self.name);
        Ok(())
    }

    pub fn set_query(&mut self, query: &str) -> Result<(), ViewError> {
        let query = query.trim();
        if query.is_empty() {
            return Err(ViewError::EmptyQuery);
        }
        query.clone_into(&mut self.query);
        Ok(())
    }
}

/// The order a list is in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListOrder {
    /// Latest activity first.
    #[default]
    NewestFirst,
    OldestFirst,
    /// Those with unread mail first, each part newest first.
    UnreadFirst,
}

impl ListOrder {
    /// Order two rows: whether each is unread and when, then ties broken
    /// by `tie` so a list keeps one order from one read to the next.
    pub fn compare(
        self,
        a: (bool, Timestamp),
        b: (bool, Timestamp),
        tie: std::cmp::Ordering,
    ) -> std::cmp::Ordering {
        let unread = match self {
            ListOrder::UnreadFirst => b.0.cmp(&a.0),
            _ => std::cmp::Ordering::Equal,
        };
        let when = match self {
            ListOrder::OldestFirst => a.1.cmp(&b.1),
            _ => b.1.cmp(&a.1),
        };
        unread.then(when).then(tie)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_requires_both_parts() {
        let made = Timestamp::from_millis(1_700_000_000_000);
        let v = SavedView::new("  Bank ", " from:bank ", made).unwrap();
        assert_eq!((v.name.as_str(), v.query.as_str()), ("Bank", "from:bank"));
        assert_eq!(v.key, "v1700000000000");
        assert_eq!(SavedView::new(" ", "q", made), Err(ViewError::EmptyName));
        assert_eq!(SavedView::new("n", "", made), Err(ViewError::EmptyQuery));
    }

    #[test]
    fn reads_with_only_what_every_view_has() {
        let v: SavedView =
            serde_json::from_str(r#"{"key":"k","name":"N","query":"in:inbox"}"#).unwrap();
        assert_eq!(v.count, ViewCount::Unread);
        assert_eq!(v.order, ListOrder::NewestFirst);
        assert!(v.accounts.is_empty() && !v.hidden && !v.hide_when_empty);
    }
}
