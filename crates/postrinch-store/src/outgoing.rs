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

/// How far sending a message over SMTP got (IMAP accounts), so that a
/// message goes once: what is left after an attempt is decided by where
/// it stopped (docs/imap.md, "Writing and sending").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendStage {
    /// Not sent: the next attempt sends it.
    #[default]
    Queued,
    /// Its data reached the server and no answer came: it may have gone.
    /// The next attempt looks for it in Sent rather than send it; when
    /// it is also `refused`, it was not found there and waits for the
    /// person (Send again, or Discard).
    DataSent,
    /// The server took it. What is left is filing it in Sent and
    /// deleting the draft it came from; it is never sent again.
    Delivered,
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
    /// How far sending it got; always [`SendStage::Queued`] for Google
    /// accounts.
    pub stage: SendStage,
    /// The recipients the server took it for in earlier transactions,
    /// when it was sent in parts because the server takes only so many
    /// recipients at once: it never goes to them again.
    pub delivered_to: Vec<EmailAddress>,
    /// While it is [`SendStage::DataSent`]: the recipients of the
    /// transaction whose data went without an answer.
    pub sending_to: Vec<EmailAddress>,
}

impl Store {
    /// Keep the bytes of a file attached to a draft, returning their
    /// SHA-256 for its [`postrinch_model::DraftAttachment`]. Stored once
    /// however many drafts attach them; kept while queued mail refers to
    /// them (see [`Store::collect_garbage`]).
    pub fn put_attachment(&self, bytes: &[u8]) -> StoreResult<String> {
        Ok(self.blobs.put(bytes)?)
    }

    /// The bytes of a draft's attachment, if they are still here.
    pub fn attachment_bytes(&self, blob: &str) -> StoreResult<Option<Vec<u8>>> {
        Ok(self.blobs.get(blob)?)
    }

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

    /// Record how far sending it got.
    pub fn outgoing_stage(&self, id: u64, stage: SendStage) -> StoreResult<()> {
        let fields = FieldsBuilder::new().json("stage", &stage);
        self.db_update(TYPE, id, fields.build())
    }

    /// Its data is about to go to `to`: [`SendStage::DataSent`], for
    /// them.
    pub fn outgoing_sending(&self, id: u64, to: &[EmailAddress]) -> StoreResult<()> {
        let fields = FieldsBuilder::new()
            .json("stage", &SendStage::DataSent)
            .json("sending_to", &to);
        self.db_update(TYPE, id, fields.build())
    }

    /// The server took it for `delivered` (everyone it went to so far),
    /// in one write with how far sending it got now: `stage` is
    /// [`SendStage::Queued`] while some recipients are left,
    /// [`SendStage::Delivered`] once none are.
    pub fn outgoing_delivered(
        &self,
        id: u64,
        delivered: &[EmailAddress],
        stage: SendStage,
    ) -> StoreResult<()> {
        let fields = FieldsBuilder::new()
            .json("stage", &stage)
            .json("delivered_to", &delivered)
            .null("sending_to");
        self.db_update(TYPE, id, fields.build())
    }

    /// An attempt stopped before its data went: [`SendStage::Queued`],
    /// keeping whom earlier transactions delivered it to.
    pub fn outgoing_not_sent(&self, id: u64) -> StoreResult<()> {
        let fields = FieldsBuilder::new()
            .json("stage", &SendStage::Queued)
            .null("sending_to");
        self.db_update(TYPE, id, fields.build())
    }

    /// The person asked for mail waiting for them to be tried again: its
    /// error is forgotten and, if it may have gone, it is queued as new,
    /// for everyone earlier transactions did not deliver it to. Mail the
    /// server took stays [`SendStage::Delivered`]: only filing it is
    /// tried again, as it is never sent twice.
    pub fn outgoing_send_again(&self, id: u64) -> StoreResult<()> {
        let stage = decode(&self.get(TYPE, id)?)?.stage;
        let stage = match stage {
            SendStage::Delivered => SendStage::Delivered,
            SendStage::Queued | SendStage::DataSent => SendStage::Queued,
        };
        let fields = FieldsBuilder::new()
            .json("stage", &stage)
            .null("sending_to")
            .null("error")
            .bool("refused", false);
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
        stage: fields.opt_json("stage")?.unwrap_or_default(),
        delivered_to: fields.opt_json("delivered_to")?.unwrap_or_default(),
        sending_to: fields.opt_json("sending_to")?.unwrap_or_default(),
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
        assert_eq!(
            failed.stage,
            SendStage::Queued,
            "queued until a send says otherwise"
        );
        store.outgoing_stage(first, SendStage::DataSent).unwrap();
        store
            .outgoing_failed(first, "it may have gone", true)
            .unwrap();
        let doubtful = &store.outgoing_for(&me).unwrap()[0];
        assert_eq!(
            (doubtful.stage, doubtful.refused),
            (SendStage::DataSent, true)
        );
        store.outgoing_send_again(first).unwrap();
        let again = &store.outgoing_for(&me).unwrap()[0];
        assert_eq!(
            (again.stage, again.refused, again.error.as_deref()),
            (SendStage::Queued, false, None)
        );
        // Sent in parts: whom each part went to is kept.
        let (ada, bob) = (draft.to[0].address.clone(), me.clone());
        store
            .outgoing_sending(first, std::slice::from_ref(&ada))
            .unwrap();
        let sending = &store.outgoing_for(&me).unwrap()[0];
        assert_eq!(
            (sending.stage, sending.sending_to.clone()),
            (SendStage::DataSent, vec![ada.clone()])
        );
        store
            .outgoing_delivered(first, std::slice::from_ref(&ada), SendStage::Queued)
            .unwrap();
        store
            .outgoing_sending(first, std::slice::from_ref(&bob))
            .unwrap();
        store.outgoing_send_again(first).unwrap();
        let parted = &store.outgoing_for(&me).unwrap()[0];
        assert_eq!(
            (
                parted.stage,
                parted.delivered_to.clone(),
                parted.sending_to.clone()
            ),
            (SendStage::Queued, vec![ada.clone()], vec![])
        );
        store.outgoing_sending(first, &[bob]).unwrap();
        store.outgoing_not_sent(first).unwrap();
        let stopped = &store.outgoing_for(&me).unwrap()[0];
        assert_eq!(
            (
                stopped.stage,
                stopped.delivered_to.clone(),
                stopped.sending_to.is_empty()
            ),
            (SendStage::Queued, vec![ada], true)
        );
        store.outgoing_stage(first, SendStage::Delivered).unwrap();
        store
            .outgoing_failed(first, "Sent would not keep a copy", true)
            .unwrap();
        store.outgoing_send_again(first).unwrap();
        let delivered = &store.outgoing_for(&me).unwrap()[0];
        assert_eq!(
            (delivered.stage, delivered.refused),
            (SendStage::Delivered, false),
            "what went is never sent again"
        );
        store.outgoing_done(first).unwrap();
        assert_eq!(
            store.outgoing_for(&me).unwrap()[0].draft.subject,
            "Saturday"
        );
    }
}
