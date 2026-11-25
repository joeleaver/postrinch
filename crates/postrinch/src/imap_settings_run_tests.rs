//! The Folders section's choices and the sign-in-again dialog's secret.

use postrinch_model::{FolderRole, FolderRoles, SystemLabel};
use postrinch_sync::imap::{AccountFolder, ImapSecret};

use super::*;

fn folder(name: &str, shown: &str, role: Option<FolderRole>) -> AccountFolder {
    AccountFolder {
        name: name.into(),
        shown: shown.into(),
        role,
    }
}

fn folders() -> Vec<AccountFolder> {
    vec![
        folder("Copies", "Copies", None),
        folder("Entw&APw-rfe", "Entwürfe", None),
        folder(
            "Sent Items",
            "Sent Items",
            Some(FolderRole::System(SystemLabel::Sent)),
        ),
    ]
}

#[test]
fn the_server_choice_names_the_folder_it_picks() {
    let choices = role_choices(Role::Sent, &folders(), &FolderRoles::default());
    assert_eq!(choices[0].value, SERVER_SAYS);
    assert_eq!(choices[0].label, "Sent Items (the server says)");
    let chosen: Vec<Option<String>> = choices[1..]
        .iter()
        .map(|c| chosen_folder(&c.value))
        .collect();
    let names = ["Copies", "Entw&APw-rfe", "Sent Items"].map(|n| Some(n.to_owned()));
    assert_eq!(
        chosen, names,
        "each folder chosen by its name on the server"
    );
    assert_eq!(choices[2].label, "Entwürfe", "shown decoded");

    let none = role_choices(Role::Junk, &folders(), &FolderRoles::default());
    assert_eq!(none[0].label, "None (the server says)");
}

#[test]
fn no_folder_is_taken_for_the_server_choice() {
    // A select shows an empty value as nothing chosen.
    assert!(!SERVER_SAYS.is_empty());
    assert_eq!(role_value(None), SERVER_SAYS);
    assert_eq!(chosen_folder(SERVER_SAYS), None);
    // Folders named like the server's choice, or empty, stay folders.
    for name in [SERVER_SAYS, "", "folder:x"] {
        assert_eq!(
            chosen_folder(&role_value(Some(name))).as_deref(),
            Some(name)
        );
    }
}

#[test]
fn once_the_person_chose_the_server_choice_is_a_way_back() {
    let mut roles = FolderRoles::default();
    Role::Sent.choose(&mut roles, Some("Copies".into()));
    assert_eq!(roles.sent.as_deref(), Some("Copies"));
    assert_eq!(Role::Sent.chosen(&roles).as_deref(), Some("Copies"));
    let choices = role_choices(Role::Sent, &folders(), &roles);
    assert_eq!(choices[0].label, "As the server says");
    Role::Sent.choose(&mut roles, None);
    assert_eq!(roles, FolderRoles::default());
}

#[test]
fn each_role_has_its_own_slot_and_label() {
    let mut roles = FolderRoles::default();
    for (i, role) in Role::ALL.into_iter().enumerate() {
        role.choose(&mut roles, Some(i.to_string()));
    }
    assert_eq!(
        [
            roles.sent.as_deref(),
            roles.drafts.as_deref(),
            roles.archive.as_deref(),
            roles.trash.as_deref(),
            roles.junk.as_deref()
        ],
        [Some("0"), Some("1"), Some("2"), Some("3"), Some("4")]
    );
    assert_eq!(
        Role::Junk.folder_role(),
        FolderRole::System(SystemLabel::Spam)
    );
    assert_eq!(Role::Archive.folder_role(), FolderRole::Archive);
}

#[test]
fn signing_in_again_needs_each_password_it_asks_for() {
    assert_eq!(
        again_secret(String::new(), None).unwrap_err(),
        "Give the password."
    );
    assert_eq!(
        again_secret("pw".into(), None).unwrap(),
        ImapSecret::password("pw")
    );
    assert!(again_secret("pw".into(), Some(String::new())).is_err());
    assert_eq!(
        again_secret("pw".into(), Some("out".into())).unwrap(),
        ImapSecret::Password {
            password: "pw".into(),
            smtp_password: Some("out".into())
        }
    );
}
