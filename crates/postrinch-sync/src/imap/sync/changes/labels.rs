//! Labels as folders: making, renaming and deleting one is `CREATE`,
//! `RENAME` and `DELETE` (docs/imap.md, "Changing mail").
//!
//! A label's name is its path with `/` between levels, as the person reads
//! it; on the server each level is in modified UTF-7, joined by the
//! server's delimiter, under `INBOX` where the server keeps every folder
//! there (Courier's way; see [`server_name`]). The special folders (the
//! inbox, Sent, Drafts, Trash, Junk and the archive) are not renamed or
//! deleted from here; deleting a label is [`super::drop_label`]'s.

use postrinch_model::{
    EmailAddress, FolderRole, ImapFolder, Label, LabelKind, LabelVisibility, SystemLabel,
};

use super::super::names::Command;
use super::super::wire::Conn;
use super::super::{ImapSync, Lane, utf7};
use super::commands::shown;
use super::failure::{Failure, Outcome};
use super::plan::Folders;
use crate::SyncResult;
use crate::imap::mint_folder_label_id;

/// The name a folder made for the archive gets.
const ARCHIVE: &str = "Archive";

/// The delimiter the account's folders use (the inbox's first).
fn delimiter(folders: &Folders) -> char {
    folders
        .get(SystemLabel::Inbox.provider_id())
        .and_then(|f| f.delimiter)
        .or_else(|| folders.all().find_map(|f| f.delimiter))
        .unwrap_or('/')
}

/// Whether the server keeps every folder under the inbox (Courier, or a
/// personal namespace of `INBOX.`): its special folders are all there and
/// no folder is beside the inbox.
///
/// RFC 2342's `NAMESPACE` says this outright, but the IMAP parser sync
/// uses (imap-proto) cannot read its answer, and an answer it cannot read
/// stops the connection, so the folders are the evidence: the special
/// folders sync found, not the person's own, as an account elsewhere may
/// keep its own folders under the inbox by choice. An account with no
/// special folder yet gets its labels beside the inbox.
fn under_inbox(folders: &Folders, inbox: &str) -> bool {
    let in_inbox = |f: &ImapFolder| {
        f.name
            .get(..inbox.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(inbox))
    };
    let others: Vec<&ImapFolder> = folders
        .all()
        .filter(|f| !f.name.eq_ignore_ascii_case("INBOX"))
        .collect();
    let special: Vec<&&ImapFolder> = others.iter().filter(|f| f.role.is_some()).collect();
    !special.is_empty() && others.iter().all(|f| in_inbox(f))
}

/// The name on the server for the label path `path`.
fn server_name(folders: &Folders, path: &str) -> Outcome<String> {
    let d = delimiter(folders);
    let inbox = format!("INBOX{d}");
    let under_inbox = under_inbox(folders, &inbox);
    let parts: Vec<&str> = path.split('/').map(str::trim).collect();
    if parts.iter().any(|p| p.is_empty()) {
        return Err(Failure::Refused(format!(
            "a label's name cannot be empty or have an empty level: \"{path}\""
        )));
    }
    if d != '/' && parts.iter().any(|p| p.contains(d)) {
        return Err(Failure::Refused(format!(
            "this server keeps folders apart with \"{d}\", so a label's name cannot have it"
        )));
    }
    let name = parts
        .iter()
        .map(|p| utf7::encode(p))
        .collect::<Vec<_>>()
        .join(&d.to_string());
    let already = name
        .get(..inbox.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(&inbox));
    Ok(match under_inbox && !already {
        true => format!("{inbox}{name}"),
        false => name,
    })
}

/// A folder's name as the person reads it, as sync shows it.
fn display(name: &str, delimiter: Option<char>) -> String {
    let decoded = utf7::decode(name);
    match delimiter {
        Some(d) if d != '/' => decoded.replace(d, "/"),
        _ => decoded,
    }
}

/// The special folders are the account's own, not the person's labels.
pub(super) fn changeable(folders: &Folders, id: &str) -> Outcome<ImapFolder> {
    let folder = folders
        .get(id)
        .ok_or_else(|| Failure::Refused("the label is gone".to_owned()))?;
    if SystemLabel::from_provider_id(id).is_some() || folder.role.is_some() {
        return Err(Failure::Refused(format!(
            "{} is one of the account's special folders",
            shown(&folder.name)
        )));
    }
    if folders.is_view(id) {
        return Err(Failure::Refused(format!(
            "{} is a view of the mail in the other folders",
            shown(&folder.name)
        )));
    }
    Ok(folder.clone())
}

pub(super) fn mailbox(verb: &str, name: &str) -> Outcome<Command> {
    Command::with_mailbox(verb, name).ok_or_else(|| {
        Failure::Refused(format!(
            "the folder {} has a name that cannot be sent to the server",
            shown(name)
        ))
    })
}

fn label_of(folder: &ImapFolder) -> Label {
    let mut label = Label::new(
        folder.label_id.clone(),
        display(&folder.name, folder.delimiter),
        LabelKind::User,
    );
    label.visibility = LabelVisibility::Show;
    label
}

impl ImapSync {
    /// Makes a label: a folder named `name` (its path, with `/` between
    /// levels).
    pub async fn create_label(&self, account: &EmailAddress, name: &str) -> SyncResult<Label> {
        self.on_folders(account, async |conn, folders| {
            let made = self.make_folder(conn, account, folders, name, None).await?;
            Ok(label_of(&made))
        })
        .await
    }

    /// Renames a label's folder (and the folders under it); its mail and
    /// its id stay.
    pub async fn rename_label(
        &self,
        account: &EmailAddress,
        id: &str,
        name: &str,
    ) -> SyncResult<Label> {
        self.on_folders(account, async |conn, folders| {
            let folder = changeable(folders, id)?;
            let new = server_name(folders, name)?;
            let command = mailbox("RENAME", &folder.name)?
                .then_mailbox(&new)
                .ok_or_else(|| Failure::Refused(format!("{} cannot be sent", shown(&new))))?;
            conn.folder_command(&format!("renaming {}", shown(&folder.name)), &command)
                .await?;
            let (a, old, d) = (account.clone(), folder.name.clone(), folder.delimiter);
            let moved: Vec<ImapFolder> = folders
                .all()
                .filter_map(|f| {
                    let rest = match f.name.strip_prefix(old.as_str()) {
                        Some("") => "",
                        Some(rest) if d.is_some_and(|d| rest.starts_with(d)) => rest,
                        _ => return None,
                    };
                    Some(ImapFolder {
                        name: format!("{new}{rest}"),
                        ..f.clone()
                    })
                })
                .collect();
            let renamed = moved.iter().find(|f| f.label_id == id).cloned();
            self.with_store(move |s| {
                for f in &moved {
                    s.put_imap_folder(&a, f)?;
                    let mut label = s
                        .labels(&a)?
                        .into_iter()
                        .find(|l| l.provider_id == f.label_id)
                        .unwrap_or_else(|| label_of(f));
                    label.name = display(&f.name, f.delimiter);
                    s.put_label(&a, &label)?;
                }
                Ok(())
            })
            .await?;
            let renamed = renamed.unwrap_or(ImapFolder {
                name: new,
                ..folder
            });
            folders.insert(renamed.clone());
            Ok(label_of(&renamed))
        })
        .await
    }

    /// Whether the folder `name` is missing from LIST.
    pub(super) async fn gone(&self, conn: &mut Conn, name: &str) -> Outcome<bool> {
        let listed = conn.session_mut().list_folders().await?;
        Ok(!listed.iter().any(|f| f.name == name))
    }

    /// Runs label work over the passes' connection, with the account's
    /// folders, giving the server's words for what it refused.
    pub(super) async fn on_folders<T>(
        &self,
        account: &EmailAddress,
        work: impl AsyncFnOnce(&mut Conn, &mut Folders) -> Outcome<T>,
    ) -> SyncResult<T> {
        let result = self
            .over(account, Lane::Sync, async |conn| {
                let mut folders = self.folders(conn, account).await?;
                let server = conn.describe();
                work(conn, &mut folders)
                    .await
                    .map_err(|e| e.into_error(server))
            })
            .await;
        self.note_sign_in(account, &result).await?;
        result
    }

    /// Makes the folder archiving files mail in, for an account that has
    /// none.
    pub(super) async fn make_archive(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        folders: &mut Folders,
    ) -> Outcome<()> {
        self.make_folder(conn, account, folders, ARCHIVE, Some(FolderRole::Archive))
            .await
            .map(|_| ())
    }

    /// `CREATE`s the folder for the label path `path` (one that is there
    /// already will do), subscribes to it, and keeps it as a label.
    async fn make_folder(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        folders: &mut Folders,
        path: &str,
        role: Option<FolderRole>,
    ) -> Outcome<ImapFolder> {
        let name = server_name(folders, path)?;
        let what = format!("making the folder {}", shown(&name));
        if let Err(refused) = conn.folder_command(&what, &mailbox("CREATE", &name)?).await {
            // One there already (`[ALREADYEXISTS]`) will do.
            let there = matches!(refused, Failure::Refused(_)) && !self.gone(conn, &name).await?;
            if !there {
                return Err(refused);
            }
        }
        // Subscribing is how other clients are told to show it; a server
        // that will not is no reason to undo the folder.
        let what = format!("subscribing to {}", shown(&name));
        if let Err(Failure::Wait(e)) = conn
            .folder_command(&what, &mailbox("SUBSCRIBE", &name)?)
            .await
        {
            return Err(Failure::Wait(e));
        }
        let kept = folders.all().find(|f| f.name == name).cloned();
        let folder = ImapFolder {
            role: role.or_else(|| kept.as_ref().and_then(|k| k.role)),
            delimiter: Some(delimiter(folders)),
            subscribed: true,
            ..kept.unwrap_or_else(|| ImapFolder::new(mint_folder_label_id(), name))
        };
        let label = label_of(&folder);
        let (a, f) = (account.clone(), folder.clone());
        self.with_store(move |s| {
            s.put_label(&a, &label)?;
            s.put_imap_folder(&a, &f)
        })
        .await?;
        folders.insert(folder.clone());
        Ok(folder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The folders with these names, Sent and Trash (under the inbox or
    /// not) given their roles, as sync gives them.
    fn folders(names: &[(&str, char)]) -> Folders {
        let role = |name: &str| match name.rsplit(['.', '/']).next() {
            Some("Sent") => Some(FolderRole::System(SystemLabel::Sent)),
            Some("Trash") => Some(FolderRole::System(SystemLabel::Trash)),
            _ => None,
        };
        let stored = names
            .iter()
            .enumerate()
            .map(|(i, (name, d))| ImapFolder {
                delimiter: Some(*d),
                role: role(name),
                ..ImapFolder::new(
                    if *name == "INBOX" {
                        "INBOX".to_owned()
                    } else {
                        format!("f{i:012}")
                    },
                    *name,
                )
            })
            .collect();
        Folders::new(stored, &[], false)
    }

    #[test]
    fn names_take_the_servers_delimiter_and_encoding() {
        let plain = folders(&[("INBOX", '/'), ("Work", '/')]);
        assert_eq!(
            server_name(&plain, "Work/Entwürfe").unwrap(),
            "Work/Entw&APw-rfe"
        );
        let dotted = folders(&[("INBOX", '.'), ("Work", '.')]);
        assert_eq!(server_name(&dotted, "A/B").unwrap(), "A.B");
        assert!(matches!(
            server_name(&dotted, "a.b"),
            Err(Failure::Refused(_))
        ));
        assert!(matches!(
            server_name(&plain, "a//b"),
            Err(Failure::Refused(_))
        ));
    }

    #[test]
    fn courier_keeps_every_folder_under_the_inbox() {
        let courier = folders(&[("INBOX", '.'), ("INBOX.Sent", '.'), ("INBOX.Trash", '.')]);
        assert_eq!(server_name(&courier, "Work").unwrap(), "INBOX.Work");
        assert_eq!(server_name(&courier, "INBOX/Work").unwrap(), "INBOX.Work");
        assert_eq!(display("INBOX.Work", Some('.')), "INBOX/Work");
    }

    #[test]
    fn folders_of_the_persons_under_the_inbox_are_not_courier() {
        // Dovecot, its special folders beside the inbox or none yet: the
        // person's own folders under the inbox say nothing.
        let none_yet = folders(&[("INBOX", '/'), ("INBOX/Work", '/')]);
        assert_eq!(server_name(&none_yet, "Projects").unwrap(), "Projects");
        let beside = folders(&[("INBOX", '/'), ("INBOX/Work", '/'), ("Sent", '/')]);
        assert_eq!(server_name(&beside, "Projects").unwrap(), "Projects");
        // A personal namespace of `INBOX/`: everything there.
        let prefixed = folders(&[("INBOX", '/'), ("INBOX/Work", '/'), ("INBOX/Sent", '/')]);
        assert_eq!(
            server_name(&prefixed, "Projects").unwrap(),
            "INBOX/Projects"
        );
    }
}
