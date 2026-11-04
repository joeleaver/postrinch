//! Labels whose ids start another's (`Label_1`, `Label_10`) stay one
//! label each across reopening the store (joeleaver/rhypedb#27).

mod common;

use common::{message, setup};
use postrinch_model::{Label, LabelKind};
use postrinch_store::Store;

/// Reopen, waiting out a lock the engine may still hold (rhypedb#29).
fn reopen(dir: &std::path::Path) -> Store {
    for _ in 0..50 {
        if let Ok(store) = Store::open(dir) {
            return store;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Store::open(dir).unwrap()
}

#[test]
fn a_label_is_not_copied_after_a_reopen() {
    let (dir, store, me) = setup();
    store
        .put_label(&me, &Label::new("Label_10", "Travel", LabelKind::User))
        .unwrap();
    let mut m = message(&me, "m1", "t1", 100, "Receipt", "thanks");
    m.labels = vec!["INBOX".into(), "Label_1".into()];
    store.put_message(&m, None).unwrap();
    drop(store);

    let store = reopen(dir.path());
    // Sync puts every label again, then stores messages carrying them.
    store
        .put_label(&me, &Label::new("Label_1", "Receipts", LabelKind::User))
        .unwrap();
    let mut m2 = message(&me, "m2", "t2", 200, "Another", "more");
    m2.labels = vec!["Label_1".into()];
    store.put_message(&m2, None).unwrap();

    let ids: Vec<String> = store
        .labels(&me)
        .unwrap()
        .into_iter()
        .map(|l| l.provider_id)
        .collect();
    assert_eq!(ids.len(), 3, "{ids:?}");
    let (_, copies) = store.duplicate_keys("Label", "key").unwrap();
    assert!(copies.is_empty(), "{copies:?}");
    let receipts = store.label_threads(&me, "Label_1").unwrap();
    assert_eq!(receipts.len(), 2, "both messages are under the one label");
}
