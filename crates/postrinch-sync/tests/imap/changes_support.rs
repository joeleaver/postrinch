//! What the tests of changing mail share: making a change as the window
//! does and sending it, reading what the server has, and checking that
//! the next sync agrees with the store without downloading anything again.

use postrinch_model::{MessageChange, ProviderMessageId};
use postrinch_sync::google::SendReport;

use super::sync_support::{Fixture, header_fetches, mail};

/// Makes `change` to the messages with these `Message-ID`s, as one
/// outbox entry, and sends the outbox.
pub async fn change(f: &Fixture, headers: &[&str], change: MessageChange) -> SendReport {
    record(f, headers, change);
    f.sync.send_outbox(&f.me).await.unwrap()
}

/// Makes `change` as [`change`] does, without sending it.
pub fn record(f: &Fixture, headers: &[&str], change: MessageChange) {
    let ids: Vec<ProviderMessageId> = headers.iter().map(|h| f.id(h)).collect();
    f.store.change_messages(&f.me, &ids, &change).unwrap();
}

/// Appends a message with this `Message-ID` to each folder named.
pub fn put(f: &Fixture, header: &str, folders: &[&str]) {
    for folder in folders {
        f.user
            .append(folder, mail(Some(header), header, ""))
            .unwrap();
    }
}

/// How many messages a folder on the server holds.
pub fn count(f: &Fixture, folder: &str) -> usize {
    f.user.messages(folder).unwrap().len()
}

/// Whether every message in the folder has `flag`.
pub fn all_have(f: &Fixture, folder: &str, flag: &str) -> bool {
    let messages = f.user.messages(folder).unwrap();
    !messages.is_empty() && messages.iter().all(|m| m.has_flag(flag))
}

/// Every stored message's labels and flags, in id order.
fn snapshot(f: &Fixture) -> Vec<String> {
    let mut ids = f.store.message_ids(&f.me).unwrap();
    ids.sort_by(|a, b| a.0.cmp(&b.0));
    ids.iter()
        .map(|id| {
            let m = f.store.message(&f.me, id).unwrap().unwrap();
            let mut labels = m.labels.clone();
            labels.sort();
            format!("{} {labels:?} {:?}", id.0, m.flags)
        })
        .collect()
}

/// Syncs, and checks the pass read back what the store already had: the
/// same messages with the same labels and flags, no headers downloaded
/// again, and nothing left in the outbox.
pub async fn sync_agrees(f: &Fixture) {
    let before = snapshot(f);
    let from = f.server.commands().len();
    f.sync().await;
    assert_eq!(snapshot(f), before, "the pass agrees with the store");
    let fetched = header_fetches(&f.server.commands()[from..]);
    assert!(fetched.is_empty(), "no headers fetched again: {fetched:?}");
    assert!(
        f.store.outbox(&f.me).unwrap().is_empty(),
        "the outbox is empty"
    );
}
