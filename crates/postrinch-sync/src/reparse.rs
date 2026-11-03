//! Rebuilding stored messages from their originals, when what Postrinch
//! reads from a message changes.

use postrinch_model::EmailAddress;
use postrinch_store::Store;

use crate::{Envelope, SyncResult, parse_message};

/// Parse every downloaded message of an account again from its stored
/// original, and store the result. What the provider said about each
/// message (thread, labels, flags, when it arrived) is kept; everything
/// read from the message itself is read again, the snippet included, which
/// is made from the body. Returns how many messages were rebuilt.
pub fn reparse(store: &Store, account: &EmailAddress) -> SyncResult<usize> {
    let mut rebuilt = 0;
    for id in store.message_ids(account)? {
        let Some(stored) = store.message(account, &id)? else {
            continue;
        };
        if !stored.downloaded {
            continue;
        }
        let Some(raw) = store.raw_message(account, &id)? else {
            continue;
        };
        let envelope = Envelope {
            account: stored.account,
            id: stored.id,
            thread: stored.thread,
            received: stored.received,
            labels: stored.labels,
            flags: stored.flags,
            snippet: None,
        };
        store.put_message(&parse_message(envelope, &raw)?, None)?;
        rebuilt += 1;
    }
    Ok(rebuilt)
}

#[cfg(test)]
mod tests {
    use postrinch_model::{
        Account, Flags, Label, LabelKind, Provider, ProviderMessageId, ProviderThreadId,
        SystemLabel, Timestamp,
    };

    use super::*;

    #[test]
    fn rebuilds_from_the_original_and_keeps_what_the_provider_said() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let me = EmailAddress::parse("me@example.org").unwrap();
        store
            .put_account(&Account::new(me.clone(), Provider::Google))
            .unwrap();
        let inbox = Label {
            provider_id: "INBOX".into(),
            name: "Inbox".into(),
            kind: LabelKind::System(SystemLabel::Inbox),
        };
        store.put_label(&me, &inbox).unwrap();
        let raw = b"From: ada@example.org\r\nSubject: hi\r\n\r\none\r\ntwo\r\n";
        let envelope = Envelope {
            account: me.clone(),
            id: ProviderMessageId("m1".into()),
            thread: ProviderThreadId("t1".into()),
            received: Timestamp::from_millis(5),
            labels: vec!["INBOX".into()],
            flags: Flags {
                unread: true,
                starred: true,
            },
            snippet: Some("from the provider".into()),
        };
        let mut message = parse_message(envelope, raw).unwrap();
        // As an older parser would have stored it.
        message.body_text = "one\r\ntwo".into();
        store.put_message(&message, Some(raw)).unwrap();

        assert_eq!(reparse(&store, &me).unwrap(), 1);
        let again = store.message(&me, &message.id).unwrap().unwrap();
        assert_eq!(again.body_text, "one\ntwo");
        assert_eq!(again.snippet, "one two");
        assert_eq!(again.labels, ["INBOX"]);
        assert!(again.flags.unread && again.flags.starred);
        assert_eq!(
            store.raw_message(&me, &message.id).unwrap().as_deref(),
            Some(&raw[..])
        );
    }
}
