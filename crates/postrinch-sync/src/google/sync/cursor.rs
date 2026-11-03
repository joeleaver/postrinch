use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{SyncError, SyncResult};

/// Where Gmail sync has got to, kept as the account's sync cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Cursor {
    /// A full sync is under way (or was interrupted). Once it has listed
    /// and stored everything, history from `history_id` (taken before the
    /// listing began) catches what changed while it ran.
    FullSince { history_id: String },
    /// Everything up to `history_id` is stored.
    History { history_id: String },
}

impl Cursor {
    pub(super) fn parse(raw: &str) -> SyncResult<Self> {
        serde_json::from_str(raw)
            .map_err(|e| SyncError::Config(format!("unreadable sync cursor {raw:?}: {e}")))
    }
}

impl fmt::Display for Cursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&serde_json::to_string(self).expect("a cursor serializes"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for c in [
            Cursor::FullSince {
                history_id: "1".into(),
            },
            Cursor::History {
                history_id: "2".into(),
            },
        ] {
            assert_eq!(Cursor::parse(&c.to_string()).unwrap(), c);
        }
        assert!(Cursor::parse("12345").is_err());
    }
}
