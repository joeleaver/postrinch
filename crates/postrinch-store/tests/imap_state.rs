//! What IMAP sync keeps: an account's settings, its folders' state and
//! where each message's copies are.

mod common;

use common::{address, message};
use postrinch_model::{
    Account, EmailAddress, FolderRole, FolderRoles, ImapFolder, ImapPlace, ImapSettings, Label,
    LabelKind, Provider, ProviderMessageId, Security, Server, SignInMethod, SystemLabel,
};
use postrinch_store::{Store, StoreError};

fn server(host: &str, port: u16, security: Security) -> Server {
    Server {
        host: host.into(),
        port,
        security,
        username: "me@example.org".into(),
    }
}

fn settings() -> ImapSettings {
    ImapSettings {
        imap: server("imap.example.org", 993, Security::Tls),
        smtp: server("smtp.example.org", 587, Security::StartTls),
        sign_in: SignInMethod::Password,
        roles: FolderRoles {
            junk: Some("Spam".into()),
            ..FolderRoles::default()
        },
        server_files_sent: true,
    }
}

/// The common account made an IMAP one, with its inbox and one folder of
/// the person's.
fn setup() -> (tempfile::TempDir, Store, EmailAddress) {
    let (dir, store, me) = common::setup();
    let mut account = Account::new(me.clone(), Provider::Imap);
    account.imap = Some(settings());
    store.put_account(&account).unwrap();
    let mut inbox = ImapFolder::new(SystemLabel::Inbox.provider_id(), "INBOX");
    inbox.delimiter = Some('/');
    inbox.role = Some(FolderRole::System(SystemLabel::Inbox));
    inbox.uidvalidity = 7;
    inbox.uidnext = 12;
    inbox.mailbox_id = Some("F2212ea87".into());
    store.put_imap_folder(&me, &inbox).unwrap();
    let mut receipts = ImapFolder::new("f0123456789ab", "Receipts");
    receipts.delimiter = Some('.');
    receipts.uidvalidity = 3;
    store.put_imap_folder(&me, &receipts).unwrap();
    (dir, store, me)
}

fn place(folder: &str, uidvalidity: u32, uid: u32, message: &str) -> ImapPlace {
    ImapPlace {
        folder: folder.into(),
        uidvalidity,
        uid,
        message: ProviderMessageId(message.into()),
        flags: vec!["\\Seen".into()],
        modseq: 0,
        arrived: None,
    }
}

fn uids(store: &Store, me: &EmailAddress, folder: &str) -> Vec<u32> {
    store
        .imap_folder_places(me, folder)
        .unwrap()
        .iter()
        .map(|p| p.uid)
        .collect()
}

#[test]
fn settings_are_kept_with_the_account() {
    let (dir, store, me) = setup();
    let kept = store.account(&me).unwrap().unwrap();
    assert_eq!(kept.imap, Some(settings()));
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.account(&me).unwrap().unwrap().imap, Some(settings()));
    // A Google account has none.
    let google = address("bob@example.org");
    store
        .put_account(&Account::new(google.clone(), Provider::Google))
        .unwrap();
    assert_eq!(store.account(&google).unwrap().unwrap().imap, None);
}

#[test]
fn folders_are_kept_and_changed() {
    let (_dir, store, me) = setup();
    let folders = store.imap_folders(&me).unwrap();
    let labels: Vec<&str> = folders.iter().map(|f| f.label_id.as_str()).collect();
    assert_eq!(labels, ["INBOX", "f0123456789ab"]);
    assert_eq!(folders[0].delimiter, Some('/'));
    assert_eq!(folders[0].mailbox_id.as_deref(), Some("F2212ea87"));
    assert_eq!(folders[1].mailbox_id, None);
    assert_eq!(
        folders[0].role,
        Some(FolderRole::System(SystemLabel::Inbox))
    );

    let mut inbox = folders[0].clone();
    inbox.uidnext = 40;
    inbox.highest_modseq = 9_000_000_000;
    store.put_imap_folder(&me, &inbox).unwrap();
    assert_eq!(store.imap_folder(&me, "INBOX").unwrap(), Some(inbox));

    store.remove_imap_folder(&me, "f0123456789ab").unwrap();
    assert_eq!(store.imap_folder(&me, "f0123456789ab").unwrap(), None);
    assert_eq!(store.imap_folders(&me).unwrap().len(), 1);
}

#[test]
fn places_are_found_by_message_and_by_folder() {
    let (_dir, store, me) = setup();
    // One message in two folders, as mail sent to oneself.
    store
        .put_imap_place(&me, &place("INBOX", 7, 10, "m1"))
        .unwrap();
    store
        .put_imap_place(&me, &place("INBOX", 7, 3, "m2"))
        .unwrap();
    store
        .put_imap_place(&me, &place("f0123456789ab", 3, 5, "m1"))
        .unwrap();

    let mut of_m1: Vec<(String, u32)> = store
        .imap_message_places(&me, &ProviderMessageId("m1".into()))
        .unwrap()
        .into_iter()
        .map(|p| (p.folder, p.uid))
        .collect();
    of_m1.sort();
    assert_eq!(
        of_m1,
        [("INBOX".to_owned(), 10), ("f0123456789ab".to_owned(), 5)]
    );
    assert_eq!(uids(&store, &me, "INBOX"), [3, 10]);
    assert_eq!(store.imap_highest_uid(&me, "INBOX").unwrap(), Some(10));
    assert_eq!(store.imap_highest_uid(&me, "nowhere").unwrap(), None);

    // Putting a place again changes its flags; it is still one place.
    let mut flagged = place("INBOX", 7, 10, "m1");
    flagged.flags.push("\\Flagged".into());
    flagged.modseq = 77;
    store.put_imap_place(&me, &flagged).unwrap();
    let places = store.imap_folder_places(&me, "INBOX").unwrap();
    assert_eq!(places.len(), 2);
    assert_eq!(places[1], flagged);

    store.remove_imap_place(&me, "INBOX", 7, 10).unwrap();
    assert_eq!(uids(&store, &me, "INBOX"), [3]);
    assert_eq!(
        store
            .imap_message_places(&me, &ProviderMessageId("m1".into()))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_place_needs_its_folder() {
    let (_dir, store, me) = setup();
    let err = store
        .put_imap_place(&me, &place("fnowhere", 1, 1, "m1"))
        .unwrap_err();
    assert!(matches!(err, StoreError::UnknownImapFolder { .. }), "{err}");
}

#[test]
fn places_are_per_account() {
    let (_dir, store, me) = setup();
    let other = address("bob@example.org");
    store
        .put_account(&Account::new(other.clone(), Provider::Imap))
        .unwrap();
    store
        .put_imap_folder(&other, &ImapFolder::new("INBOX", "INBOX"))
        .unwrap();
    store
        .put_imap_place(&me, &place("INBOX", 7, 1, "m1"))
        .unwrap();
    store
        .put_imap_place(&other, &place("INBOX", 1, 1, "m1"))
        .unwrap();
    let message = ProviderMessageId("m1".into());
    let mine = store.imap_message_places(&me, &message).unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].uidvalidity, 7);
    assert_eq!(uids(&store, &other, "INBOX"), [1]);
}

#[test]
fn a_new_uidvalidity_forgets_the_folders_places() {
    let (_dir, store, me) = setup();
    for uid in 1..=3 {
        store
            .put_imap_place(&me, &place("INBOX", 7, uid, &format!("m{uid}")))
            .unwrap();
    }
    store
        .put_imap_place(&me, &place("f0123456789ab", 3, 1, "m1"))
        .unwrap();
    assert_eq!(store.clear_imap_folder(&me, "INBOX").unwrap(), 3);
    assert!(uids(&store, &me, "INBOX").is_empty());
    assert_eq!(uids(&store, &me, "f0123456789ab"), [1]);
    assert!(store.imap_folder(&me, "INBOX").unwrap().is_some());
}

#[test]
fn clearing_mail_starts_every_folder_again() {
    let (_dir, store, me) = setup();
    let mut m = message(&me, "m1", "t1", 100, "Hello", "hi");
    m.labels = vec!["INBOX".into()];
    store.put_message(&m, None).unwrap();
    store
        .put_imap_place(&me, &place("INBOX", 7, 10, "m1"))
        .unwrap();
    store
        .put_imap_place(&me, &place("f0123456789ab", 3, 4, "m1"))
        .unwrap();

    assert_eq!(store.clear_mail(&me).unwrap(), 1);
    for (label_id, name) in [("INBOX", "INBOX"), ("f0123456789ab", "Receipts")] {
        let folder = store.imap_folder(&me, label_id).unwrap().unwrap();
        assert_eq!(
            (folder.uidvalidity, folder.uidnext, folder.highest_modseq),
            (0, 0, 0)
        );
        assert_eq!(folder.name, name, "the folder and its label id stay");
        assert!(uids(&store, &me, label_id).is_empty());
    }
}

#[test]
fn removing_a_folders_label_removes_the_folder_and_its_places() {
    let (_dir, store, me) = setup();
    store
        .put_label(
            &me,
            &Label::new("f0123456789ab", "Receipts", LabelKind::User),
        )
        .unwrap();
    store
        .put_imap_place(&me, &place("f0123456789ab", 3, 1, "m1"))
        .unwrap();
    store
        .put_imap_place(&me, &place("INBOX", 7, 10, "m1"))
        .unwrap();

    store.remove_label(&me, "f0123456789ab").unwrap();
    assert!(store.imap_folder(&me, "f0123456789ab").unwrap().is_none());
    let left = store
        .imap_message_places(&me, &ProviderMessageId("m1".into()))
        .unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].folder, "INBOX");
    assert!(store.imap_folder(&me, "INBOX").unwrap().is_some());
}

#[test]
fn removing_the_account_removes_its_folders_and_places() {
    let (dir, store, me) = setup();
    store
        .put_imap_place(&me, &place("INBOX", 7, 10, "m1"))
        .unwrap();
    store.remove_account(&me).unwrap();
    // Added again, it starts with nothing.
    store
        .put_account(&Account::new(me.clone(), Provider::Imap))
        .unwrap();
    assert!(store.imap_folders(&me).unwrap().is_empty());
    assert!(
        store
            .imap_message_places(&me, &ProviderMessageId("m1".into()))
            .unwrap()
            .is_empty()
    );
    drop(store);
    // And the store opens again as it was.
    Store::open(dir.path()).unwrap();
}
