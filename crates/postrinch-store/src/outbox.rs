//! The outbox: changes the person made here, on their way to the provider.
//!
//! [`Store::change_messages`] records a change and applies it to the stored
//! messages at once, so the window shows it before the provider knows. The
//! provider's sync sends what waits ([`Store::outbox`]), marks each entry
//! sent, and forgets sent entries once a sync that began after them has
//! finished ([`Store::forget_sent`]): by then the provider's own report of
//! the messages includes the change.
//!
//! Until an entry is forgotten, every write of a message's labels and
//! flags applies it again over what is written ([`Store::overlay`]), in the
//! order the changes were made. A change only adds, removes or sets, so
//! applying it again is harmless, and sync can never undo a change still on
//! its way out.

use std::time::{SystemTime, UNIX_EPOCH};

use postrinch_model::{EmailAddress, Flags, MessageChange, ProviderMessageId, Timestamp};

use crate::codec::{Fields, FieldsBuilder};
use crate::keys::scoped;
use crate::messages::TYPE as MESSAGE;
use crate::{Store, StoreError, StoreResult, labels};

pub(crate) const TYPE: &str = "OutboxEntry";

/// One change, to one or more messages of one account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEntry {
    /// The store's id for the entry, to mark it sent or failed.
    pub id: u64,
    pub account: EmailAddress,
    /// The order the changes were made in, across the store.
    pub seq: u64,
    pub messages: Vec<ProviderMessageId>,
    pub change: MessageChange,
    pub created: Timestamp,
    /// How many times sending it has failed.
    pub attempts: u32,
    pub error: Option<String>,
    /// When the provider took it; `None` while it waits.
    pub sent: Option<Timestamp>,
}

fn now() -> Timestamp {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    Timestamp::from_millis(millis)
}

impl Store {
    /// Change some of an account's messages: apply the change to them now,
    /// and record it for the provider. The messages and any labels it adds
    /// must be stored; an empty change or no messages does nothing.
    pub fn change_messages(
        &self,
        account: &EmailAddress,
        messages: &[ProviderMessageId],
        change: &MessageChange,
    ) -> StoreResult<()> {
        if messages.is_empty() || change.is_empty() {
            return Ok(());
        }
        // Refuse before recording anything the provider would be sent.
        self.label_ids(account, &change.add_labels)?;
        let ids = messages
            .iter()
            .map(|m| self.message_id(account, m))
            .collect::<StoreResult<Vec<_>>>()?;

        let _order = self.outbox_lock.lock().unwrap_or_else(|e| e.into_inner());
        let seq = self.next_seq()?;
        let key = scoped(account, &format!("{seq:020}"));
        let fields = FieldsBuilder::new()
            .str("key", key.clone())
            .str("account", account.to_string())
            .u64("seq", seq)
            .json("messages", &messages)
            .json("change", change)
            .datetime("created", now().as_millis())
            .u32("attempts", 0);
        self.upsert(TYPE, "key", &key, fields.clone().build(), fields.build())?;

        // Written after the entry, so a crash between the two leaves a
        // change the provider will still be sent, and whose result sync
        // brings back.
        for id in ids {
            let (labels, flags) = self.stored_labels(id)?;
            self.write_labels(account, id, labels, flags)?;
        }
        Ok(())
    }

    /// The account's entries, waiting or sent, in the order they were made.
    pub fn outbox(&self, account: &EmailAddress) -> StoreResult<Vec<OutboxEntry>> {
        let mut entries: Vec<OutboxEntry> = self
            .outbox_all()?
            .into_iter()
            .filter(|e| e.account == *account)
            .collect();
        entries.sort_by_key(|e| e.seq);
        Ok(entries)
    }

    /// The provider took the entry.
    pub fn outbox_sent(&self, entry: u64) -> StoreResult<()> {
        let fields = FieldsBuilder::new()
            .datetime("sent", now().as_millis())
            .null("error");
        self.db_update(TYPE, entry, fields.build())
    }

    /// Sending the entry failed, for `error`; it is tried again later.
    pub fn outbox_failed(&self, entry: u64, error: &str) -> StoreResult<()> {
        let attempts = self.get(TYPE, entry)?.u32("attempts")?;
        let fields = FieldsBuilder::new()
            .u32("attempts", attempts.saturating_add(1))
            .str("error", error);
        self.db_update(TYPE, entry, fields.build())
    }

    /// Drop an entry without sending it: one the provider refused for good.
    /// What it changed stays until sync brings the provider's state back.
    pub fn outbox_drop(&self, entry: u64) -> StoreResult<()> {
        self.db_delete(TYPE, entry)
    }

    /// Forget the account's entries sent before `before`: the start of a
    /// sync that has since finished, whose report of the messages includes
    /// them. Returns how many were forgotten.
    pub fn forget_sent(&self, account: &EmailAddress, before: Timestamp) -> StoreResult<usize> {
        let mut forgotten = 0;
        for entry in self.outbox(account)? {
            if entry.sent.is_some_and(|sent| sent < before) {
                self.db_delete(TYPE, entry.id)?;
                forgotten += 1;
            }
        }
        Ok(forgotten)
    }

    /// Apply the account's entries for one message over the labels and
    /// flags about to be written for it. Labels an entry adds that the
    /// store no longer has are left out: the provider removed them, and
    /// will refuse the entry.
    pub(crate) fn overlay(
        &self,
        account: &EmailAddress,
        message: &ProviderMessageId,
        labels: &mut Vec<String>,
        flags: &mut Flags,
    ) -> StoreResult<()> {
        for entry in self.outbox(account)? {
            if !entry.messages.contains(message) {
                continue;
            }
            let mut change = entry.change;
            change.add_labels.retain(|l| {
                matches!(
                    self.find(labels::TYPE, "key", &scoped(account, l)),
                    Ok(Some(_))
                )
            });
            change.apply(labels, flags);
        }
        Ok(())
    }

    fn outbox_all(&self) -> StoreResult<Vec<OutboxEntry>> {
        self.db
            .scan_type(TYPE)?
            .into_iter()
            .map(|o| decode(&Fields::new(TYPE, o)))
            .collect()
    }

    fn next_seq(&self) -> StoreResult<u64> {
        Ok(self
            .outbox_all()?
            .iter()
            .map(|e| e.seq + 1)
            .max()
            .unwrap_or(1))
    }

    /// A stored message's labels and flags, as stored.
    fn stored_labels(&self, id: u64) -> StoreResult<(Vec<String>, Flags)> {
        let fields = self.get(MESSAGE, id)?;
        let flags = Flags {
            unread: fields.bool("unread")?,
            starred: fields.bool("starred")?,
        };
        let labels = self
            .get_many(labels::TYPE, &self.links(MESSAGE, id, "labels")?)?
            .iter()
            .map(|f| f.str("provider_id"))
            .collect::<StoreResult<Vec<_>>>()?;
        Ok((labels, flags))
    }
}

fn decode(fields: &Fields) -> StoreResult<OutboxEntry> {
    let address = fields.str("account")?;
    let account =
        EmailAddress::parse(&address).map_err(|e| StoreError::corrupt(TYPE, fields.id(), e))?;
    Ok(OutboxEntry {
        id: fields.id(),
        account,
        seq: fields.u64("seq")?,
        messages: fields.json("messages")?,
        change: fields.json("change")?,
        created: Timestamp::from_millis(fields.datetime("created")?),
        attempts: fields.u32("attempts")?,
        error: fields.opt_str("error")?,
        sent: fields.opt_datetime("sent")?.map(Timestamp::from_millis),
    })
}
