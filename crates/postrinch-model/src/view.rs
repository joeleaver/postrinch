use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ViewError {
    #[error("a view needs a name")]
    EmptyName,
    #[error("a view needs a query")]
    EmptyQuery,
}

/// A saved view: a named query over all accounts' mail, such as
/// "everything from my bank" or "unanswered threads older than a week".
///
/// The query is in the store's query language; the store checks that it
/// parses when the view is saved. The window and an agent create and read
/// views the same way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedView {
    name: String,
    query: String,
}

impl SavedView {
    pub fn new(name: &str, query: &str) -> Result<Self, ViewError> {
        let name = name.trim();
        let query = query.trim();
        if name.is_empty() {
            return Err(ViewError::EmptyName);
        }
        if query.is_empty() {
            return Err(ViewError::EmptyQuery);
        }
        Ok(Self {
            name: name.to_owned(),
            query: query.to_owned(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn query(&self) -> &str {
        &self.query
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_requires_both_parts() {
        let v = SavedView::new("  Bank ", " Message.filter(.from == \"x\") ").unwrap();
        assert_eq!(v.name(), "Bank");
        assert_eq!(v.query(), "Message.filter(.from == \"x\")");
        assert_eq!(SavedView::new(" ", "q"), Err(ViewError::EmptyName));
        assert_eq!(SavedView::new("n", ""), Err(ViewError::EmptyQuery));
    }
}
