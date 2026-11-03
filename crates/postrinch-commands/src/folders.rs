//! Folders: what the sidebar lists and what a thread list shows.

use postrinch_model::{Account, EmailAddress, Label, LabelKind, SystemLabel};
use serde::{Deserialize, Serialize};

use crate::{CommandError, CommandResult, Commands};

/// Somewhere threads are listed from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Folder {
    /// One system label across every account: all inboxes, or the unified
    /// Sent.
    Everywhere(SystemLabel),
    /// One label of one account, by the provider's id for it.
    Label {
        account: EmailAddress,
        label: String,
    },
}

/// A folder as the sidebar shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderEntry {
    pub folder: Folder,
    pub name: String,
    pub unread: usize,
}

/// One account in the sidebar: its inbox and the labels the person made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountFolders {
    pub account: Account,
    pub inbox: FolderEntry,
    /// The person's own labels, by name.
    pub labels: Vec<FolderEntry>,
}

/// Everything the sidebar lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidebar {
    /// All inboxes, then the unified Sent and Drafts.
    pub everywhere: Vec<FolderEntry>,
    pub accounts: Vec<AccountFolders>,
}

/// The system labels the sidebar unifies across accounts, in its order.
const UNIFIED: [(SystemLabel, &str); 3] = [
    (SystemLabel::Inbox, "All inboxes"),
    (SystemLabel::Sent, "Sent"),
    (SystemLabel::Drafts, "Drafts"),
];

impl Commands {
    /// The sidebar: unified folders, then each account's inbox and labels.
    pub fn sidebar(&self) -> CommandResult<Sidebar> {
        let mut accounts = Vec::new();
        for account in self.store.accounts()? {
            let labels = self.store.labels(&account.address)?;
            let inbox_id = system_label(&labels, SystemLabel::Inbox).map(|l| l.provider_id.clone());
            let inbox = FolderEntry {
                folder: Folder::Label {
                    account: account.address.clone(),
                    label: inbox_id.clone().unwrap_or_default(),
                },
                name: account.address.to_string(),
                unread: match &inbox_id {
                    Some(id) => self.store.unread_count(&account.address, id)?,
                    None => 0,
                },
            };
            let mut own = Vec::new();
            for label in labels.iter().filter(|l| l.kind == LabelKind::User) {
                own.push(FolderEntry {
                    folder: Folder::Label {
                        account: account.address.clone(),
                        label: label.provider_id.clone(),
                    },
                    name: label.name.clone(),
                    unread: self
                        .store
                        .unread_count(&account.address, &label.provider_id)?,
                });
            }
            accounts.push(AccountFolders {
                account,
                inbox,
                labels: own,
            });
        }
        let mut everywhere = Vec::new();
        for (system, name) in UNIFIED {
            let folder = Folder::Everywhere(system);
            let unread = match system {
                // Unread counts matter for the inbox; Sent and Drafts are
                // yours, and Gmail leaves them read.
                SystemLabel::Inbox => accounts.iter().map(|a| a.inbox.unread).sum(),
                _ => 0,
            };
            everywhere.push(FolderEntry {
                folder,
                name: name.to_owned(),
                unread,
            });
        }
        Ok(Sidebar {
            everywhere,
            accounts,
        })
    }

    /// The (account, provider label id) pairs a folder reads from.
    pub(crate) fn sources(&self, folder: &Folder) -> CommandResult<Vec<(EmailAddress, String)>> {
        match folder {
            Folder::Label { account, label } => {
                if self.store.account(account)?.is_none() {
                    return Err(CommandError::UnknownAccount(account.clone()));
                }
                Ok(vec![(account.clone(), label.clone())])
            }
            Folder::Everywhere(system) => {
                let mut sources = Vec::new();
                for account in self.store.accounts()? {
                    let labels = self.store.labels(&account.address)?;
                    if let Some(label) = system_label(&labels, *system) {
                        sources.push((account.address.clone(), label.provider_id.clone()));
                    }
                }
                Ok(sources)
            }
        }
    }
}

fn system_label(labels: &[Label], system: SystemLabel) -> Option<&Label> {
    labels.iter().find(|l| l.kind == LabelKind::System(system))
}
