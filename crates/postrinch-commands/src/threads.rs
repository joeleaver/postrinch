//! Listing a folder's threads and reading one.

use std::collections::HashMap;

use postrinch_model::{
    EmailAddress, LabelKind, Message, ProviderMessageId, ProviderThreadId, SystemLabel,
    ThreadSummary,
};
use postrinch_render::{Options as RenderOptions, Rendered};
use postrinch_store::ThreadEntry;
use serde::{Deserialize, Serialize};

use crate::labels::Chip;
use crate::search::Excerpt;
use crate::{CommandResult, Commands, Folder};

pub use postrinch_model::ListOrder;

/// A page of a folder's threads, in the order asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadPage {
    /// How many threads the folder has in all.
    pub total: usize,
    pub offset: usize,
    pub threads: Vec<ThreadRow>,
}

/// A thread as a list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRow {
    pub summary: ThreadSummary,
    /// Its tag-like labels, leaving out the one being listed.
    pub chips: Vec<Chip>,
    /// Where a search matched it; `None` in a folder listing.
    pub excerpt: Option<Excerpt>,
    /// Found by the provider's own search, in mail not downloaded here.
    #[serde(default)]
    pub remote: bool,
    /// The one message the row stands for, when mail is listed one
    /// message at a time ([`crate::Grouping::Messages`]); `None` for a
    /// conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<ProviderMessageId>,
}

/// Gmail's inbox categories, in the order its tabs show them.
pub const CATEGORIES: [SystemLabel; 5] = [
    SystemLabel::CategoryPersonal,
    SystemLabel::CategoryUpdates,
    SystemLabel::CategoryPromotions,
    SystemLabel::CategorySocial,
    SystemLabel::CategoryForums,
];

/// A thread with all its messages, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub summary: ThreadSummary,
    pub messages: Vec<Message>,
}

impl Commands {
    /// The folder's threads from `offset`, at most `limit` of them. With a
    /// `category` (one of [`CATEGORIES`]), only threads Gmail also filed
    /// under it, as its inbox tabs do.
    pub fn list_threads(
        &self,
        folder: &Folder,
        category: Option<SystemLabel>,
        offset: usize,
        limit: usize,
    ) -> CommandResult<ThreadPage> {
        self.list_threads_in(folder, category, ListOrder::NewestFirst, offset, limit)
    }

    /// [`Commands::list_threads`] in `order`.
    pub fn list_threads_in(
        &self,
        folder: &Folder,
        category: Option<SystemLabel>,
        order: ListOrder,
        offset: usize,
        limit: usize,
    ) -> CommandResult<ThreadPage> {
        let unread = order == ListOrder::UnreadFirst;
        let mut entries = Vec::new();
        for (account, label) in self.sources(folder)? {
            let found = match category {
                None => self
                    .store
                    .label_threads_sorting(&account, &label, None, unread)?,
                Some(category) => match self.system_label_id(&account, category)? {
                    Some(id) => {
                        self.store
                            .label_threads_sorting(&account, &label, Some(&id), unread)?
                    }
                    None => Vec::new(),
                },
            };
            entries.extend(found);
        }
        let listed = match folder {
            Folder::Label { label, .. } => Some(label.as_str()),
            Folder::Everywhere(_) => None,
        };
        self.thread_page(entries, order, offset, limit, listed)
    }

    /// A page of `entries` in `order`, from `offset`, at most `limit`
    /// rows; their chips leave out `listed`, the label being listed.
    pub(crate) fn thread_page(
        &self,
        mut entries: Vec<ThreadEntry>,
        order: ListOrder,
        offset: usize,
        limit: usize,
        listed: Option<&str>,
    ) -> CommandResult<ThreadPage> {
        entries.sort_by(|a, b| {
            let tie = a
                .account
                .key()
                .cmp(&b.account.key())
                .then_with(|| a.thread.cmp(&b.thread));
            order.compare((a.unread, a.latest), (b.unread, b.latest), tie)
        });
        let total = entries.len();
        let mut threads = Vec::new();
        let mut account_chips: HashMap<String, HashMap<String, Chip>> = HashMap::new();
        for entry in entries.iter().skip(offset).take(limit) {
            let messages = self.store.thread_messages(&entry.account, &entry.thread)?;
            let Some(summary) = ThreadSummary::from_messages(&messages) else {
                continue;
            };
            let known = match account_chips.entry(entry.account.key()) {
                std::collections::hash_map::Entry::Occupied(known) => known.into_mut(),
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(self.account_chips(&entry.account)?)
                }
            };
            let chips = summary
                .labels
                .iter()
                .filter(|l| Some(l.as_str()) != listed)
                .filter_map(|l| known.get(l).cloned())
                .collect();
            threads.push(ThreadRow {
                summary,
                chips,
                excerpt: None,
                remote: false,
                message: None,
            });
        }
        Ok(ThreadPage {
            total,
            offset,
            threads,
        })
    }

    /// The provider's id for one of an account's system labels.
    pub(crate) fn system_label_id(
        &self,
        account: &EmailAddress,
        system: SystemLabel,
    ) -> CommandResult<Option<String>> {
        Ok(self
            .store
            .labels(account)?
            .into_iter()
            .find(|l| l.kind == LabelKind::System(system))
            .map(|l| l.provider_id))
    }

    /// A thread and its messages. `None` for a thread the store does not
    /// have.
    pub fn read_thread(
        &self,
        account: &EmailAddress,
        thread: &ProviderThreadId,
    ) -> CommandResult<Option<Thread>> {
        self.check(account)?;
        let messages = self.store.thread_messages(account, thread)?;
        Ok(ThreadSummary::from_messages(&messages).map(|summary| Thread { summary, messages }))
    }

    /// A downloaded message as it shows: its best part, sanitized (see
    /// `postrinch-render`), with pictures from the web only when `remote`.
    /// `None` for a message not downloaded or that does not parse.
    pub fn render_message(
        &self,
        account: &EmailAddress,
        message: &ProviderMessageId,
        remote: bool,
    ) -> CommandResult<Option<Rendered>> {
        self.check(account)?;
        let Some(raw) = self.store.raw_message(account, message)? else {
            return Ok(None);
        };
        Ok(postrinch_render::render(
            &raw,
            RenderOptions {
                remote,
                ..RenderOptions::default()
            },
        ))
    }

    /// A downloaded message's text for a program reading it: what its reader
    /// sees, and what it hides from its reader (`postrinch_render::
    /// MessageText`). `None` for a message not downloaded or that does not
    /// parse.
    pub fn message_text(
        &self,
        account: &EmailAddress,
        message: &ProviderMessageId,
    ) -> CommandResult<Option<postrinch_render::MessageText>> {
        self.check(account)?;
        let Some(raw) = self.store.raw_message(account, message)? else {
            return Ok(None);
        };
        Ok(postrinch_render::message_text(&raw))
    }
}
