//! Whether the account answered each thread last (`last:me` in search),
//! kept on the thread, so a search of thousands of threads for it reads a
//! field of each rather than every message of every thread.
//!
//! The thread keeps when its latest message that is not a draft arrived
//! (`last_message`) and whether that message is the account's answer
//! (`replied_last`, by `postrinch_model::answered`). Storing a later
//! message sets both from it alone; removing one works them out again from
//! what is left. A thread from a store written before threads kept them has
//! neither: a search works it out when it meets the thread, and
//! [`Store::fill_replied_last`] keeps it for them all, once, in the
//! background.

use std::collections::HashMap;

use postrinch_model::{EmailAddress, Message, answered, is_draft};
use rhypedb_engine::object::Value;

use crate::codec::{Fields, FieldsBuilder};
use crate::messages::{THREAD, TYPE};
use crate::{Found, Store, StoreResult};

impl Store {
    /// `message` is now stored in `thread`.
    pub(crate) fn thread_answered(
        &self,
        thread: u64,
        account: &EmailAddress,
        message: &Message,
    ) -> StoreResult<()> {
        if is_draft(message) {
            return Ok(());
        }
        let _one_at_a_time = self.thread_lock.lock().unwrap_or_else(|e| e.into_inner());
        let at = message.received.as_millis();
        match self.get(THREAD, thread)?.opt_datetime("last_message")? {
            // An older message: the latest still decides.
            Some(last) if last > at => Ok(()),
            Some(_) => self.set_answered(thread, at, answered(account, message).is_some()),
            // Older than these fields: its other messages decide too.
            None => self.work_out_answered(thread).map(drop),
        }
    }

    /// Work out again who answered last, after one of the thread's
    /// messages was removed.
    pub(crate) fn thread_answered_again(&self, thread: u64) -> StoreResult<()> {
        let _one_at_a_time = self.thread_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.work_out_answered(thread).map(drop)
    }

    /// Keep who answered last for every thread that lacks it: those of a
    /// store written before threads kept it. Run once, in the background;
    /// returns how many were filled.
    pub fn fill_replied_last(&self) -> StoreResult<usize> {
        let mut filled = 0;
        for object in self.db.scan_type_projected(THREAD, &["replied_last"])? {
            let fields = Fields::new(THREAD, object);
            if !matches!(fields.value("replied_last"), Value::Null) {
                continue;
            }
            let _one_at_a_time = self.thread_lock.lock().unwrap_or_else(|e| e.into_inner());
            // Read again under the lock: a message may have filled it.
            if matches!(
                self.get(THREAD, fields.id())?.value("replied_last"),
                Value::Null
            ) {
                self.work_out_answered(fields.id())?;
                filled += 1;
            }
        }
        Ok(filled)
    }

    /// The found messages whose thread the account answered last (`me`)
    /// or someone else did, each thread looked at once.
    pub fn whose_last(&self, found: Vec<Found>, me: bool) -> StoreResult<Vec<Found>> {
        let mut answered: HashMap<u64, bool> = HashMap::new();
        let mut kept = Vec::new();
        for f in found {
            let Some(thread) = self.link(TYPE, f.object, "thread")? else {
                continue;
            };
            let mine = match answered.get(&thread) {
                Some(&mine) => mine,
                None => {
                    let mine = match self.get(THREAD, thread)?.value("replied_last") {
                        Value::Bool(mine) => *mine,
                        _ => {
                            let _one_at_a_time =
                                self.thread_lock.lock().unwrap_or_else(|e| e.into_inner());
                            self.work_out_answered(thread)?
                        }
                    };
                    answered.insert(thread, mine);
                    mine
                }
            };
            if mine == me {
                kept.push(f);
            }
        }
        Ok(kept)
    }

    /// Who answered last, from the thread's latest message that is not a
    /// draft, kept. Only that one is read in full (the rest by when they
    /// arrived), as most threads have a few. The caller holds
    /// `thread_lock`.
    fn work_out_answered(&self, thread: u64) -> StoreResult<bool> {
        let key = self.get(THREAD, thread)?.str("key")?;
        let Some(account_key) = key.split('\u{1f}').next() else {
            return Ok(false);
        };
        let Some(account) = self.accounts_by_key()?.remove(account_key) else {
            return Ok(false);
        };
        let ids = self.links(THREAD, thread, "messages")?;
        let mut by_time: Vec<(i64, u64)> = Vec::with_capacity(ids.len());
        for fields in self.get_many_projected(TYPE, &ids, &["received"])? {
            by_time.push((fields.datetime("received")?, fields.id()));
        }
        by_time.sort_unstable_by(|a, b| b.cmp(a));
        for (at, id) in by_time {
            let message = self.decode_message(&self.get(TYPE, id)?)?;
            if is_draft(&message) {
                continue;
            }
            let mine = answered(&account, &message).is_some();
            self.set_answered(thread, at, mine)?;
            return Ok(mine);
        }
        Ok(false)
    }

    fn set_answered(&self, thread: u64, at: i64, mine: bool) -> StoreResult<()> {
        let fields = FieldsBuilder::new()
            .datetime("last_message", at)
            .bool("replied_last", mine)
            .build();
        self.db_update(THREAD, thread, fields)
    }
}

#[cfg(test)]
mod tests {
    use postrinch_model::{
        Account, EmailAddress, Flags, Label, LabelKind, Mailbox, Message, MessageHeaders, Provider,
        ProviderMessageId, ProviderThreadId, SystemLabel, Timestamp,
    };

    use crate::codec::FieldsBuilder;
    use crate::keys::scoped;
    use crate::messages::THREAD;
    use crate::{Find, Store};

    fn address(s: &str) -> EmailAddress {
        EmailAddress::parse(s).unwrap()
    }

    fn message(id: &str, at: i64, from: &str, labels: &[&str]) -> Message {
        Message {
            account: address("me@example.org"),
            id: ProviderMessageId(id.into()),
            thread: ProviderThreadId("t1".into()),
            headers: MessageHeaders {
                from: vec![Mailbox::new(None, address(from))],
                to: vec![Mailbox::new(None, address("bob@example.org"))],
                in_reply_to: (id != "m1").then(|| "m1@example.org".to_owned()),
                ..Default::default()
            },
            received: Timestamp::from_millis(at),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            flags: Flags::default(),
            snippet: String::new(),
            body_text: String::new(),
            attachments: Vec::new(),
            size: 0,
            downloaded: true,
        }
    }

    fn mine(store: &Store, me: bool) -> usize {
        let found = store.find_messages(&Find::default()).unwrap();
        let threads: std::collections::HashSet<_> = store
            .whose_last(found, me)
            .unwrap()
            .into_iter()
            .map(|f| store.found_thread(&f).unwrap())
            .collect();
        threads.len()
    }

    #[test]
    fn kept_as_messages_come_and_go() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let me = address("me@example.org");
        store
            .put_account(&Account::new(me.clone(), Provider::Google))
            .unwrap();
        let drafts = Label::new("DRAFT", "DRAFT", LabelKind::System(SystemLabel::Drafts));
        store.put_label(&me, &drafts).unwrap();
        store
            .put_message(&message("m1", 100, "bob@example.org", &[]), None)
            .unwrap();
        assert_eq!((mine(&store, true), mine(&store, false)), (0, 1));
        store
            .put_message(&message("m2", 200, "me@example.org", &[]), None)
            .unwrap();
        assert_eq!(mine(&store, true), 1, "my answer is the latest");
        store
            .put_message(&message("m3", 300, "me@example.org", &["DRAFT"]), None)
            .unwrap();
        store
            .put_message(&message("m0", 50, "bob@example.org", &[]), None)
            .unwrap();
        assert_eq!(
            mine(&store, true),
            1,
            "a draft and an older message change nothing"
        );
        store
            .put_message(&message("m4", 400, "bob@example.org", &[]), None)
            .unwrap();
        assert_eq!(mine(&store, true), 0, "Bob answered");
        store
            .remove_message(&me, &ProviderMessageId("m4".into()))
            .unwrap();
        assert_eq!(mine(&store, true), 1, "worked out again");

        // A thread from before threads kept it: worked out when met, and
        // by the fill.
        let thread = store
            .find(THREAD, "key", &scoped(&me, "t1"))
            .unwrap()
            .unwrap();
        let clear = || {
            FieldsBuilder::new()
                .null("replied_last")
                .null("last_message")
                .build()
        };
        store.db_update(THREAD, thread.id(), clear()).unwrap();
        assert_eq!(mine(&store, true), 1);
        store.db_update(THREAD, thread.id(), clear()).unwrap();
        assert_eq!(store.fill_replied_last().unwrap(), 1);
        assert_eq!(store.fill_replied_last().unwrap(), 0);
        assert_eq!(mine(&store, true), 1);
    }
}
