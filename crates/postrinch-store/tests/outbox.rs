mod common;

use common::{message, setup};
use postrinch_model::{
    EmailAddress, Flags, Label, LabelKind, MessageChange, ProviderMessageId, SystemLabel, Timestamp,
};
use postrinch_store::{Store, StoreError};

fn id(s: &str) -> ProviderMessageId {
    ProviderMessageId(s.into())
}

fn stored(store: &Store, me: &EmailAddress, m: &str) -> (Vec<String>, Flags) {
    let message = store.message(me, &id(m)).unwrap().unwrap();
    (message.labels, message.flags)
}

/// A store with one unread message, m1, in the inbox.
fn one_message() -> (tempfile::TempDir, Store, EmailAddress) {
    let (dir, store, me) = setup();
    store
        .put_label(
            &me,
            &Label::new("TRASH", "Trash", LabelKind::System(SystemLabel::Trash)),
        )
        .unwrap();
    store
        .put_message(&message(&me, "m1", "t1", 100, "Rota", "a"), None)
        .unwrap();
    (dir, store, me)
}

#[test]
fn a_change_applies_at_once_and_waits_to_be_sent() {
    let (_dir, store, me) = one_message();
    store
        .change_messages(&me, &[id("m1")], &MessageChange::read())
        .unwrap();
    store
        .change_messages(&me, &[id("m1")], &MessageChange::remove_label("INBOX"))
        .unwrap();

    let (labels, flags) = stored(&store, &me, "m1");
    assert!(labels.is_empty(), "archived: {labels:?}");
    assert!(!flags.unread);

    let outbox = store.outbox(&me).unwrap();
    assert_eq!(outbox.len(), 2);
    assert!(outbox[0].seq < outbox[1].seq, "kept in the order made");
    assert_eq!(outbox[0].change, MessageChange::read());
    assert_eq!(outbox[1].messages, [id("m1")]);
    assert!(outbox.iter().all(|e| e.sent.is_none() && e.attempts == 0));
}

#[test]
fn sync_does_not_undo_a_change_on_its_way_out() {
    let (_dir, store, me) = one_message();
    store
        .change_messages(&me, &[id("m1")], &MessageChange::read())
        .unwrap();

    // Gmail still reports it unread, in either way sync writes it.
    let unread = Flags {
        unread: true,
        starred: false,
    };
    store
        .set_message_labels(&me, &id("m1"), &["INBOX".into()], unread)
        .unwrap();
    assert!(!stored(&store, &me, "m1").1.unread);
    store
        .put_message(&message(&me, "m1", "t1", 100, "Rota", "a"), None)
        .unwrap();
    assert!(!stored(&store, &me, "m1").1.unread);

    // Sent, but not yet seen in a finished sync: still applied.
    let entry = store.outbox(&me).unwrap()[0].id;
    store.outbox_sent(entry).unwrap();
    store
        .set_message_labels(&me, &id("m1"), &["INBOX".into()], unread)
        .unwrap();
    assert!(!stored(&store, &me, "m1").1.unread);

    // A sync that began before it was sent does not forget it.
    assert_eq!(
        store.forget_sent(&me, Timestamp::from_millis(0)).unwrap(),
        0
    );
    // One that began after does; then the provider's word stands.
    assert_eq!(
        store
            .forget_sent(&me, Timestamp::from_millis(i64::MAX))
            .unwrap(),
        1
    );
    store
        .set_message_labels(&me, &id("m1"), &["INBOX".into()], unread)
        .unwrap();
    assert!(stored(&store, &me, "m1").1.unread);
}

#[test]
fn failures_are_counted_and_entries_can_be_dropped() {
    let (_dir, store, me) = one_message();
    store
        .change_messages(&me, &[id("m1")], &MessageChange::starred(true))
        .unwrap();
    let entry = store.outbox(&me).unwrap()[0].id;
    store.outbox_failed(entry, "offline").unwrap();
    store.outbox_failed(entry, "still offline").unwrap();
    let e = &store.outbox(&me).unwrap()[0];
    assert_eq!((e.attempts, e.error.as_deref()), (2, Some("still offline")));
    store.outbox_sent(entry).unwrap();
    assert_eq!(
        store.outbox(&me).unwrap()[0].error,
        None,
        "cleared once sent"
    );
    store.outbox_drop(entry).unwrap();
    assert!(store.outbox(&me).unwrap().is_empty());
}

#[test]
fn unknown_messages_and_labels_are_refused_before_anything_is_recorded() {
    let (_dir, store, me) = one_message();
    let err = store
        .change_messages(&me, &[id("nope")], &MessageChange::read())
        .unwrap_err();
    assert!(matches!(err, StoreError::UnknownMessage { .. }), "{err}");
    let err = store
        .change_messages(&me, &[id("m1")], &MessageChange::add_label("Label_9"))
        .unwrap_err();
    assert!(matches!(err, StoreError::UnknownLabel { .. }), "{err}");
    assert!(store.outbox(&me).unwrap().is_empty());
    // Nothing to do is not recorded either.
    store
        .change_messages(&me, &[id("m1")], &MessageChange::default())
        .unwrap();
    store
        .change_messages(&me, &[], &MessageChange::read())
        .unwrap();
    assert!(store.outbox(&me).unwrap().is_empty());
}

#[test]
fn trashed_mail_leaves_every_other_mailbox() {
    let (_dir, store, me) = one_message();
    let mut receipt = message(&me, "m2", "t2", 200, "Receipt", "b");
    receipt.labels = vec!["INBOX".into(), "Label_1".into()];
    store.put_message(&receipt, None).unwrap();

    store
        .change_messages(&me, &[id("m2")], &MessageChange::add_label("TRASH"))
        .unwrap();

    let inbox = store.label_threads(&me, "INBOX").unwrap();
    assert_eq!(inbox.len(), 1, "only t1 left: {inbox:?}");
    assert!(store.label_threads(&me, "Label_1").unwrap().is_empty());
    assert_eq!(store.label_threads(&me, "TRASH").unwrap().len(), 1);
    let stats = store.label_stats(&me).unwrap();
    assert_eq!((stats["INBOX"].messages, stats["INBOX"].unread), (1, 1));
    assert_eq!(stats["TRASH"].messages, 1);

    // Out of the trash, back where it was.
    store
        .change_messages(&me, &[id("m2")], &MessageChange::remove_label("TRASH"))
        .unwrap();
    assert_eq!(store.label_threads(&me, "INBOX").unwrap().len(), 2);
    assert_eq!(store.label_threads(&me, "Label_1").unwrap().len(), 1);
}
