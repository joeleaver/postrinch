//! What `postrinch-mcp` and the app say to each other: one JSON value per
//! line over a loopback TCP connection.
//!
//! The bridge opens with a [`Hello`] carrying the agent's key; the app
//! answers with a [`Greeting`]. Then each [`Request`] gets one
//! [`Response`] with the same id.

use postrinch_commands::{Folder, ThreadAction, ThreadRef};
use postrinch_model::{EmailAddress, ProviderThreadId, SystemLabel};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub key: String,
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
}

impl Call {
    /// Whether the call changes mail.
    pub fn changes(&self) -> bool {
        matches!(self, Call::Act { .. })
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
