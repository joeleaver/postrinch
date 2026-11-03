//! Changing mail: read, starred, archived, trashed, labelled.
//!
//! An action names threads, as the window and an agent see mail; it is
//! turned into a change to the messages it concerns, which the store
//! applies at once and keeps for the provider (see `postrinch-store`'s
//! outbox). None of these needs the person's confirmation: each can be
//! undone, and none leaves the account.

use postrinch_model::{
    EmailAddress, LabelKind, Message, MessageChange, ProviderMessageId, ProviderThreadId,
    SystemLabel,
};
use serde::{Deserialize, Serialize};

use crate::{CommandError, CommandResult, Commands, app_of};

/// Something to do to threads.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "action", content = "label", rename_all = "snake_case")]
pub enum ThreadAction {
    MarkRead,
    /// The latest message becomes unread, as when you would come back to
    /// the thread.
    MarkUnread,
    /// Star the latest message.
    Star,
    /// Unstar every message.
    Unstar,
    /// Out of the inbox; every label it has stays.
    Archive,
    MoveToInbox,
    Trash,
    /// Out of the trash, back wherever its labels put it.
    Untrash,
    /// Add a label, by the provider's id.
    AddLabel(String),
    RemoveLabel(String),
}

/// A thread, as an action names it.
pub type ThreadRef = (EmailAddress, ProviderThreadId);

/// A label the person can put on mail, as the label menu offers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelChoice {
    /// The provider's id, for [`ThreadAction::AddLabel`].
    pub label: String,
    /// The person's name for it, or the provider's whole path.
    pub name: String,
    pub color: Option<String>,
}

impl Commands {
    /// Do `action` to each thread. Threads the store does not have are
    /// passed over. Only messages the action would change are changed, so
    /// marking a read thread read sends nothing.
    pub fn act(&self, threads: &[ThreadRef], action: &ThreadAction) -> CommandResult<()> {
        for (account, thread) in threads {
            let messages = self.store.thread_messages(account, thread)?;
            for (ids, change) in self.changes(account, &messages, action)? {
                self.store.change_messages(account, &ids, &change)?;
            }
        }
        Ok(())
    }

    /// The labels the person made in an account, by name: what the label
    /// menu offers. Other apps' labels are left out, as the sidebar sets
    /// them apart.
    pub fn label_choices(&self, account: &EmailAddress) -> CommandResult<Vec<LabelChoice>> {
        let prefs = self.store.label_prefs(account)?;
        let mut choices: Vec<LabelChoice> = self
            .store
            .labels(account)?
            .into_iter()
            .filter(|l| l.kind == LabelKind::User && app_of(&l.name).is_none())
            .map(|l| {
                let prefs = prefs.get(&l.provider_id).cloned().unwrap_or_default();
                LabelChoice {
                    name: prefs.name.unwrap_or_else(|| l.name.clone()),
                    color: prefs
                        .color
                        .or_else(|| l.color.as_ref().map(|c| c.background.clone())),
                    label: l.provider_id,
                }
            })
            .collect();
        choices.sort_by_key(|c| c.name.to_lowercase());
        Ok(choices)
    }

    /// The changes `action` makes to a thread's messages (oldest first).
    fn changes(
        &self,
        account: &EmailAddress,
        messages: &[Message],
        action: &ThreadAction,
    ) -> CommandResult<Vec<(Vec<ProviderMessageId>, MessageChange)>> {
        let ids = |keep: &dyn Fn(&Message) -> bool| -> Vec<ProviderMessageId> {
            messages
                .iter()
                .filter(|m| keep(m))
                .map(|m| m.id.clone())
                .collect()
        };
        let latest: Vec<ProviderMessageId> =
            messages.last().map(|m| m.id.clone()).into_iter().collect();
        let has = |label: String| move |m: &Message| m.labels.contains(&label);
        let lacks = |label: String| move |m: &Message| !m.labels.contains(&label);
        let (ids, change) = match action {
            ThreadAction::MarkRead => (ids(&|m| m.flags.unread), MessageChange::read()),
            ThreadAction::MarkUnread => (latest, MessageChange::unread()),
            ThreadAction::Star => (latest, MessageChange::starred(true)),
            ThreadAction::Unstar => (ids(&|m| m.flags.starred), MessageChange::starred(false)),
            ThreadAction::Archive => {
                let inbox = self.system_label(account, SystemLabel::Inbox)?;
                (ids(&has(inbox.clone())), MessageChange::remove_label(inbox))
            }
            ThreadAction::MoveToInbox => {
                let inbox = self.system_label(account, SystemLabel::Inbox)?;
                (ids(&lacks(inbox.clone())), MessageChange::add_label(inbox))
            }
            ThreadAction::Trash => {
                let trash = self.system_label(account, SystemLabel::Trash)?;
                (ids(&lacks(trash.clone())), MessageChange::add_label(trash))
            }
            ThreadAction::Untrash => {
                let trash = self.system_label(account, SystemLabel::Trash)?;
                (ids(&has(trash.clone())), MessageChange::remove_label(trash))
            }
            ThreadAction::AddLabel(label) => (
                ids(&lacks(label.clone())),
                MessageChange::add_label(label.clone()),
            ),
            ThreadAction::RemoveLabel(label) => (
                ids(&has(label.clone())),
                MessageChange::remove_label(label.clone()),
            ),
        };
        Ok(if ids.is_empty() {
            Vec::new()
        } else {
            vec![(ids, change)]
        })
    }

    /// The provider's id for one of the account's system labels.
    fn system_label(&self, account: &EmailAddress, system: SystemLabel) -> CommandResult<String> {
        self.store
            .labels(account)?
            .into_iter()
            .find(|l| l.kind == LabelKind::System(system))
            .map(|l| l.provider_id)
            .ok_or(CommandError::NoSystemLabel {
                account: account.clone(),
                label: system,
            })
    }
}
