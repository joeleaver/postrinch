//! The folders a pass plans: roles, ids kept, and Gmail's All Mail.

use super::*;

fn folder(name: &str, special_use: Option<SpecialUse>) -> ListedFolder {
    ListedFolder {
        name: name.into(),
        delimiter: Some('/'),
        special_use,
        selectable: true,
        may_have_children: true,
    }
}

fn ids(plan: &[Planned]) -> Vec<(&str, &str)> {
    plan.iter()
        .map(|p| (p.state.name.as_str(), p.state.label_id.as_str()))
        .collect()
}

#[test]
fn special_use_then_names_and_order() {
    let listed = [
        folder("Work", None),
        folder("Trash", Some(SpecialUse::Trash)),
        folder("Sent Items", None),
        folder("INBOX", None),
        folder("Junk E-mail", None),
        folder("Archive", None),
    ];
    let plan = plan(
        &listed,
        &[],
        &FolderRoles::default(),
        &Renames::new(),
        false,
    );
    let got = ids(&plan);
    assert_eq!(got[0], ("INBOX", "INBOX"));
    assert_eq!(got[1], ("Sent Items", "SENT"));
    assert_eq!(got[2].0, "Work");
    assert!(got[2].1.starts_with('f'));
    assert_eq!(got[3].0, "Archive");
    assert_eq!(plan[3].state.role, Some(FolderRole::Archive));
    assert_eq!(got[4], ("Trash", "TRASH"));
    assert_eq!(got[5], ("Junk E-mail", "SPAM"));
}

#[test]
fn the_persons_choice_wins() {
    let listed = [
        folder("INBOX", None),
        folder("Trash", Some(SpecialUse::Trash)),
        folder("Bin", None),
    ];
    let roles = FolderRoles {
        trash: Some("Bin".into()),
        ..FolderRoles::default()
    };
    let plan = plan(&listed, &[], &roles, &Renames::new(), false);
    let bin = plan.iter().find(|p| p.state.name == "Bin").unwrap();
    assert_eq!(bin.state.label_id, "TRASH");
    let trash = plan.iter().find(|p| p.state.name == "Trash").unwrap();
    assert_eq!(trash.state.role, None);
    assert_eq!(trash.label.as_ref().unwrap().kind, LabelKind::User);
}

#[test]
fn gmail_all_mail_is_the_archive_and_virtual_folders_are_skipped() {
    let listed = [
        folder("INBOX", None),
        ListedFolder {
            selectable: false,
            ..folder("[Gmail]", None)
        },
        folder("[Gmail]/All Mail", Some(SpecialUse::All)),
        folder("[Gmail]/Starred", Some(SpecialUse::Flagged)),
        folder("[Gmail]/Important", Some(SpecialUse::Important)),
        folder("[Gmail]/Sent Mail", Some(SpecialUse::Sent)),
    ];
    let plan = plan(&listed, &[], &FolderRoles::default(), &Renames::new(), true);
    let names: Vec<_> = plan.iter().map(|p| p.state.name.as_str()).collect();
    assert_eq!(names, ["INBOX", "[Gmail]/Sent Mail", "[Gmail]/All Mail"]);
    assert_eq!(plan[2].state.role, Some(FolderRole::Archive));
    assert!(plan[2].state.label_id.starts_with('f'));
    // Synced, but no label: every message would carry it.
    assert_eq!(plan[2].label, None);
    assert!(plan[..2].iter().all(|p| p.label.is_some()));
}

#[test]
fn gmail_all_mail_kept_as_a_label_before_is_one_no_longer() {
    // A pass before this fix kept All Mail as an ordinary label.
    let mut all = ImapFolder::new("f0123456789ab", "[Gmail]/All Mail");
    all.role = Some(FolderRole::Archive);
    all.uidvalidity = 11;
    all.uidnext = 900;
    let listed = [
        folder("INBOX", None),
        folder("[Gmail]/All Mail", Some(SpecialUse::All)),
    ];
    let plan = plan(
        &listed,
        &[all],
        &FolderRoles::default(),
        &Renames::new(),
        true,
    );
    let kept = plan
        .iter()
        .find(|p| p.state.name == "[Gmail]/All Mail")
        .unwrap();
    // Its id, place and copies stay: it is not read over again.
    assert_eq!(kept.state.label_id, "f0123456789ab");
    assert_eq!(kept.state.uidnext, 900);
    assert!(!kept.forget_places);
    assert_eq!(kept.label, None);
}

#[test]
fn gmail_all_mail_is_no_label_whichever_folder_archives() {
    let listed = [
        folder("INBOX", None),
        folder("[Gmail]/All Mail", Some(SpecialUse::All)),
        folder("Filed", None),
    ];
    let roles = FolderRoles {
        archive: Some("Filed".into()),
        ..FolderRoles::default()
    };
    let plan = plan(&listed, &[], &roles, &Renames::new(), true);
    let all = plan
        .iter()
        .find(|p| p.state.name == "[Gmail]/All Mail")
        .unwrap();
    assert_eq!(all.state.role, None);
    assert_eq!(all.label, None);
    let filed = plan.iter().find(|p| p.state.name == "Filed").unwrap();
    assert_eq!(filed.state.role, Some(FolderRole::Archive));
    assert!(filed.label.is_some(), "a real archive folder is a label");
}

#[test]
fn all_elsewhere_is_not_gmails_all_mail() {
    let all = folder("virtual/All", Some(SpecialUse::All));
    assert!(holds_everything(&all, true));
    assert!(!holds_everything(&all, false));
    assert!(view_of_all(&all, false));
}

#[test]
fn kept_folders_keep_their_ids_and_state() {
    let mut work = ImapFolder::new("f0123456789ab", "Work");
    work.uidvalidity = 7;
    work.uidnext = 30;
    let listed = [folder("INBOX", None), folder("Work", None)];
    let plan = plan(
        &listed,
        &[work.clone()],
        &FolderRoles::default(),
        &Renames::new(),
        false,
    );
    let kept = plan.iter().find(|p| p.state.name == "Work").unwrap();
    assert_eq!(kept.state.uidnext, 30);
    assert_eq!(kept.state.label_id, "f0123456789ab");
    assert!(!kept.forget_places);
}

#[test]
fn a_role_moving_to_another_folder_forgets_the_old_places() {
    let mut sent = ImapFolder::new("SENT", "Sent");
    sent.uidvalidity = 3;
    let listed = [
        folder("INBOX", None),
        folder("Sent Items", Some(SpecialUse::Sent)),
    ];
    let plan = plan(
        &listed,
        &[sent],
        &FolderRoles::default(),
        &Renames::new(),
        false,
    );
    let new = plan.iter().find(|p| p.state.name == "Sent Items").unwrap();
    assert_eq!(new.state.label_id, "SENT");
    assert_eq!(new.state.uidvalidity, 0);
    assert!(new.forget_places);
}

#[test]
fn a_renamed_folder_keeps_its_id_state_and_places() {
    let mut work = ImapFolder::new("f0123456789ab", "Work");
    work.uidvalidity = 7;
    work.uidnext = 30;
    let listed = [folder("INBOX", None), folder("Projects", None)];
    let renames = Renames::from([("Projects".to_owned(), work.label_id.clone())]);
    let plan = plan(&listed, &[work], &FolderRoles::default(), &renames, false);
    let renamed = plan.iter().find(|p| p.state.name == "Projects").unwrap();
    assert_eq!(renamed.state.label_id, "f0123456789ab");
    assert_eq!(renamed.label.as_ref().unwrap().name, "Projects");
    assert_eq!(renamed.state.uidnext, 30);
    assert!(!renamed.forget_places);
}

#[test]
fn names_are_decoded_and_levels_shown_with_slashes() {
    let listed = [ListedFolder {
        delimiter: Some('.'),
        ..folder("INBOX.Entw&APw-rfe", None)
    }];
    assert_eq!(display_name(&listed[0]), "INBOX/Entwürfe");
    // "Drafts" under the inbox, by name, in German it is not: no role.
    let plan = plan(
        &listed,
        &[],
        &FolderRoles::default(),
        &Renames::new(),
        false,
    );
    assert_eq!(plan[0].state.role, None);
}
