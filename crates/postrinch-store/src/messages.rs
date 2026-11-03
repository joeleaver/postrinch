use postrinch_model::{
    EmailAddress, Flags, Message, MessageHeaders, ProviderMessageId, ProviderThreadId, Timestamp,
};

use crate::codec::{Fields, FieldsBuilder};
use crate::keys::scoped;
use crate::{Store, StoreError, StoreResult};

pub(crate) const TYPE: &str = "Message";
pub(crate) const CONTENT: &str = "MessageContent";
pub(crate) const THREAD: &str = "Thread";

/// Subject, then everyone it was from or to, then the body: what full-text
/// search matches against.
fn search_text(message: &Message) -> String {
    let people: Vec<String> = message
        .headers
        .participants()
        .iter()
        .map(|m| m.to_string())
        .collect();
    format!(
        "{}\n{}\n{}",
        message.subject(),
        people.join(", "),
        message.body_text
    )
}

impl Store {
    /// Store a message, or bring a stored one up to date: its flags, labels
    /// and everything else. `raw` is the whole RFC 5322 message, when it has
    /// been fetched; storing without it keeps any raw message already
    /// stored.
    ///
    /// A message that is not downloaded (headers only) is stored as such,
    /// to be downloaded later; but over one already downloaded it changes
    /// only labels and flags, so a body is never lost.
    ///
    /// The message's labels must already be in the store
    /// ([`Store::put_label`]). Calling this again with the same message
    /// repairs whatever an interrupted earlier call left undone.
    pub fn put_message(&self, message: &Message, raw: Option<&[u8]>) -> StoreResult<()> {
        let account = &message.account;
        // Headers alone never replace a downloaded message: they would wipe
        // its body. Only what headers can change is brought up to date.
        if !message.downloaded
            && let Some(existing) = self.find(TYPE, "key", &scoped(account, &message.id.0))?
            && existing.bool("downloaded")?
        {
            return self.set_message_labels(account, &message.id, &message.labels, message.flags);
        }
        let account_id = self.account_id(account)?;
        let thread_id = self.thread_id(account, account_id, &message.thread)?;
        // Changes made here and not yet forgotten win over what the
        // provider reported (see outbox.rs).
        let (mut labels, mut flags) = (message.labels.clone(), message.flags);
        self.overlay(account, &message.id, &mut labels, &mut flags)?;
        let label_ids = self.label_ids(account, &labels)?;
        let from_ids = self.person_ids(&message.headers.from)?;
        let h = &message.headers;
        let recipient_ids = self.person_ids(h.to.iter().chain(&h.cc).chain(&h.bcc))?;
        let raw_sha256 = raw.map(|bytes| self.blobs.put(bytes)).transpose()?;

        let mut fields = FieldsBuilder::new()
            .opt_str("rfc_message_id", h.message_id.clone())
            .opt_str("subject", h.subject.clone())
            .str("snippet", message.snippet.clone())
            .datetime("received", message.received.as_millis())
            .bool("unread", flags.unread)
            .bool("starred", flags.starred)
            .bool("has_attachments", message.has_attachments())
            .u64("size", message.size)
            .bool("downloaded", message.downloaded);
        if let Some(hash) = raw_sha256 {
            fields = fields.str("raw_sha256", hash);
        }

        let key = scoped(account, &message.id.0);
        let create = fields
            .clone()
            .str("key", key.clone())
            .str("provider_id", message.id.0.clone())
            .relation("thread", thread_id);
        let id = self.upsert(TYPE, "key", &key, create.build(), fields.build())?;

        self.set_links(TYPE, id, "thread", &[thread_id])?;
        self.set_links(TYPE, id, "labels", &label_ids)?;
        self.set_links(TYPE, id, "from", &from_ids)?;
        self.set_links(TYPE, id, "recipients", &recipient_ids)?;

        // What the message says, kept apart (see schema.rhype).
        let content = FieldsBuilder::new()
            .str("search_text", search_text(message))
            .json("headers", &message.headers)
            .str("body_text", message.body_text.clone())
            .json("attachments", &message.attachments);
        let create = content
            .clone()
            .str("key", key.clone())
            .relation("message", id);
        self.upsert(CONTENT, "key", &key, create.build(), content.build())?;
        Ok(())
    }

    /// Change a stored message's labels and flags, as when the provider
    /// reports that it was read, starred, archived or labelled. Changes made
    /// here that the provider may not have yet are applied over them (see
    /// outbox.rs).
    pub fn set_message_labels(
        &self,
        account: &EmailAddress,
        id: &ProviderMessageId,
        labels: &[String],
        flags: Flags,
    ) -> StoreResult<()> {
        let message_id = self.message_id(account, id)?;
        self.write_labels(account, message_id, labels.to_vec(), flags)
    }

    /// Write a message's labels and flags, with the outbox applied over
    /// them.
    pub(crate) fn write_labels(
        &self,
        account: &EmailAddress,
        message_id: u64,
        mut labels: Vec<String>,
        mut flags: Flags,
    ) -> StoreResult<()> {
        let provider_id = ProviderMessageId(self.get(TYPE, message_id)?.str("provider_id")?);
        self.overlay(account, &provider_id, &mut labels, &mut flags)?;
        let label_ids = self.label_ids(account, &labels)?;
        let fields = FieldsBuilder::new()
            .bool("unread", flags.unread)
            .bool("starred", flags.starred)
            .build();
        self.db_update(TYPE, message_id, fields)?;
        self.set_links(TYPE, message_id, "labels", &label_ids)
    }

    pub fn message(
        &self,
        account: &EmailAddress,
        id: &ProviderMessageId,
    ) -> StoreResult<Option<Message>> {
        match self.find(TYPE, "key", &scoped(account, &id.0))? {
            Some(fields) => self.decode_message(&fields).map(Some),
            None => Ok(None),
        }
    }

    /// The raw RFC 5322 message, if it has been fetched.
    pub fn raw_message(
        &self,
        account: &EmailAddress,
        id: &ProviderMessageId,
    ) -> StoreResult<Option<Vec<u8>>> {
        let fields = self.get(TYPE, self.message_id(account, id)?)?;
        match fields.opt_str("raw_sha256")? {
            Some(hash) => Ok(self.blobs.get(&hash)?),
            None => Ok(None),
        }
    }

    /// Remove a message, and its thread if it was the last one. Removing a
    /// message that is not there does nothing.
    pub fn remove_message(
        &self,
        account: &EmailAddress,
        id: &ProviderMessageId,
    ) -> StoreResult<()> {
        let Some(fields) = self.find(TYPE, "key", &scoped(account, &id.0))? else {
            return Ok(());
        };
        let thread = self.link(TYPE, fields.id(), "thread")?;
        self.db_delete(TYPE, fields.id())?;
        if let Some(thread) = thread
            && self.links(THREAD, thread, "messages")?.is_empty()
        {
            self.db_delete(THREAD, thread)?;
        }
        Ok(())
    }

    /// A thread's messages, oldest first. Empty for a thread the store does
    /// not have.
    pub fn thread_messages(
        &self,
        account: &EmailAddress,
        thread: &ProviderThreadId,
    ) -> StoreResult<Vec<Message>> {
        let Some(thread) = self.find(THREAD, "key", &scoped(account, &thread.0))? else {
            return Ok(Vec::new());
        };
        let ids = self.links(THREAD, thread.id(), "messages")?;
        let mut messages = self
            .get_many(TYPE, &ids)?
            .iter()
            .map(|f| self.decode_message(f))
            .collect::<StoreResult<Vec<_>>>()?;
        messages.sort_by_key(|m| m.received);
        Ok(messages)
    }

    /// Whether a message is stored.
    pub fn has_message(&self, account: &EmailAddress, id: &ProviderMessageId) -> StoreResult<bool> {
        Ok(self.find(TYPE, "key", &scoped(account, &id.0))?.is_some())
    }

    /// The provider ids of every message stored for an account, for
    /// reconciling with the provider's own list after a full sync.
    pub fn message_ids(&self, account: &EmailAddress) -> StoreResult<Vec<ProviderMessageId>> {
        let prefix = scoped(account, "");
        let mut ids = Vec::new();
        for object in self.db.scan_type_projected(TYPE, &["key"])? {
            let key = Fields::new(TYPE, object).str("key")?;
            if let Some(id) = key.strip_prefix(&prefix) {
                ids.push(ProviderMessageId(id.to_owned()));
            }
        }
        ids.sort();
        Ok(ids)
    }

    pub(crate) fn message_id(
        &self,
        account: &EmailAddress,
        id: &ProviderMessageId,
    ) -> StoreResult<u64> {
        self.find(TYPE, "key", &scoped(account, &id.0))?
            .map(|f| f.id())
            .ok_or_else(|| StoreError::UnknownMessage {
                account: account.clone(),
                id: id.0.clone(),
            })
    }

    fn thread_id(
        &self,
        account: &EmailAddress,
        account_id: u64,
        thread: &ProviderThreadId,
    ) -> StoreResult<u64> {
        let key = scoped(account, &thread.0);
        let create = FieldsBuilder::new()
            .str("key", key.clone())
            .relation("account", account_id)
            .str("provider_id", thread.0.clone());
        self.upsert(
            THREAD,
            "key",
            &key,
            create.build(),
            FieldsBuilder::new().build(),
        )
    }

    /// Rebuild a [`Message`] from its object and the objects it links to.
    /// Labels come back sorted by provider id.
    pub(crate) fn decode_message(&self, fields: &Fields) -> StoreResult<Message> {
        let id = fields.id();
        let missing = |what: &str| StoreError::corrupt(TYPE, id, format!("no {what} link"));

        // A message reaches its account through its thread (see schema.rhype
        // on why Message has no account link of its own).
        let thread_id = self
            .link(TYPE, id, "thread")?
            .ok_or_else(|| missing("thread"))?;
        let thread = ProviderThreadId(self.get(THREAD, thread_id)?.str("provider_id")?);
        let account_id = self
            .link(THREAD, thread_id, "account")?
            .ok_or_else(|| StoreError::corrupt(THREAD, thread_id, "no account link"))?;
        let address = self.get("Account", account_id)?.str("address")?;
        let account = EmailAddress::parse(&address)
            .map_err(|e| StoreError::corrupt("Account", account_id, e))?;

        let label_ids = self.links(TYPE, id, "labels")?;
        let mut labels = self
            .get_many(crate::labels::TYPE, &label_ids)?
            .iter()
            .map(|f| f.str("provider_id"))
            .collect::<StoreResult<Vec<_>>>()?;
        labels.sort();

        let content_id = self
            .links(TYPE, id, "content")?
            .into_iter()
            .next()
            .ok_or_else(|| missing("content"))?;
        let content = self.get(CONTENT, content_id)?;
        let headers: MessageHeaders = content.json("headers")?;
        Ok(Message {
            account,
            id: ProviderMessageId(fields.str("provider_id")?),
            thread,
            headers,
            received: Timestamp::from_millis(fields.datetime("received")?),
            labels,
            flags: Flags {
                unread: fields.bool("unread")?,
                starred: fields.bool("starred")?,
            },
            snippet: fields.str("snippet")?,
            body_text: content.str("body_text")?,
            attachments: content.json("attachments")?,
            size: fields.u64("size")?,
            downloaded: fields.bool("downloaded")?,
        })
    }
}
