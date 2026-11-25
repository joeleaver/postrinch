//! Which folders an account has, as labels (docs/imap.md, "One model, two
//! providers"): the special folders take the system labels' ids, the
//! person's choices in the account's settings win over what the server
//! says, and every other folder keeps the id minted when it was first
//! seen. Gmail's virtual `\Flagged` and `\Important` folders are left out:
//! the flags carry what they mirror. So is a `\All` folder anywhere but
//! Gmail ([`view_of_all`]).

use postrinch_model::{
    FolderRole, FolderRoles, ImapFolder, Label, LabelKind, LabelVisibility, SystemLabel,
};

use super::renames::Renames;
use super::utf7;
use crate::imap::folders::{ListedFolder, SpecialUse};
use crate::imap::mint_folder_label_id;

/// One folder to sync this pass: its label, its state, and whether the
/// places kept under its label id belong to another folder (the role moved
/// to a folder of another name), so must be forgotten before it is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Planned {
    pub label: Label,
    pub state: ImapFolder,
    pub forget_places: bool,
}

/// The roles a folder other than the inbox can take, in the order they
/// are given out, with the names providers use when SPECIAL-USE is not
/// offered.
const ROLES: [(Role, &[&str]); 5] = [
    (
        Role::Sent,
        &["Sent", "Sent Items", "Sent Messages", "Sent Mail"],
    ),
    (Role::Drafts, &["Drafts", "Draft"]),
    (
        Role::Trash,
        &["Trash", "Deleted Items", "Deleted Messages", "Deleted"],
    ),
    (
        Role::Junk,
        &["Junk", "Spam", "Junk E-mail", "Junk Email", "Bulk Mail"],
    ),
    (Role::Archive, &["Archive", "Archives"]),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Sent,
    Drafts,
    Trash,
    Junk,
    Archive,
}

impl Role {
    fn folder_role(self) -> FolderRole {
        match self {
            Role::Sent => FolderRole::System(SystemLabel::Sent),
            Role::Drafts => FolderRole::System(SystemLabel::Drafts),
            Role::Trash => FolderRole::System(SystemLabel::Trash),
            Role::Junk => FolderRole::System(SystemLabel::Spam),
            Role::Archive => FolderRole::Archive,
        }
    }

    fn chosen(self, roles: &FolderRoles) -> Option<&str> {
        match self {
            Role::Sent => roles.sent.as_deref(),
            Role::Drafts => roles.drafts.as_deref(),
            Role::Trash => roles.trash.as_deref(),
            Role::Junk => roles.junk.as_deref(),
            Role::Archive => roles.archive.as_deref(),
        }
    }

    /// Whether a folder's special use names this role. `\All` is the
    /// archive only where there is no `\Archive`; it is listed only on
    /// Gmail, where it is All Mail ([`view_of_all`]).
    fn special_use(self, special: SpecialUse, has_archive: bool) -> bool {
        matches!(
            (self, special),
            (Role::Sent, SpecialUse::Sent)
                | (Role::Drafts, SpecialUse::Drafts)
                | (Role::Trash, SpecialUse::Trash)
                | (Role::Junk, SpecialUse::Junk)
                | (Role::Archive, SpecialUse::Archive)
        ) || (self == Role::Archive && special == SpecialUse::All && !has_archive)
    }
}

/// A folder's name as the person reads it: decoded from modified UTF-7,
/// with `/` between levels whatever the server's delimiter.
pub(super) fn display_name(folder: &ListedFolder) -> String {
    shown_name(&folder.name, folder.delimiter)
}

/// A folder's name as the person reads it, from its name on the server
/// and the server's delimiter.
pub(super) fn shown_name(name: &str, delimiter: Option<char>) -> String {
    let decoded = utf7::decode(name);
    match delimiter {
        Some(d) if d != '/' => decoded.replace(d, "/"),
        _ => decoded,
    }
}

fn is_inbox(folder: &ListedFolder) -> bool {
    folder.name.eq_ignore_ascii_case("INBOX")
}

/// Whether `folder` is named `wanted`, at the top or under the inbox (as
/// Courier-style servers keep every folder), ignoring case.
fn named(folder: &ListedFolder, wanted: &str) -> bool {
    let name = display_name(folder);
    let under_inbox = name
        .get(..6)
        .filter(|p| p.eq_ignore_ascii_case("INBOX/"))
        .map(|_| &name[6..]);
    name.eq_ignore_ascii_case(wanted) || under_inbox.is_some_and(|n| n.eq_ignore_ascii_case(wanted))
}

/// The role each listed folder plays, by index: the person's choice
/// first, then SPECIAL-USE, then the usual names; one folder per role.
fn roles_by_index(listed: &[&ListedFolder], roles: &FolderRoles) -> Vec<Option<FolderRole>> {
    let mut out: Vec<Option<FolderRole>> = listed
        .iter()
        .map(|f| is_inbox(f).then_some(FolderRole::System(SystemLabel::Inbox)))
        .collect();
    let has_archive = listed
        .iter()
        .any(|f| f.special_use == Some(SpecialUse::Archive));
    for (role, names) in ROLES {
        let free = |i: usize| out[i].is_none();
        let by_choice = role.chosen(roles).and_then(|chosen| {
            (0..listed.len()).find(|&i| {
                free(i) && (listed[i].name == chosen || display_name(listed[i]) == chosen)
            })
        });
        let found = by_choice
            .or_else(|| {
                (0..listed.len()).find(|&i| {
                    free(i)
                        && listed[i]
                            .special_use
                            .is_some_and(|s| role.special_use(s, has_archive))
                })
            })
            .or_else(|| {
                // Names only stand in where the server marks no folder.
                let marked = listed.iter().any(|f| {
                    f.special_use
                        .is_some_and(|s| role.special_use(s, has_archive))
                });
                (!marked)
                    .then(|| {
                        names.iter().find_map(|name| {
                            (0..listed.len()).find(|&i| free(i) && named(listed[i], name))
                        })
                    })
                    .flatten()
            });
        if let Some(i) = found {
            out[i] = Some(role.folder_role());
        }
    }
    out
}

/// Where a folder comes in a pass: the inbox first, then Sent, Drafts,
/// the person's folders, Archive, Trash and Junk.
pub(super) fn rank(role: Option<FolderRole>) -> u8 {
    match role {
        Some(FolderRole::System(SystemLabel::Inbox)) => 0,
        Some(FolderRole::System(SystemLabel::Sent)) => 1,
        Some(FolderRole::System(SystemLabel::Drafts)) => 2,
        None => 3,
        Some(FolderRole::Archive) => 4,
        Some(FolderRole::System(SystemLabel::Trash)) => 5,
        Some(FolderRole::System(_)) => 6,
    }
}

/// The capability by which Gmail says it is Gmail.
pub(super) const GMAIL: &str = "X-GM-EXT-1";

/// Whether a listed folder is a `\All` folder of a server other than
/// Gmail (`gmail`, the [`GMAIL`] capability). Gmail's All Mail holds a copy
/// of every message, kept when every other label goes. Elsewhere a `\All`
/// folder (Dovecot's `virtual/All` and the like) is a view of the mail in
/// the other folders: its "copies" are those folders' messages, so a copy
/// deleted from it, or the last real copy deleted because it seemed to
/// stay there, is the message lost. Such a folder is not synced, and the
/// outbox never counts it as keeping a message.
pub(super) fn view_of_all(folder: &ListedFolder, gmail: bool) -> bool {
    !gmail && folder.special_use == Some(SpecialUse::All)
}

/// Whether a listed folder is synced: it can be selected, and is not one
/// of Gmail's virtual folders.
pub(super) fn syncable(folder: &ListedFolder) -> bool {
    folder.selectable
        && !matches!(
            folder.special_use,
            Some(SpecialUse::Flagged | SpecialUse::Important)
        )
}

/// The listed folder playing `role` (the person's choice, SPECIAL-USE or
/// the usual names, as a pass gives it out), for sending before the
/// first pass has kept the account's folders.
pub(super) fn role_folder<'a>(
    listed: &'a [ListedFolder],
    roles: &FolderRoles,
    role: FolderRole,
) -> Option<&'a ListedFolder> {
    let wanted: Vec<&ListedFolder> = listed.iter().filter(|f| syncable(f)).collect();
    roles_by_index(&wanted, roles)
        .into_iter()
        .zip(wanted)
        .find_map(|(r, folder)| (r == Some(role)).then_some(folder))
}

/// The folders to sync, in the order to sync them, from what LIST gave,
/// the folders kept from earlier passes and the renames found among them
/// (a renamed folder is the kept one under its new name).
pub(super) fn plan(
    listed: &[ListedFolder],
    stored: &[ImapFolder],
    roles: &FolderRoles,
    renames: &Renames,
) -> Vec<Planned> {
    let wanted: Vec<&ListedFolder> = listed.iter().filter(|f| syncable(f)).collect();
    let by_index = roles_by_index(&wanted, roles);
    let mut planned: Vec<Planned> = wanted
        .iter()
        .zip(by_index)
        .map(|(folder, role)| {
            let system = match role {
                Some(FolderRole::System(system)) => Some(system),
                _ => None,
            };
            let renamed_from = renames.get(&folder.name);
            let kept = stored
                .iter()
                .find(|s| s.name == folder.name)
                .or_else(|| stored.iter().find(|s| Some(&s.label_id) == renamed_from));
            // A folder keeps its id while its role keeps its kind: a user
            // folder made Sent becomes `SENT`, and Sent made an ordinary
            // folder gets an id of its own.
            let label_id = match (system, kept) {
                (Some(system), _) => system.provider_id().to_owned(),
                (None, Some(kept)) if SystemLabel::from_provider_id(&kept.label_id).is_none() => {
                    kept.label_id.clone()
                }
                (None, _) => mint_folder_label_id(),
            };
            let previous = stored.iter().find(|s| s.label_id == label_id);
            let same_folder = previous
                .is_some_and(|p| p.name == folder.name || Some(&p.label_id) == renamed_from);
            let mut state = match previous.filter(|_| same_folder) {
                Some(previous) => previous.clone(),
                None => ImapFolder::new(label_id.clone(), folder.name.clone()),
            };
            state.name = folder.name.clone();
            state.delimiter = folder.delimiter;
            state.role = role;
            let kind = match system {
                Some(system) => LabelKind::System(system),
                None => LabelKind::User,
            };
            let mut label = Label::new(label_id.clone(), display_name(folder), kind);
            label.visibility = LabelVisibility::Show;
            Planned {
                label,
                state,
                forget_places: previous.is_some() && !same_folder,
            }
        })
        .collect();
    planned.sort_by_key(|p| rank(p.state.role));
    planned
}

#[cfg(test)]
mod tests {
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
            .map(|p| (p.state.name.as_str(), p.label.provider_id.as_str()))
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
        let plan = plan(&listed, &[], &FolderRoles::default(), &Renames::new());
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
        let plan = plan(&listed, &[], &roles, &Renames::new());
        let bin = plan.iter().find(|p| p.state.name == "Bin").unwrap();
        assert_eq!(bin.label.provider_id, "TRASH");
        let trash = plan.iter().find(|p| p.state.name == "Trash").unwrap();
        assert_eq!(trash.state.role, None);
        assert_eq!(trash.label.kind, LabelKind::User);
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
        let plan = plan(&listed, &[], &FolderRoles::default(), &Renames::new());
        let names: Vec<_> = plan.iter().map(|p| p.state.name.as_str()).collect();
        assert_eq!(names, ["INBOX", "[Gmail]/Sent Mail", "[Gmail]/All Mail"]);
        assert_eq!(plan[2].state.role, Some(FolderRole::Archive));
        assert_eq!(plan[2].label.name, "[Gmail]/All Mail");
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
        );
        let kept = plan.iter().find(|p| p.state.name == "Work").unwrap();
        assert_eq!(kept.state.uidnext, 30);
        assert_eq!(kept.label.provider_id, "f0123456789ab");
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
        let plan = plan(&listed, &[sent], &FolderRoles::default(), &Renames::new());
        let new = plan.iter().find(|p| p.state.name == "Sent Items").unwrap();
        assert_eq!(new.label.provider_id, "SENT");
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
        let plan = plan(&listed, &[work], &FolderRoles::default(), &renames);
        let renamed = plan.iter().find(|p| p.state.name == "Projects").unwrap();
        assert_eq!(renamed.label.provider_id, "f0123456789ab");
        assert_eq!(renamed.label.name, "Projects");
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
        let plan = plan(&listed, &[], &FolderRoles::default(), &Renames::new());
        assert_eq!(plan[0].state.role, None);
    }
}
