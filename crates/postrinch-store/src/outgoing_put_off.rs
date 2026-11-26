//! Recipients an SMTP server put off (IMAP accounts; docs/imap.md,
//! "Writing and sending"): a 4xx at RCPT for one recipient, as when their
//! mailbox is full. The message goes to the others, and to them on a
//! later attempt; how often and since when each was put off is kept with
//! the message, so sync can stop trying after a while and leave it to the
//! person.

use postrinch_model::{EmailAddress, Timestamp};
use serde::{Deserialize, Serialize};

use crate::codec::FieldsBuilder;
use crate::outgoing::{TYPE, decode};
use crate::{Store, StoreResult};

/// A recipient the server put off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutOff {
    pub address: EmailAddress,
    /// The server's words, the last time.
    pub words: String,
    /// How many attempts it was put off in.
    pub times: u32,
    /// When it was first put off.
    pub since: Timestamp,
}

impl Store {
    /// An attempt at `now` had the server put off each of `put_off` (an
    /// address and the server's words): counts it for each, and returns
    /// how each of them stands now. Recipients put off before and not in
    /// `put_off` are kept as they were.
    pub fn outgoing_put_off(
        &self,
        id: u64,
        put_off: &[(EmailAddress, String)],
        now: Timestamp,
    ) -> StoreResult<Vec<PutOff>> {
        let mut all = decode(&self.get(TYPE, id)?)?.put_off;
        let mut now_put_off = Vec::with_capacity(put_off.len());
        for (address, words) in put_off {
            let at = all.iter().position(|p| p.address.key() == address.key());
            let entry = match at {
                Some(at) => {
                    let entry = &mut all[at];
                    entry.times = entry.times.saturating_add(1);
                    entry.words.clone_from(words);
                    entry.clone()
                }
                None => {
                    let entry = PutOff {
                        address: address.clone(),
                        words: words.clone(),
                        times: 1,
                        since: now,
                    };
                    all.push(entry.clone());
                    entry
                }
            };
            now_put_off.push(entry);
        }
        let fields = FieldsBuilder::new().json("put_off", &all);
        self.db_update(TYPE, id, fields.build())?;
        Ok(now_put_off)
    }
}

#[cfg(test)]
mod tests {
    use postrinch_model::{Account, Draft, Provider};

    use super::*;
    use crate::OutgoingKind;

    #[test]
    fn each_recipient_is_counted_from_when_it_was_first_put_off() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let me = EmailAddress::parse("me@example.org").unwrap();
        store
            .put_account(&Account::new(me.clone(), Provider::Imap))
            .unwrap();
        let id = store
            .queue_outgoing(&Draft::new(me.clone()), OutgoingKind::Send)
            .unwrap();
        let bob = EmailAddress::parse("bob@example.net").unwrap();
        let carol = EmailAddress::parse("carol@example.net").unwrap();
        let (one, two) = (Timestamp::from_millis(1_000), Timestamp::from_millis(2_000));

        let first = store
            .outgoing_put_off(id, &[(bob.clone(), "Mailbox full".into())], one)
            .unwrap();
        assert_eq!((first[0].times, first[0].since), (1, one));
        let put_off = [
            (
                EmailAddress::parse("BOB@example.net").unwrap(),
                "Still full".into(),
            ),
            (carol.clone(), "Try later".into()),
        ];
        let second = store.outgoing_put_off(id, &put_off, two).unwrap();
        assert_eq!(
            second
                .iter()
                .map(|p| (p.address.key(), p.words.as_str(), p.times, p.since))
                .collect::<Vec<_>>(),
            [
                (bob.key(), "Still full", 2, one),
                (carol.key(), "Try later", 1, two)
            ]
        );
        let kept = &store.outgoing_for(&me).unwrap()[0];
        assert_eq!(kept.put_off, second);

        // Sent again by the person: tried afresh.
        store.outgoing_send_again(id).unwrap();
        assert!(store.outgoing_for(&me).unwrap()[0].put_off.is_empty());
    }
}
