use std::time::Duration;

mod common;

use common::{address, message, setup};
use postrinch_model::{
    Account, Flags, Label, LabelKind, Message, Provider, ProviderMessageId, SystemLabel,
};
use postrinch_store::{Store, StoreError};

#[test]
fn put_and_read_back() {
    let (_dir, store, me) = setup();
    let m = message(
        &me,
        "m1",
        "t1",
        1_000,
        "Engine notes",
        "the analytical engine weaves algebraic patterns",
    );
    store.put_message(&m, Some(b"raw bytes")).unwrap();

    assert_eq!(store.message(&me, &m.id).unwrap(), Some(m.clone()));
    assert_eq!(
        store.raw_message(&me, &m.id).unwrap().as_deref(),
        Some(&b"raw bytes"[..])
    );
}

#[test]
fn put_again_updates_and_keeps_raw() {
    let (_dir, store, me) = setup();
    let mut m = message(&me, "m1", "t1", 1_000, "Engine notes", "body");
    store.put_message(&m, Some(b"raw bytes")).unwrap();

    m.flags.unread = false;
    m.labels = vec!["INBOX".into(), "Label_1".into()];
    store.put_message(&m, None).unwrap();

    assert_eq!(store.message(&me, &m.id).unwrap(), Some(m.clone()));
    assert_eq!(
        store.raw_message(&me, &m.id).unwrap().as_deref(),
        Some(&b"raw bytes"[..])
    );
}

#[test]
fn labels_and_flags_change_in_place() {
    let (_dir, store, me) = setup();
    let m = message(&me, "m1", "t1", 1_000, "s", "b");
    store.put_message(&m, None).unwrap();

    let flags = Flags {
        unread: false,
        starred: true,
    };
    store
        .set_message_labels(&me, &m.id, &["Label_1".into()], flags)
        .unwrap();

    let stored = store.message(&me, &m.id).unwrap().unwrap();
    assert_eq!(stored.labels, ["Label_1"]);
    assert_eq!(stored.flags, flags);
}

#[test]
fn unknown_label_is_refused() {
    let (_dir, store, me) = setup();
    let mut m = message(&me, "m1", "t1", 1_000, "s", "b");
    m.labels = vec!["Label_404".into()];
    assert!(matches!(
        store.put_message(&m, None),
        Err(StoreError::UnknownLabel { .. })
    ));
}

#[test]
fn threads_collect_their_messages_and_go_with_the_last() {
    let (_dir, store, me) = setup();
    let reply = message(&me, "m2", "t1", 2_000, "Re: plans", "sounds good");
    let first = message(&me, "m1", "t1", 1_000, "plans", "shall we?");
    let other = message(&me, "m3", "t2", 1_500, "other", "unrelated");
    for m in [&reply, &first, &other] {
        store.put_message(m, None).unwrap();
    }

    let thread = store.thread_messages(&me, &first.thread).unwrap();
    assert_eq!(thread, vec![first.clone(), reply.clone()]);

    store.remove_message(&me, &first.id).unwrap();
    store.remove_message(&me, &first.id).unwrap();
    assert_eq!(
        store.thread_messages(&me, &first.thread).unwrap(),
        vec![reply.clone()]
    );

    store.remove_message(&me, &reply.id).unwrap();
    assert!(
        store
            .thread_messages(&me, &first.thread)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.thread_messages(&me, &other.thread).unwrap(),
        vec![other]
    );
}

#[test]
fn search_matches_subject_people_and_body() {
    let (_dir, store, me) = setup();
    let engine = message(
        &me,
        "m1",
        "t1",
        1_000,
        "Engine notes",
        "the analytical engine weaves algebraic patterns",
    );
    let lunch = message(
        &me,
        "m2",
        "t2",
        2_000,
        "Lunch",
        "shall we get noodles on Friday",
    );
    store.put_message(&engine, None).unwrap();
    store.put_message(&lunch, None).unwrap();

    let hits = store.search("weaving patterns", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].message.id, engine.id);

    let by_name = store.search("lovelace", 10).unwrap();
    assert_eq!(by_name.len(), 2);

    assert_eq!(store.search("noodles", 10).unwrap()[0].message.id, lunch.id);
}

#[test]
fn removing_an_account_removes_its_mail_and_garbage_collects_raw() {
    let (_dir, store, me) = setup();
    let m = message(&me, "m1", "t1", 1_000, "s", "b");
    store.put_message(&m, Some(b"raw bytes")).unwrap();

    assert_eq!(store.collect_garbage(Duration::ZERO).unwrap(), 0);
    store.remove_account(&me).unwrap();
    assert_eq!(store.account(&me).unwrap(), None);
    assert!(store.search("b", 10).unwrap().is_empty());
    assert_eq!(store.collect_garbage(Duration::ZERO).unwrap(), 1);
}

#[test]
fn survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let me = address("me@example.org");
    let m = message(&me, "m1", "t1", 1_000, "s", "persisted body");
    {
        let store = Store::open(dir.path()).unwrap();
        store
            .put_account(&Account::new(me.clone(), Provider::Google))
            .unwrap();
        store
            .put_label(
                &me,
                &Label {
                    provider_id: "INBOX".into(),
                    name: "Inbox".into(),
                    kind: LabelKind::System(SystemLabel::Inbox),
                },
            )
            .unwrap();
        store.put_message(&m, None).unwrap();
    }
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.message(&me, &m.id).unwrap(), Some(m));
    assert_eq!(store.search("persisted", 10).unwrap().len(), 1);
}

#[test]
fn lists_an_accounts_message_ids_only() {
    let (_dir, store, me) = setup();
    let other = address("other@example.org");
    store
        .put_account(&Account::new(other.clone(), Provider::Google))
        .unwrap();
    store
        .put_label(
            &other,
            &Label {
                provider_id: "INBOX".into(),
                name: "Inbox".into(),
                kind: LabelKind::System(SystemLabel::Inbox),
            },
        )
        .unwrap();
    for (account, id) in [(&me, "m2"), (&me, "m1"), (&other, "m9")] {
        store
            .put_message(&message(account, id, "t", 1, "s", "b"), None)
            .unwrap();
    }

    let ids = store.message_ids(&me).unwrap();
    assert_eq!(
        ids,
        [
            ProviderMessageId("m1".into()),
            ProviderMessageId("m2".into())
        ]
    );
    assert!(store.has_message(&me, &ids[0]).unwrap());
    assert!(!store.has_message(&other, &ids[0]).unwrap());
}

#[test]
fn clear_mail_keeps_the_account_and_labels() {
    let (_dir, store, me) = setup();
    for id in ["m1", "m2"] {
        store
            .put_message(&message(&me, id, "t", 1, "s", "b"), Some(id.as_bytes()))
            .unwrap();
    }
    store.set_sync_cursor(&me, Some("cursor")).unwrap();

    assert_eq!(store.clear_mail(&me).unwrap(), 2);
    assert!(store.message_ids(&me).unwrap().is_empty());
    assert_eq!(store.sync_cursor(&me).unwrap(), None);
    assert!(store.account(&me).unwrap().is_some());
    assert_eq!(store.labels(&me).unwrap().len(), 2);
    assert_eq!(store.collect_garbage(Duration::ZERO).unwrap(), 2);
}

fn headers_only(m: &Message) -> Message {
    Message {
        body_text: String::new(),
        attachments: Vec::new(),
        downloaded: false,
        ..m.clone()
    }
}

#[test]
fn headers_only_messages_are_listed_newest_first_for_download() {
    let (_dir, store, me) = setup();
    let old = headers_only(&message(&me, "old", "t1", 1_000, "old", "b"));
    let new = headers_only(&message(&me, "new", "t2", 3_000, "new", "b"));
    let done = message(&me, "done", "t3", 2_000, "done", "b");
    for m in [&old, &new, &done] {
        store.put_message(m, None).unwrap();
    }
    assert_eq!(
        store.undownloaded(&me).unwrap(),
        [new.id.clone(), old.id.clone()]
    );
    assert_eq!(store.message(&me, &old.id).unwrap(), Some(old.clone()));

    // Downloading it replaces the headers-only copy.
    let full = message(&me, "old", "t1", 1_000, "old", "now with a body");
    store.put_message(&full, Some(b"raw")).unwrap();
    assert_eq!(store.undownloaded(&me).unwrap(), [new.id]);
    assert_eq!(store.search("body", 5).unwrap().len(), 1);
}

#[test]
fn headers_never_overwrite_a_downloaded_body() {
    let (_dir, store, me) = setup();
    let full = message(&me, "m1", "t1", 1_000, "s", "the body");
    store.put_message(&full, Some(b"raw")).unwrap();

    let mut later = headers_only(&full);
    later.flags.unread = false;
    store.put_message(&later, None).unwrap();

    let stored = store.message(&me, &full.id).unwrap().unwrap();
    assert!(stored.downloaded);
    assert_eq!(stored.body_text, "the body");
    assert!(!stored.flags.unread);
}
