mod common;

use common::{address, message, setup};
use postrinch_model::{Account, Provider, ProviderThreadId, Timestamp};

#[test]
fn label_threads_go_by_latest_activity() {
    let (_dir, store, me) = setup();
    // t1 starts first, but a reply outside the inbox (no labels) is the
    // newest message of all, so t1 comes first.
    store
        .put_message(&message(&me, "m1", "t1", 100, "Rota", "a"), None)
        .unwrap();
    store
        .put_message(&message(&me, "m2", "t2", 200, "Engine", "b"), None)
        .unwrap();
    let mut reply = message(&me, "m3", "t1", 300, "Re: Rota", "c");
    reply.labels.clear();
    store.put_message(&reply, None).unwrap();
    let mut receipt = message(&me, "m4", "t3", 400, "Receipt", "d");
    receipt.labels = vec!["Label_1".into()];
    store.put_message(&receipt, None).unwrap();

    let inbox = store.label_threads(&me, "INBOX").unwrap();
    let threads: Vec<_> = inbox
        .iter()
        .map(|e| (e.thread.0.as_str(), e.latest))
        .collect();
    assert_eq!(
        threads,
        [
            ("t1", Timestamp::from_millis(300)),
            ("t2", Timestamp::from_millis(200))
        ]
    );
    assert_eq!(
        store.label_threads(&me, "Label_1").unwrap()[0].thread,
        ProviderThreadId("t3".into())
    );
    assert!(
        store
            .label_threads(&me, "NO_SUCH_LABEL")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn label_threads_stay_within_the_account() {
    let (_dir, store, me) = setup();
    let other = address("other@example.org");
    store
        .put_account(&Account::new(other.clone(), Provider::Google))
        .unwrap();
    store
        .put_message(&message(&me, "m1", "t1", 100, "Mine", "a"), None)
        .unwrap();
    assert_eq!(store.label_threads(&me, "INBOX").unwrap().len(), 1);
    assert!(store.label_threads(&other, "INBOX").unwrap().is_empty());
}

#[test]
fn counts_unread_messages() {
    let (_dir, store, me) = setup();
    store
        .put_message(&message(&me, "m1", "t1", 100, "One", "a"), None)
        .unwrap();
    let mut read = message(&me, "m2", "t1", 200, "Two", "b");
    read.flags.unread = false;
    store.put_message(&read, None).unwrap();
    assert_eq!(store.unread_count(&me, "INBOX").unwrap(), 1);
    assert_eq!(store.unread_count(&me, "Label_1").unwrap(), 0);
    assert_eq!(store.unread_count(&me, "NO_SUCH_LABEL").unwrap(), 0);
}
