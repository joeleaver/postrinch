//! The IMAP commands outgoing mail needs, on the account's outgoing
//! connection: finding Sent and Drafts, APPEND, looking for a message by
//! its `Message-ID`, and deleting copies of one by UID; and keeping the
//! store's places in step with what they did.

use postrinch_model::{
    EmailAddress, FolderRole, ImapPlace, ImapSettings, ProviderMessageId, SystemLabel,
};

use super::super::folder_error::FolderError;
use super::super::names::Command;
use super::super::pending;
use super::super::places::{self, DELETED};
use super::super::wire::{Conn, Done, Selected, uid_set};
use super::super::{ImapSync, folders};
use crate::imap::ids::bare;
use crate::{SyncError, SyncResult};

/// A folder outgoing mail goes to: its name on the server, and its label
/// id when sync keeps the folder (places are recorded only then).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Target {
    pub name: String,
    pub label: Option<String>,
}

/// What APPEND did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Appended {
    /// The server took the message; where it went, with UIDPLUS
    /// (`(uidvalidity, uid)`).
    Kept(Option<(u32, u32)>),
    /// The server would not take it (`NO`: over quota, too large, no such
    /// folder), in its words.
    Refused(String),
}

/// How copies were deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Deleted {
    /// Gone from the folder.
    Expunged,
    /// Marked `\Deleted` only: without UIDPLUS, a plain `EXPUNGE` would
    /// also take away copies someone else marked, so they stay marked,
    /// which Postrinch counts as gone (docs/imap.md, "Changing mail").
    Marked,
}

impl ImapSync {
    /// The folder playing `role`: as sync keeps it, or, before the first
    /// pass has kept the account's folders, from LIST. `None` when the
    /// account has no such folder.
    pub(super) async fn target(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        settings: &ImapSettings,
        role: SystemLabel,
    ) -> SyncResult<Option<Target>> {
        let role = FolderRole::System(role);
        let a = account.clone();
        let kept = self.with_store(move |s| s.imap_folders(&a)).await?;
        if let Some(folder) = kept.into_iter().find(|f| f.role == Some(role)) {
            return Ok(Some(Target {
                name: folder.name,
                label: Some(folder.label_id),
            }));
        }
        let listed = conn.session_mut().list_folders().await?;
        Ok(
            folders::role_folder(&listed, &settings.roles, role).map(|folder| Target {
                name: folder.name.clone(),
                label: None,
            }),
        )
    }

    /// Records the copy an APPEND made (with UIDPLUS, and where sync
    /// keeps the folder), so the message is known to be there before the
    /// next pass reads it; that pass then stores it once, by the same id.
    /// Without them the pass finds it like any new mail.
    pub(super) async fn record(
        &self,
        account: &EmailAddress,
        target: &Target,
        at: Option<(u32, u32)>,
        message: &ProviderMessageId,
        flags: &[&str],
    ) -> SyncResult<()> {
        let (Some(label), Some((uidvalidity, uid))) = (target.label.clone(), at) else {
            return Ok(());
        };
        let place = ImapPlace {
            folder: label,
            uidvalidity,
            uid,
            message: message.clone(),
            flags: places::normalize(&flags.iter().map(|f| f.to_string()).collect::<Vec<_>>()),
            modseq: 0,
            arrived: Some(places::now()),
        };
        let a = account.clone();
        self.blocking(move |s| {
            if s.imap_folder(&a, &place.folder)?.is_none() {
                return Ok(());
            }
            s.put_imap_place(&a, &place)?;
            places::refresh(s, &a, &place.message)?;
            Ok(())
        })
        .await
    }

    /// Brings the store in step with deleting `uids` (copies of `message`
    /// in a folder sync keeps as `label`): expunged copies are forgotten,
    /// marked ones kept as marked, and the message loses the folder's
    /// label when no copy is left there; with no live copy left anywhere
    /// it is pending, as sync makes a message that went.
    pub(super) async fn forget(
        &self,
        account: &EmailAddress,
        (label, uidvalidity): (String, u32),
        uids: Vec<u32>,
        message: &ProviderMessageId,
        deleted: Deleted,
    ) -> SyncResult<()> {
        let (a, message) = (account.clone(), message.clone());
        self.blocking(move |s| {
            let gone = s
                .imap_message_places(&a, &message)?
                .into_iter()
                .filter(|p| {
                    p.folder == label && p.uidvalidity == uidvalidity && uids.contains(&p.uid)
                });
            for mut place in gone {
                match deleted {
                    Deleted::Expunged => {
                        s.remove_imap_place(&a, &label, uidvalidity, place.uid)?;
                    }
                    Deleted::Marked => {
                        place.flags.push(DELETED.to_owned());
                        place.flags = places::normalize(&place.flags);
                        s.put_imap_place(&a, &place)?;
                    }
                }
            }
            if places::summary(&s.imap_message_places(&a, &message)?).is_none() {
                // No live copy is left: it waits as one sync saw go does,
                // and goes at the end of a later pass.
                pending::mark(s, &a, [&message])?;
            }
            places::refresh(s, &a, &message)?;
            Ok(())
        })
        .await
    }
}

/// A folder's busy answer is the server busy for what a send does there.
fn folder_error(conn: &Conn, error: FolderError) -> SyncError {
    match error {
        FolderError::Busy(message) => SyncError::Busy {
            service: "IMAP",
            server: conn.describe(),
            message,
        },
        FolderError::Sync(e) => e,
    }
}

fn refused(conn: &Conn, message: String) -> SyncError {
    SyncError::Server {
        service: "IMAP",
        server: conn.describe(),
        message,
    }
}

/// `APPEND <folder> (<flags>) <message>`.
pub(super) async fn append(
    conn: &mut Conn,
    folder: &str,
    flags: &str,
    message: &[u8],
) -> SyncResult<Appended> {
    let command = Command::with_mailbox("APPEND", folder)
        .ok_or_else(|| refused(conn, format!("the folder {folder:?} cannot be named")))?
        .then(&format!(" ({flags}) "))
        .literal(message.to_vec());
    match conn
        .command("filing a message", &command, |_| Ok(()))
        .await?
    {
        Done::Ok(completed) => Ok(Appended::Kept(completed.appended)),
        Done::No(refusal) => Ok(Appended::Refused(refusal.text)),
        Done::FolderBusy(text) => Err(folder_error(conn, FolderError::Busy(text))),
    }
}

/// `SELECT`s a folder, to change what is in it.
pub(super) async fn select(conn: &mut Conn, folder: &str) -> SyncResult<Selected> {
    match conn.select(folder).await {
        Ok(Some(selected)) => Ok(selected),
        Ok(None) => Err(refused(
            conn,
            format!("the server would not open {folder:?}"),
        )),
        Err(e) => Err(folder_error(conn, e)),
    }
}

/// `EXAMINE`s a folder, to look in it. `None` when the server refuses to
/// open it (deleted on the server since the store saw it): there is
/// nothing in it to find or count, and what is filed there is refused in
/// its turn. A busy answer is the server busy.
pub(super) async fn examine(conn: &mut Conn, folder: &str) -> SyncResult<Option<Selected>> {
    conn.examine(folder)
        .await
        .map_err(|e| folder_error(conn, e))
}

/// The UIDs of the copies in the selected folder whose `Message-ID` is
/// `header`. A header that cannot be searched for as a quoted string
/// (not ASCII, or with a line break) finds nothing.
pub(super) async fn find(conn: &mut Conn, header: &str) -> SyncResult<Vec<u32>> {
    let id = bare(header);
    if id.is_empty() || !id.is_ascii() || id.contains(['\r', '\n']) {
        return Ok(Vec::new());
    }
    let quoted = format!("<{id}>").replace('\\', "\\\\").replace('"', "\\\"");
    let criteria = format!("HEADER Message-ID \"{quoted}\"");
    match conn.uid_search(&criteria).await {
        Ok(Some(uids)) => Ok(uids),
        Ok(None) => Err(refused(
            conn,
            "the server would not search for a message".into(),
        )),
        Err(e) => Err(folder_error(conn, e)),
    }
}

/// Deletes the copies `uids` in the selected folder: `\Deleted`, then
/// `UID EXPUNGE` (UIDPLUS); without UIDPLUS a plain `EXPUNGE`, but only
/// when no other copy there is marked `\Deleted`.
///
/// Without UIDPLUS this has a race no client can close: another client
/// may mark a message `\Deleted` between the `UID SEARCH DELETED` and the
/// `EXPUNGE`, which then takes it too. It only goes sooner than its owner
/// would have expunged it, as that client meant it to go (docs/imap.md,
/// "Writing and sending").
pub(super) async fn delete(conn: &mut Conn, uids: &[u32]) -> SyncResult<Deleted> {
    let set = uid_set(uids);
    let mark = Command::plain(format!("UID STORE {set} +FLAGS.SILENT (\\Deleted)"));
    expect_ok(conn, "deleting a message", &mark).await?;
    if conn.session_mut().capabilities().uidplus() {
        let expunge = Command::plain(format!("UID EXPUNGE {set}"));
        expect_ok(conn, "deleting a message", &expunge).await?;
        return Ok(Deleted::Expunged);
    }
    let marked = match conn.uid_search("DELETED").await {
        Ok(Some(marked)) => marked,
        Ok(None) => return Ok(Deleted::Marked),
        Err(e) => return Err(folder_error(conn, e)),
    };
    if marked.iter().any(|uid| !uids.contains(uid)) {
        return Ok(Deleted::Marked);
    }
    expect_ok(conn, "deleting a message", &Command::plain("EXPUNGE")).await?;
    Ok(Deleted::Expunged)
}

/// Sends a command that must succeed.
async fn expect_ok(conn: &mut Conn, what: &str, command: &Command) -> SyncResult<()> {
    match conn.command(what, command, |_| Ok(())).await? {
        Done::Ok(_) => Ok(()),
        Done::No(refusal) => Err(refused(
            conn,
            format!("{} refused: {}", command.verb(), refusal.text),
        )),
        Done::FolderBusy(text) => Err(folder_error(conn, FolderError::Busy(text))),
    }
}
