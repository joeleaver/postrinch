//! Each thread's latest activity, kept on the thread, so a mailbox is
//! listed in order without reading every message of every thread in it.
//!
//! Storing a message brings its thread's `latest` up to the message's
//! time; removing one works it out again from what is left. A thread from
//! a store written before threads kept it has none: listing works it out
//! without keeping it, as a write per thread would hold a listing up for
//! minutes, and [`Store::fill_thread_latest`] keeps it for them all, once,
//! in the background.

use crate::codec::FieldsBuilder;
use crate::messages::{THREAD, TYPE};
use crate::{Store, StoreResult};

impl Store {
    /// A message received at `received` (milliseconds) is now in `thread`.
    pub(crate) fn thread_has(&self, thread: u64, received: i64) -> StoreResult<()> {
        let _one_at_a_time = self.thread_lock.lock().unwrap_or_else(|e| e.into_inner());
        match self.get(THREAD, thread)?.opt_datetime("latest")? {
            Some(latest) if latest >= received => Ok(()),
            Some(_) => self.set_latest(thread, received),
            // Older than this field: its other messages decide too.
            None => self.work_out_latest(thread).map(drop),
        }
    }

    /// When a thread's latest message arrived: as kept, or else worked out
    /// from its messages. `None` for a thread with no messages.
    pub(crate) fn thread_latest(&self, thread: &crate::codec::Fields) -> StoreResult<Option<i64>> {
        match thread.opt_datetime("latest")? {
            Some(latest) => Ok(Some(latest)),
            None => self.latest_of_messages(thread.id()),
        }
    }

    /// Keep the latest activity of every thread that lacks it: those of a
    /// store written before threads kept it. Run once, in the background;
    /// returns how many were filled.
    pub fn fill_thread_latest(&self) -> StoreResult<usize> {
        let mut filled = 0;
        for object in self.db.scan_type_projected(THREAD, &["latest"])? {
            let fields = crate::codec::Fields::new(THREAD, object);
            if fields.opt_datetime("latest")?.is_some() {
                continue;
            }
            let _one_at_a_time = self.thread_lock.lock().unwrap_or_else(|e| e.into_inner());
            // Read again under the lock: a message may have filled it.
            if self
                .get(THREAD, fields.id())?
                .opt_datetime("latest")?
                .is_none()
                && self.work_out_latest(fields.id())?.is_some()
            {
                filled += 1;
            }
        }
        Ok(filled)
    }

    /// Work a thread's latest out from its messages, after one was removed.
    pub(crate) fn thread_lost_one(&self, thread: u64) -> StoreResult<()> {
        let _one_at_a_time = self.thread_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.work_out_latest(thread).map(drop)
    }

    /// The caller holds `thread_lock`.
    fn work_out_latest(&self, thread: u64) -> StoreResult<Option<i64>> {
        let latest = self.latest_of_messages(thread)?;
        if let Some(latest) = latest {
            self.set_latest(thread, latest)?;
        }
        Ok(latest)
    }

    fn latest_of_messages(&self, thread: u64) -> StoreResult<Option<i64>> {
        let messages = self.links(THREAD, thread, "messages")?;
        let mut latest = None;
        for message in self.get_many(TYPE, &messages)? {
            latest = latest.max(Some(message.datetime("received")?));
        }
        Ok(latest)
    }

    fn set_latest(&self, thread: u64, latest: i64) -> StoreResult<()> {
        let fields = FieldsBuilder::new().datetime("latest", latest).build();
        self.db_update(THREAD, thread, fields)
    }
}

#[cfg(test)]
mod tests {
    use postrinch_model::{
        Account, EmailAddress, Flags, Label, LabelKind, Message, MessageHeaders, Provider,
        ProviderMessageId, ProviderThreadId, SystemLabel, Timestamp,
    };

    use crate::Store;
    use crate::codec::FieldsBuilder;
    use crate::keys::scoped;
    use crate::messages::THREAD;

    fn message(me: &EmailAddress, id: &str, at: i64) -> Message {
        Message {
            account: me.clone(),
            id: ProviderMessageId(id.into()),
            thread: ProviderThreadId("t1".into()),
            headers: MessageHeaders::default(),
            received: Timestamp::from_millis(at),
            labels: vec!["INBOX".into()],
            flags: Flags::default(),
            snippet: String::new(),
            body_text: String::new(),
            attachments: Vec::new(),
            size: 0,
            downloaded: true,
        }
    }

    fn latest(store: &Store, me: &EmailAddress) -> i64 {
        store.label_threads(me, "INBOX").unwrap()[0]
            .latest
            .as_millis()
    }

    #[test]
    fn latest_follows_messages_in_and_out() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let me = EmailAddress::parse("me@example.org").unwrap();
        store
            .put_account(&Account::new(me.clone(), Provider::Google))
            .unwrap();
        let inbox = Label::new("INBOX", "Inbox", LabelKind::System(SystemLabel::Inbox));
        store.put_label(&me, &inbox).unwrap();

        store.put_message(&message(&me, "m2", 200), None).unwrap();
        store.put_message(&message(&me, "m1", 100), None).unwrap();
        assert_eq!(
            latest(&store, &me),
            200,
            "an older message does not lower it"
        );
        store.put_message(&message(&me, "m3", 300), None).unwrap();
        assert_eq!(latest(&store, &me), 300);
        store
            .remove_message(&me, &ProviderMessageId("m3".into()))
            .unwrap();
        assert_eq!(latest(&store, &me), 200, "worked out again after a removal");

        // A thread stored before threads kept their latest gets it when
        // listed, and when a message joins it.
        let thread = store
            .find(THREAD, "key", &scoped(&me, "t1"))
            .unwrap()
            .unwrap();
        let clear = || FieldsBuilder::new().null("latest").build();
        store.db_update(THREAD, thread.id(), clear()).unwrap();
        assert_eq!(latest(&store, &me), 200);
        store.db_update(THREAD, thread.id(), clear()).unwrap();
        store.put_message(&message(&me, "m0", 50), None).unwrap();
        assert_eq!(
            latest(&store, &me),
            200,
            "not just the joining message's time"
        );
    }
}
