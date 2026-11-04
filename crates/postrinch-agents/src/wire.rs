//! What `postrinch-mcp` and the app say to each other: one JSON value per
//! line over a loopback TCP connection.
//!
//! The bridge opens with a [`Hello`] carrying the agent's key; the app
//! answers with a [`Greeting`]. Then each [`Request`] gets one
//! [`Response`] with the same id.

use postrinch_commands::{Folder, ThreadAction, ThreadRef};
use postrinch_model::{EmailAddress, Mailbox, ProviderThreadId, SystemLabel};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where the app listens, unless [`ADDR_ENV`] says otherwise.
pub const DEFAULT_ADDR: &str = "127.0.0.1:7465";
/// Overrides [`DEFAULT_ADDR`], for the app and the bridge alike.
pub const ADDR_ENV: &str = "POSTRINCH_APP_ADDR";
/// The agent's key, in the bridge's environment (never its command line).
pub const KEY_ENV: &str = "POSTRINCH_AGENT_KEY";
/// The longest line either side reads: a guard, not a limit anyone meets.
pub const MAX_LINE: u64 = 64 * 1024 * 1024;

/// The address in use.
pub fn addr() -> String {
    std::env::var(ADDR_ENV).unwrap_or_else(|_| DEFAULT_ADDR.to_owned())
}

/// This build's version. The bridge and the app must be the same one: the
/// wire format changes with them, so they are installed and updated together.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub key: String,
    /// The bridge's [`VERSION`]; empty from a bridge older than this field.
    #[serde(default)]
    pub version: String,
}

impl Hello {
    pub fn new(key: String) -> Self {
        Self {
            key,
            version: VERSION.to_owned(),
        }
    }

    /// Why the app turns this bridge away, if it is not the app's version.
    pub fn mismatch(&self) -> Option<String> {
        (self.version != VERSION).then(|| {
            let theirs = if self.version.is_empty() {
                "an older version".to_owned()
            } else {
                self.version.clone()
            };
            format!(
                "This postrinch-mcp is {theirs} and Postrinch is {VERSION}; they must match. \
                 Restart the app that started it, so it starts the one installed with Postrinch."
            )
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Greeting {
    Welcome { agent: String },
    Refused { reason: String },
}

/// One command, as an agent asks for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "snake_case")]
pub enum Call {
    Accounts,
    Folders,
    ListThreads {
        folder: Folder,
        /// With an [`Folder::Everywhere`] folder: that folder of this
        /// account alone.
        #[serde(default)]
        account: Option<EmailAddress>,
        category: Option<SystemLabel>,
        offset: usize,
        limit: usize,
    },
    ReadThread {
        account: EmailAddress,
        thread: ProviderThreadId,
    },
    Search {
        query: String,
        limit: usize,
    },
    LabelChoices {
        account: EmailAddress,
    },
    Act {
        threads: Vec<ThreadRef>,
        action: ThreadAction,
    },
    /// A message for the person to check and send: a reply to `thread`'s
    /// latest message (to everyone on it, unless `to` says otherwise), or
    /// new mail.
    Propose {
        account: EmailAddress,
        thread: Option<ProviderThreadId>,
        to: Option<Vec<Mailbox>>,
        cc: Option<Vec<Mailbox>>,
        bcc: Option<Vec<Mailbox>>,
        subject: Option<String>,
        body: String,
        reason: Option<String>,
    },
}

impl Call {
    /// Whether the call changes mail.
    pub fn changes(&self) -> bool {
        matches!(self, Call::Act { .. } | Call::Propose { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub call: Call,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(flatten)]
    pub outcome: Outcome,
}

/// A call's answer: its result, or one sentence saying why not.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok(Value),
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_bridge_of_the_same_version_is_let_in() {
        assert_eq!(Hello::new("k".into()).mismatch(), None);
        let old: Hello = serde_json::from_str(r#"{"key":"k"}"#).unwrap();
        assert!(old.mismatch().unwrap().contains("an older version"));
        let other = Hello {
            key: "k".into(),
            version: "0.0.1".into(),
        };
        assert!(other.mismatch().unwrap().contains("0.0.1"));
    }
}
