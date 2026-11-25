use postrinch_model::ProviderMessageId;

use super::*;

const WORK: &str = "f000000000001";
const HOME: &str = "f000000000002";
const ARCHIVE: &str = "f000000000003";
const ALL: &str = "f000000000004";

fn folder(label: &str, name: &str, role: Option<FolderRole>) -> ImapFolder {
    ImapFolder {
        role,
        ..ImapFolder::new(label, name)
    }
}

fn folders(archive: bool) -> Folders {
    let mut stored = vec![
        folder(
            "INBOX",
            "INBOX",
            Some(FolderRole::System(SystemLabel::Inbox)),
        ),
        folder(
            "TRASH",
            "Trash",
            Some(FolderRole::System(SystemLabel::Trash)),
        ),
        folder("SPAM", "Junk", Some(FolderRole::System(SystemLabel::Spam))),
        folder(WORK, "Work", None),
        folder(HOME, "Home", None),
    ];
    if archive {
        stored.push(folder(ARCHIVE, "Archive", Some(FolderRole::Archive)));
    }
    Folders::new(stored, &[])
}

/// Gmail's: All Mail is the archive and holds every message.
fn gmail() -> Folders {
    let stored = vec![
        folder(
            "INBOX",
            "INBOX",
            Some(FolderRole::System(SystemLabel::Inbox)),
        ),
        folder(
            "TRASH",
            "[Gmail]/Trash",
            Some(FolderRole::System(SystemLabel::Trash)),
        ),
        folder(WORK, "Work", None),
        folder(ALL, "[Gmail]/All Mail", Some(FolderRole::Archive)),
    ];
    let listed = [ListedFolder {
        name: "[Gmail]/All Mail".into(),
        delimiter: Some('/'),
        special_use: Some(SpecialUse::All),
        selectable: true,
        may_have_children: true,
    }];
    Folders::new(stored, &listed)
}

fn at(folder: &str, uid: u32) -> ImapPlace {
    ImapPlace {
        folder: folder.into(),
        uidvalidity: 1,
        uid,
        message: ProviderMessageId("m1".into()),
        flags: vec![],
        modseq: 0,
        arrived: None,
    }
}

fn ops(planned: Planned) -> Vec<Op> {
    match planned {
        Planned::Ops(ops) => ops,
        other => panic!("not ops: {other:?}"),
    }
}

fn moved(from: ImapPlace, to: &str) -> Op {
    Op::Move {
        from,
        to: to.into(),
    }
}

fn deleted(place: ImapPlace) -> Op {
    Op::Delete { place }
}

#[test]
fn flags_go_on_every_live_copy() {
    let gone = ImapPlace {
        flags: vec![DELETED.into()],
        ..at(WORK, 9)
    };
    let places = [at("INBOX", 1), at(WORK, 2), gone];
    let got = ops(plan(&places, &MessageChange::read(), &folders(true)));
    assert_eq!(
        got,
        [at("INBOX", 1), at(WORK, 2)].map(|place| Op::Flag {
            place,
            flag: SEEN,
            add: true
        })
    );
    let got = ops(plan(&places[..1], &MessageChange::unread(), &folders(true)));
    assert_eq!(
        got,
        [Op::Flag {
            place: at("INBOX", 1),
            flag: SEEN,
            add: false
        }]
    );
    let got = ops(plan(
        &places[..1],
        &MessageChange::starred(true),
        &folders(true),
    ));
    assert!(matches!(
        &got[..],
        [Op::Flag {
            flag: FLAGGED,
            add: true,
            ..
        }]
    ));
}

#[test]
fn archiving_moves_the_only_copy_and_deletes_one_of_several() {
    let archive = MessageChange::remove_label("INBOX");
    let got = ops(plan(&[at("INBOX", 1)], &archive, &folders(true)));
    assert_eq!(got, [moved(at("INBOX", 1), ARCHIVE)]);
    // Also labelled Work: the inbox copy goes, the Work one stays.
    let got = ops(plan(
        &[at("INBOX", 1), at(WORK, 4)],
        &archive,
        &folders(true),
    ));
    assert_eq!(got, [deleted(at("INBOX", 1))]);
    // No archive folder yet.
    assert_eq!(
        plan(&[at("INBOX", 1)], &archive, &folders(false)),
        Planned::NeedsArchive
    );
}

#[test]
fn labels_are_copies_and_a_label_for_another_is_a_move() {
    let add = MessageChange::add_label(WORK);
    let got = ops(plan(&[at("INBOX", 1)], &add, &folders(true)));
    assert_eq!(
        got,
        [Op::Copy {
            from: at("INBOX", 1),
            to: WORK.into()
        }]
    );
    // Already there: nothing.
    assert!(ops(plan(&[at(WORK, 1)], &add, &folders(true))).is_empty());
    let change = MessageChange {
        add_labels: vec![HOME.into()],
        remove_labels: vec![WORK.into()],
        ..MessageChange::default()
    };
    let got = ops(plan(
        &[at("INBOX", 1), at(WORK, 2)],
        &change,
        &folders(true),
    ));
    assert_eq!(got, [moved(at(WORK, 2), HOME)]);
    // Removing a user label that is the last copy files it in the archive.
    let remove = MessageChange::remove_label(WORK);
    let got = ops(plan(&[at(WORK, 2)], &remove, &folders(true)));
    assert_eq!(got, [moved(at(WORK, 2), ARCHIVE)]);
}

#[test]
fn trash_moves_one_copy_and_deletes_the_rest_and_restoring_comes_back() {
    let trash = MessageChange::add_label("TRASH");
    let places = [at(WORK, 2), at("INBOX", 1)];
    let got = ops(plan(&places, &trash, &folders(true)));
    assert_eq!(got, [moved(at(WORK, 2), "TRASH"), deleted(at("INBOX", 1))]);
    let restore = MessageChange::remove_label("TRASH");
    let got = ops(plan(&[at("TRASH", 7)], &restore, &folders(true)));
    assert_eq!(got, [moved(at("TRASH", 7), "INBOX")]);
    let not_spam = MessageChange {
        add_labels: vec!["INBOX".into()],
        remove_labels: vec!["SPAM".into()],
        ..MessageChange::default()
    };
    let got = ops(plan(&[at("SPAM", 3)], &not_spam, &folders(true)));
    assert_eq!(got, [moved(at("SPAM", 3), "INBOX")]);
    // Restored into the folders the change adds.
    let into = MessageChange {
        add_labels: vec![WORK.into(), HOME.into()],
        remove_labels: vec!["TRASH".into()],
        ..MessageChange::default()
    };
    let got = ops(plan(&[at("TRASH", 7)], &into, &folders(true)));
    assert_eq!(
        got,
        [
            Op::Copy {
                from: at("TRASH", 7),
                to: HOME.into()
            },
            moved(at("TRASH", 7), WORK)
        ]
    );
}

#[test]
fn gmail_all_mail_is_never_deleted_from() {
    let places = [at("INBOX", 1), at(ALL, 50)];
    // Archiving deletes the inbox copy: Gmail removes the label.
    let got = ops(plan(
        &places,
        &MessageChange::remove_label("INBOX"),
        &gmail(),
    ));
    assert_eq!(got, [deleted(at("INBOX", 1))]);
    // Removing All Mail's label does nothing.
    let got = ops(plan(&places, &MessageChange::remove_label(ALL), &gmail()));
    assert!(got.is_empty());
    // Trash moves the inbox copy and leaves All Mail's alone.
    let places = [at(ALL, 50), at("INBOX", 1), at(WORK, 3)];
    let got = ops(plan(&places, &MessageChange::add_label("TRASH"), &gmail()));
    assert_eq!(got, [moved(at("INBOX", 1), "TRASH"), deleted(at(WORK, 3))]);
    // Archived mail is trashed from All Mail.
    let got = ops(plan(
        &[at(ALL, 50)],
        &MessageChange::add_label("TRASH"),
        &gmail(),
    ));
    assert_eq!(got, [moved(at(ALL, 50), "TRASH")]);
}

#[test]
fn what_cannot_be_done_is_refused_and_gone_mail_needs_nothing() {
    let only_archive = [at(ARCHIVE, 1)];
    let refused = plan(
        &only_archive,
        &MessageChange::remove_label(ARCHIVE),
        &folders(true),
    );
    assert!(matches!(refused, Planned::Refused(_)), "{refused:?}");
    let missing = plan(
        &[at("INBOX", 1)],
        &MessageChange::add_label("f0000000000ff"),
        &folders(true),
    );
    assert!(matches!(missing, Planned::Refused(_)), "{missing:?}");
    assert_eq!(
        plan(&[], &MessageChange::remove_label("INBOX"), &folders(true)),
        Planned::Ops(Vec::new())
    );
}

#[test]
fn duplicate_copies_in_a_folder_all_go() {
    let places = [at("INBOX", 1), at("INBOX", 2)];
    let got = ops(plan(
        &places,
        &MessageChange::remove_label("INBOX"),
        &folders(true),
    ));
    assert_eq!(
        got,
        [moved(at("INBOX", 1), ARCHIVE), deleted(at("INBOX", 2))]
    );
}
