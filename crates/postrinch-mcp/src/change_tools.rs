//! The tools that change mail and labels: change_threads (named threads,
//! or every thread a search matches), and making, renaming and deleting
//! labels.

use postrinch_agents::Call;
use postrinch_commands::ThreadAction;
use postrinch_model::{ProviderMessageId, ProviderThreadId};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::{AccountArgs, Postrinch, ThreadArgs, address};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChangeArgs {
    /// The threads to change, as list_threads and search give them. Give
    /// these, `search` or `messages`.
    #[serde(default)]
    pub threads: Vec<ThreadArgs>,
    /// Change every thread this search matches here, however many: the
    /// same language as the search tool (`category:updates in:inbox
    /// older_than:7d`, `from:a OR from:b`). Run search first to see what it
    /// matches.
    #[serde(default)]
    pub search: Option<String>,
    /// Or single messages, `{"account", "message"}` with ids as read_thread
    /// gives them, each changed alone: archiving one leaves the rest of its
    /// conversation in the inbox, as a view showing one message at a time
    /// does.
    #[serde(default)]
    pub messages: Vec<MessageArgs>,
    /// What to do, in order: `mark_read`, `mark_unread`, `star`, `unstar`,
    /// `archive`, `move_to_inbox`, `trash`, `untrash`, `add_label:<name or
    /// id>`, `remove_label:<name or id>`. For example `["mark_read",
    /// "archive"]` or `["add_label:Personal"]`.
    pub actions: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MessageArgs {
    /// The account's address.
    pub account: String,
    /// The message's id, as read_thread gives it.
    pub message: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateLabelArgs {
    /// The account's address.
    pub account: String,
    /// The label's name; `Parent/Child` puts it under another label.
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RenameLabelArgs {
    /// The account's address.
    pub account: String,
    /// The label, by name or id as list_labels gives it.
    pub label: String,
    /// Its new name.
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LabelArgs {
    /// The account's address.
    pub account: String,
    /// The label, by name or id as list_labels gives it.
    pub label: String,
}

#[tool_router(router = change_router, vis = "pub(crate)")]
impl Postrinch {
    /// The labels the person made in an account, with their ids: what
    /// change_threads can add and remove.
    #[tool(name = "list_labels")]
    async fn list_labels(
        &self,
        Parameters(args): Parameters<AccountArgs>,
    ) -> Result<String, String> {
        self.call(Call::LabelChoices {
            account: address(&args.account)?,
        })
        .await
    }

    /// Change threads: named ones, or every thread a search matches (as
    /// selecting all of a search and acting does in Postrinch). Several
    /// actions can be done at once. Each can be undone with its opposite.
    /// Unread and star apply to a thread's latest message, as in Gmail.
    #[tool(name = "change_threads")]
    async fn change_threads(
        &self,
        Parameters(args): Parameters<ChangeArgs>,
    ) -> Result<String, String> {
        let actions = args
            .actions
            .iter()
            .map(|a| action(a))
            .collect::<Result<Vec<_>, String>>()?;
        let threads = args
            .threads
            .iter()
            .map(|t| Ok((address(&t.account)?, ProviderThreadId(t.thread.clone()))))
            .collect::<Result<Vec<_>, String>>()?;
        let messages = args
            .messages
            .iter()
            .map(|m| Ok((address(&m.account)?, ProviderMessageId(m.message.clone()))))
            .collect::<Result<Vec<_>, String>>()?;
        let search = args.search.filter(|s| !s.trim().is_empty());
        self.call(Call::Act {
            threads,
            search,
            messages,
            actions,
        })
        .await
    }

    /// Make a label in an account, in Gmail at once, so change_threads and
    /// save_rule can use it next. Asking for one that is already there
    /// answers with it.
    #[tool(name = "create_label")]
    async fn create_label(
        &self,
        Parameters(args): Parameters<CreateLabelArgs>,
    ) -> Result<String, String> {
        self.call(Call::CreateLabel {
            account: address(&args.account)?,
            name: args.name,
        })
        .await
    }

    /// Rename one of the person's labels; its mail keeps it.
    #[tool(name = "rename_label")]
    async fn rename_label(
        &self,
        Parameters(args): Parameters<RenameLabelArgs>,
    ) -> Result<String, String> {
        self.call(Call::RenameLabel {
            account: address(&args.account)?,
            label: args.label,
            name: args.name,
        })
        .await
    }

    /// Delete one of the person's labels. Its mail stays, without the
    /// label; this cannot be undone. A label a rule uses is not deleted.
    #[tool(name = "delete_label")]
    async fn delete_label(
        &self,
        Parameters(args): Parameters<LabelArgs>,
    ) -> Result<String, String> {
        self.call(Call::DeleteLabel {
            account: address(&args.account)?,
            label: args.label,
        })
        .await
    }
}

fn action(text: &str) -> Result<ThreadAction, String> {
    let text = text.trim();
    let (name, label) = match text.split_once(':') {
        Some((name, label)) => (name.trim(), Some(label.trim())),
        None => (text, None),
    };
    let label = || {
        label
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("{name} needs a label: `{name}:<name or id>`."))
    };
    Ok(match name {
        "mark_read" => ThreadAction::MarkRead,
        "mark_unread" => ThreadAction::MarkUnread,
        "star" => ThreadAction::Star,
        "unstar" => ThreadAction::Unstar,
        "archive" => ThreadAction::Archive,
        "move_to_inbox" => ThreadAction::MoveToInbox,
        "trash" => ThreadAction::Trash,
        "untrash" => ThreadAction::Untrash,
        "add_label" => ThreadAction::AddLabel(label()?),
        "remove_label" => ThreadAction::RemoveLabel(label()?),
        other => return Err(format!("There is no action {other:?}.")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_read_with_their_labels() {
        assert_eq!(action("archive").unwrap(), ThreadAction::Archive);
        assert_eq!(
            action("add_label: Work/Clients").unwrap(),
            ThreadAction::AddLabel("Work/Clients".into())
        );
        assert!(action("add_label").unwrap_err().contains("needs a label"));
        assert!(action("snooze").is_err());
    }
}
