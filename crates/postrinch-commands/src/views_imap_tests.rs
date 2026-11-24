//! Views over an account without Gmail's categories (an IMAP account):
//! its whole inbox is its Primary, and it has no category tabs.

use postrinch_model::{Account, Label, LabelKind, ListOrder, Provider, SystemLabel};

use super::{address, listed, message, setup};
use crate::{Commands, Folder};

/// The Gmail account of `setup` and an IMAP account beside it, whose
/// labels are its folders: the special ones and one of the person's.
fn with_imap() -> (tempfile::TempDir, Commands) {
    let (dir, commands, _) = setup();
    let store = commands.store.clone();
    let imap = address("me@fastmail.example");
    store
        .put_account(&Account::new(imap.clone(), Provider::Imap))
        .unwrap();
    for system in [
        SystemLabel::Inbox,
        SystemLabel::Sent,
        SystemLabel::Drafts,
        SystemLabel::Trash,
        SystemLabel::Spam,
    ] {
        let label = Label::new(
            system.provider_id(),
            format!("{system:?}"),
            LabelKind::System(system),
        );
        store.put_label(&imap, &label).unwrap();
    }
    let folder = Label::new("f0123456789ab", "Receipts", LabelKind::User);
    store.put_label(&imap, &folder).unwrap();
    store
        .put_message(&message(&imap, "lunch", 400, true, &["INBOX"]), None)
        .unwrap();
    store
        .put_message(&message(&imap, "notes", 250, false, &["INBOX"]), None)
        .unwrap();
    store
        .put_message(
            &message(&imap, "receipt", 350, false, &["f0123456789ab"]),
            None,
        )
        .unwrap();
    (dir, commands)
}

#[test]
fn the_inbox_shows_an_account_without_categories_whole() {
    let (_dir, commands) = with_imap();
    let inbox = crate::views::built_in("inbox").unwrap();
    assert_eq!(inbox.query, "in:inbox category:primary");
    assert_eq!(
        listed(&commands, &inbox.query),
        ["lunch", "dinner", "notes", "acme"],
        "the IMAP inbox whole, beside Gmail's Primary"
    );
    // The same through a search rather than the labels' links.
    assert_eq!(
        listed(&commands, "in:inbox category:primary is:unread"),
        ["lunch", "dinner"]
    );
    assert_eq!(
        listed(&commands, "in:inbox category:updates"),
        ["invoice"],
        "an account without categories has nothing in Updates"
    );
    assert_eq!(listed(&commands, "label:receipts"), ["receipt"]);
}

#[test]
fn an_account_without_categories_has_no_tabs() {
    let (_dir, commands) = with_imap();
    let keys: Vec<String> = commands
        .views()
        .unwrap()
        .into_iter()
        .map(|e| e.view.key)
        .collect();
    assert!(keys.contains(&"inbox:me@fastmail.example".to_owned()));
    assert!(keys.contains(&"updates:me@example.org".to_owned()));
    assert!(
        !keys
            .iter()
            .any(|k| k.ends_with(":me@fastmail.example") && k != "inbox:me@fastmail.example"),
        "no category tabs for the IMAP account: {keys:?}"
    );
}

#[test]
fn listing_primary_lists_an_uncategorised_inbox() {
    let (_dir, commands) = with_imap();
    let inbox = Folder::Label {
        account: address("me@fastmail.example"),
        label: SystemLabel::Inbox.provider_id().into(),
    };
    let page = commands
        .list_threads_in(
            &inbox,
            Some(SystemLabel::CategoryPersonal),
            ListOrder::NewestFirst,
            0,
            50,
        )
        .unwrap();
    let ids: Vec<&str> = page
        .threads
        .iter()
        .map(|r| r.summary.id.0.as_str())
        .collect();
    assert_eq!(ids, ["lunch", "notes"]);
    let updates = commands
        .list_threads(&inbox, Some(SystemLabel::CategoryUpdates), 0, 50)
        .unwrap();
    assert!(updates.threads.is_empty());
}
