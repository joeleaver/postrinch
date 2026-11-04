//! Mail written here, on its way out: a message to send, a draft to keep
//! with the provider, or an agent's proposal waiting for the person.
//!
//! Sending is local-first, like changing mail (outbox.rs): Send records the
//! message here at once and sync's sender delivers it, in the order it was
//! made, across going offline and restarting. Once the provider has it the
//! record goes; the provider's own copy comes back through sync (Sent, or
//! Drafts). A proposal is never sent from here: the window opens it for the
//! person, whose Send queues it as a message of their own.

use postrinch_model::{Draft, EmailAddress, Timestamp};
use serde::{Deserialize, Serialize};

use crate::codec::{Fields, FieldsBuilder};
use crate::keys::scoped;
use crate::{Store, StoreResult};

pub(crate) const TYPE: &str = "Outgoing";

/// What to do with a draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutgoingKind {
    /// Send it.
    Send,
    /// Keep it as one of the provider's drafts.
    Keep,
    /// An agent wrote it: show it to the person, who may send it.
    Proposed,
}

/// One piece of outgoing mail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    /// The store's id, to mark it done or failed.
    pub id: u64,
    pub kind: OutgoingKind,
    pub draft: Draft,
    /// The order it was made in, across the store.
    pub seq: u64,
    pub created: Timestamp,
    pub attempts: u32,
    /// Why the last attempt failed.
    pub error: Option<String>,
    /// Whether the provider refused it for good: it waits for the person
    /// to change it, and is not tried again.
    pub refused: bool,
}

impl Store {
    /// Record outgoing mail; it is sent (or kept, or shown) from here.
    pub fn queue_outgoing(&self, draft: &Draft, kind: OutgoingKind) -> StoreResult<u64> {
        let _order = self.outbox_lock.lock().unwrap_or_else(|e| e.into_inner());
        let seq = self.next_seq()?.max(self.next_outgoing_seq()?);
        let key = scoped(&draft.account, &format!("{seq:020}"));
        let fields = FieldsBuilder::new()
            .str("key", key.clone())
            .str("account", draft.account.to_string())
            .u64("seq", seq)
            .json("kind", &kind)
            .json("draft", draft)
            .datetime("created", crate::outbox::now().as_millis())
            .u32("attempts", 0)
            .bool("refused", false);
        self.upsert(TYPE, "key", &key, fields.clone().build(), fields.build())
    }

    /// Every account's outgoing mail, oldest first.
    pub fn outgoing(&self) -> StoreResult<Vec<Outgoing>> {
        let mut all = self
            .db
            .scan_type(TYPE)?
            .into_iter()
            .map(|o| decode(&Fields::new(TYPE, o)))
            .collect::<StoreResult<Vec<_>>>()?;
        all.sort_by_key(|o| o.seq);
        Ok(all)
    }

    /// One account's mail for the provider (not proposals), oldest first.
    pub fn outgoing_for(&self, account: &EmailAddress) -> StoreResult<Vec<Outgoing>> {
        let mut mine = self.outgoing()?;
        mine.retain(|o| o.draft.account == *account && o.kind != OutgoingKind::Proposed);
        Ok(mine)
    }

    /// The provider has it, or the person settled a proposal: forget it.
    pub fn outgoing_done(&self, id: u64) -> StoreResult<()> {
        self.db_delete(TYPE, id)
    }

    /// Delivering it failed, for `error`. `refused`: for good, so it is
    /// not tried again until the person changes it.
    pub fn outgoing_failed(&self, id: u64, error: &str, refused: bool) -> StoreResult<()> {
        let attempts = self.get(TYPE, id)?.u32("attempts")?;
        let fields = FieldsBuilder::new()
            .u32("attempts", attempts.saturating_add(1))
            .str("error", error)
            .bool("refused", refused);
        self.db_update(TYPE, id, fields.build())
    }

    fn next_outgoing_seq(&self) -> StoreResult<u64> {
        Ok(self
            .outgoing()?
            .iter()
            .map(|o| o.seq + 1)
            .max()
            .unwrap_or(1))
    }
}

fn decode(fields: &Fields) -> StoreResult<Outgoing> {
    Ok(Outgoing {
        id: fields.id(),
        kind: fields.json("kind")?,
        draft: fields.json("draft")?,
        seq: fields.u64("seq")?,
        created: Timestamp::from_millis(fields.datetime("created")?),
        attempts: fields.u32("attempts")?,
        error: fields.opt_str("error")?,
        refused: fields.bool("refused")?,
    })
}

#[cfg(test)]
mod tests {
    use postrinch_model::{Account, Mailbox, Provider};

    use super::*;

    #[test]
    fn queued_in_order_until_done() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let me = EmailAddress::parse("me@example.org").unwrap();
        store
            .put_account(&Account::new(me.clone(), Provider::Google))
            .unwrap();
        let mut draft = Draft::new(me.clone());
        draft.to = vec![Mailbox::new(
            None,
            EmailAddress::parse("ada@example.org").unwrap(),
        )];
        draft.subject = "Thursday".into();

        let first = store.queue_outgoing(&draft, OutgoingKind::Send).unwrap();
        draft.subject = "Friday".into();
        store
            .queue_outgoing(&draft, OutgoingKind::Proposed)
            .unwrap();
        draft.subject = "Saturday".into();
        store.queue_outgoing(&draft, OutgoingKind::Keep).unwrap();

        let all = store.outgoing().unwrap();
        let subjects: Vec<&str> = all.iter().map(|o| o.draft.subject.as_str()).collect();
        assert_eq!(subjects, ["Thursday", "Friday", "Saturday"]);
        let mine = store.outgoing_for(&me).unwrap();
        assert_eq!(mine.len(), 2, "proposals are not the provider's");

        store.outgoing_failed(first, "offline", false).unwrap();
        let failed = &store.outgoing_for(&me).unwrap()[0];
        assert_eq!(
            (failed.attempts, failed.error.as_deref(), failed.refused),
            (1, Some("offline"), false)
        );
        store.outgoing_done(first).unwrap();
        assert_eq!(
            store.outgoing_for(&me).unwrap()[0].draft.subject,
            "Saturday"
        );
    }
}
