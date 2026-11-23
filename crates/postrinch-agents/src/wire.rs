//! What `postrinch-mcp` and the app say to each other: one JSON value per
//! line over a loopback TCP connection.
//!
//! The bridge opens with a [`Hello`] carrying the agent's key; the app
//! answers with a [`Greeting`]. Then each [`Request`] gets one
//! [`Response`] with the same id.

use postrinch_commands::{Folder, MessageRef, ThreadAction, ThreadRef};
use postrinch_model::{EmailAddress, Mailbox, ProviderMessageId, ProviderThreadId, SystemLabel};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::rule_shape::RuleShape;
use crate::view_shape::ViewShape;

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
    /// Do `actions`, in order, to `threads`, or to every thread `search`
    /// (the search language) matches.
    Act {
        #[serde(default)]
        threads: Vec<ThreadRef>,
        #[serde(default)]
        search: Option<String>,
        /// Or messages, each changed alone, as one message at a time does.
        #[serde(default)]
        messages: Vec<MessageRef>,
        actions: Vec<ThreadAction>,
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
        /// Files to attach.
        #[serde(default)]
        attachments: Vec<ProposedFile>,
    },
    /// A message's attachment, by its place in the message (as read_thread
    /// numbers them), its bytes in base64.
    Attachment {
        account: EmailAddress,
        message: ProviderMessageId,
        attachment: usize,
    },
    /// The account's rules, in the order they run.
    Rules {
        account: EmailAddress,
    },
    /// Make a rule, or change the one with `rule_id`. One that forwards mail or
    /// asks an agent is proposed to the person instead.
    SaveRule {
        account: EmailAddress,
        /// The rule's id (`id` names the request).
        rule_id: Option<u32>,
        rule: RuleShape,
        /// Also do it now to the matching mail already here.
        #[serde(default)]
        apply_to_existing: bool,
        reason: Option<String>,
    },
    DeleteRule {
        account: EmailAddress,
        rule_id: u32,
    },
    SetRuleOn {
        account: EmailAddress,
        rule_id: u32,
        on: bool,
    },
    /// Move a rule to place `to` (0 first) in the order rules run.
    MoveRule {
        account: EmailAddress,
        rule_id: u32,
        to: usize,
    },
    /// Run a saved rule now over the mail already here: the last three
    /// months', or with `all` every message.
    RunRule {
        account: EmailAddress,
        rule_id: u32,
        #[serde(default)]
        all: bool,
    },
    /// Make a label in Gmail at once (one already named so is answered).
    CreateLabel {
        account: EmailAddress,
        name: String,
    },
    /// Rename one of the person's labels, named by id or name.
    RenameLabel {
        account: EmailAddress,
        label: String,
        name: String,
    },
    DeleteLabel {
        account: EmailAddress,
        label: String,
    },
    /// Every view, in the sidebar's order, each with its count.
    Views,
    /// A page of what a view shows, named by key or name.
    ListView {
        view: String,
        offset: usize,
        limit: usize,
    },
    /// Make a view, or change the one `view` names: only what `change`
    /// gives.
    SaveView {
        view: Option<String>,
        change: ViewShape,
    },
    /// Delete one of the person's views, or put a built-in one back.
    DeleteView {
        view: String,
    },
    /// Each account's calendars.
    Calendars,
    /// Who one of the account's own calendars is shared with, and how.
    CalendarShares {
        account: EmailAddress,
        calendar: String,
    },
    /// Add a calendar shared with the account (as Google's mail names it)
    /// to its calendars.
    AddCalendar {
        account: EmailAddress,
        calendar: String,
    },
    /// The events between two moments (RFC 3339, or a date for its
    /// midnight in the person's zone), each repeating one at each time.
    Events {
        #[serde(default)]
        accounts: Vec<EmailAddress>,
        from: String,
        to: String,
    },
    /// When the person is free between two moments.
    FreeTime {
        #[serde(default)]
        accounts: Vec<EmailAddress>,
        from: String,
        to: String,
        /// The shortest stretch worth giving, in minutes.
        minutes: i64,
        /// Each day's working hours, `HH:MM`, in the person's zone.
        #[serde(default)]
        day_starts: Option<String>,
        #[serde(default)]
        day_ends: Option<String>,
        /// The calendars whose events count, by id; when empty, each
        /// account's own and the others it may change and shows.
        #[serde(default)]
        calendars: Vec<String>,
    },
    /// Make an event, or change the one `event` names (only what is given).
    /// One that would invite or tell guests waits for the person.
    SaveEvent {
        account: EmailAddress,
        #[serde(default)]
        calendar: Option<String>,
        #[serde(default)]
        event: Option<String>,
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        starts: Option<String>,
        #[serde(default)]
        ends: Option<String>,
        #[serde(default)]
        first_day: Option<String>,
        #[serde(default)]
        last_day: Option<String>,
        #[serde(default)]
        location: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        guests: Option<Vec<Mailbox>>,
        reason: Option<String>,
    },
    /// Delete an event (one time of a repeating one, when it names that).
    /// One whose guests would be told waits for the person.
    DeleteEvent {
        account: EmailAddress,
        calendar: String,
        event: String,
        reason: Option<String>,
    },
    /// Answer an event the account is invited to: kept for the person.
    AnswerEvent {
        account: EmailAddress,
        calendar: String,
        event: String,
        answer: String,
        reason: Option<String>,
    },
    /// Answer the invitation in a message: kept for the person to give.
    AnswerInvitation {
        account: EmailAddress,
        message: ProviderMessageId,
        /// `yes`, `maybe` or `no`.
        answer: String,
        reason: Option<String>,
    },
}

/// A file an agent attaches to a proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum ProposedFile {
    /// A file on this computer, read by the agent's own server (whose
    /// permissions are the agent's), sent with the path it came from so
    /// the person sees it.
    File {
        path: String,
        name: String,
        /// The bytes, base64.
        data: String,
    },
    /// An attachment of a message in an account this agent may see.
    Mail {
        account: EmailAddress,
        message: ProviderMessageId,
        attachment: usize,
    },
}

/// What a call changed, for the window to show and sync to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Changed {
    Mail,
    Rules(EmailAddress),
    Views,
    /// An answer to an invitation waits for the person.
    Answers,
    /// A calendar changed, or a change to one waits for the person.
    Calendar,
}

impl Call {
    /// What the call changes, if anything.
    pub fn changes(&self) -> Option<Changed> {
        match self {
            Call::Act { .. }
            | Call::Propose { .. }
            | Call::CreateLabel { .. }
            | Call::RenameLabel { .. }
            | Call::DeleteLabel { .. }
            | Call::RunRule { .. } => Some(Changed::Mail),
            Call::SaveRule { account, .. }
            | Call::DeleteRule { account, .. }
            | Call::SetRuleOn { account, .. }
            | Call::MoveRule { account, .. } => Some(Changed::Rules(account.clone())),
            Call::SaveView { .. } | Call::DeleteView { .. } => Some(Changed::Views),
            Call::AnswerInvitation { .. } => Some(Changed::Answers),
            Call::SaveEvent { .. }
            | Call::DeleteEvent { .. }
            | Call::AnswerEvent { .. }
            | Call::AddCalendar { .. } => Some(Changed::Calendar),
            _ => None,
        }
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
