//! The commands that change mail and folders, as the outbox sends them:
//! each answers with what it did, or a [`Failure`] saying whether it is
//! for good, may pass, or is to be waited out.

use std::collections::HashSet;

use async_imap::imap_proto::rfc4315::UidSetMember;
use async_imap::imap_proto::{MailboxDatum, Response, ResponseCode, StatusAttribute};

use super::super::names::Command;
use super::super::utf7;
use super::super::wire::{Conn, Done, FetchReply, Selected, uid_set};
use super::failure::{Failure, Outcome, refusal};

/// How many UIDs one command names at most, so a thousand archives are a
/// few commands and no line grows past what servers take (Dovecot reads
/// lines of up to 64 KB).
pub(super) const CHUNK: usize = 500;

/// RFC 4315's `COPYUID`: the UIDs the copies got, in the folder's
/// UIDVALIDITY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CopyUid {
    pub uidvalidity: u32,
    /// Each source UID with its copy's UID.
    pub pairs: Vec<(u32, u32)>,
}

impl CopyUid {
    fn of(uidvalidity: u32, from: &[UidSetMember], to: &[UidSetMember]) -> Self {
        let expand = |set: &[UidSetMember]| -> Vec<u32> {
            set.iter()
                .flat_map(|m| match m {
                    UidSetMember::Uid(u) => *u..=*u,
                    UidSetMember::UidRange(r) => r.clone(),
                })
                .collect()
        };
        Self {
            uidvalidity,
            pairs: expand(from).into_iter().zip(expand(to)).collect(),
        }
    }
}

/// A folder's name as the person reads it, for reasons.
pub(super) fn shown(name: &str) -> String {
    format!("\"{}\"", utf7::decode(name))
}

impl Conn {
    /// Whether the server offers an extension, by its capability name.
    pub(in crate::imap::sync) fn offers(&mut self, capability: &str) -> bool {
        self.session_mut().capabilities().has(capability)
    }

    /// `SELECT`s the folder named `name` on the server.
    pub(super) async fn select_to_change(&mut self, name: &str) -> Outcome<Selected> {
        let server = self.describe();
        match self.open_or_refusal("SELECT", name).await {
            Ok(Ok(selected)) => Ok(selected),
            Ok(Err(Some(no))) => Err(refusal(&format!("opening {}", shown(name)), &no)),
            Ok(Err(None)) => Err(Failure::Refused(format!(
                "the folder {} has a name that cannot be sent back to the server",
                shown(name)
            ))),
            Err(e) => Err(Failure::in_folder(e, server)),
        }
    }

    /// `UID STORE <uids> ±FLAGS.SILENT (<flag>)` in the selected folder.
    pub(super) async fn store_flag(&mut self, uids: &[u32], flag: &str, add: bool) -> Outcome<()> {
        let sign = if add { '+' } else { '-' };
        let command = Command::plain(format!(
            "UID STORE {} {sign}FLAGS.SILENT ({flag})",
            uid_set(uids)
        ));
        self.run("changing flags", &command).await
    }

    /// `UID COPY` (or, `moving`, `UID MOVE`) of `uids` from the selected
    /// folder into the folder named `to`, and the `COPYUID` it answered
    /// with, where it did (UIDPLUS).
    pub(super) async fn transfer(
        &mut self,
        uids: &[u32],
        to: &str,
        moving: bool,
    ) -> Outcome<Option<CopyUid>> {
        let verb = if moving { "MOVE" } else { "COPY" };
        let what = if moving {
            format!("moving mail to {}", shown(to))
        } else {
            format!("copying mail to {}", shown(to))
        };
        let Some(command) = Command::with_mailbox(&format!("UID {verb} {}", uid_set(uids)), to)
        else {
            return Err(Failure::Refused(format!(
                "the folder {} has a name that cannot be sent back to the server",
                shown(to)
            )));
        };
        let mut copied = None;
        let done = self
            .command(&what, &command, |response| {
                let outcome = match response {
                    Response::Data { outcome, .. } | Response::Done { outcome, .. } => outcome,
                    _ => return Ok(()),
                };
                if let Some(ResponseCode::CopyUid(validity, from, to)) = &outcome.code {
                    copied = Some(CopyUid::of(*validity, from, to));
                }
                Ok(())
            })
            .await?;
        self.ended(&what, done)?;
        Ok(copied)
    }

    /// The UIDs marked `\Deleted` in the selected folder.
    pub(super) async fn deleted_uids(&mut self) -> Outcome<Vec<u32>> {
        let server = self.describe();
        match self.uid_search("DELETED").await {
            Ok(Some(uids)) => Ok(uids),
            Ok(None) => Err(Failure::Declined(
                "the server would not list the mail marked deleted".to_owned(),
            )),
            Err(e) => Err(Failure::in_folder(e, server)),
        }
    }

    /// Which of `uids` the selected folder has, not marked `\Deleted`
    /// (`UID SEARCH UID <uids> UNDELETED`); `None` when the server will
    /// not say.
    pub(super) async fn present(&mut self, uids: &[u32]) -> Outcome<Option<HashSet<u32>>> {
        self.search_to_change(&format!("UID {} UNDELETED", uid_set(uids)))
            .await
            .map(|found| found.map(|uids| uids.into_iter().collect()))
    }

    /// Every UID of the selected folder not marked `\Deleted`; `None`
    /// when the server will not say.
    pub(super) async fn undeleted(&mut self) -> Outcome<Option<Vec<u32>>> {
        self.search_to_change("UNDELETED").await
    }

    async fn search_to_change(&mut self, criteria: &str) -> Outcome<Option<Vec<u32>>> {
        let server = self.describe();
        self.uid_search(criteria)
            .await
            .map_err(|e| Failure::in_folder(e, server))
    }

    /// `UID EXPUNGE <uids>` (UIDPLUS), or, without `uids`, `EXPUNGE`:
    /// every message of the selected folder marked `\Deleted` goes.
    pub(super) async fn expunge(&mut self, uids: Option<&[u32]>) -> Outcome<()> {
        let command = match uids {
            Some(uids) => Command::plain(format!("UID EXPUNGE {}", uid_set(uids))),
            None => Command::plain("EXPUNGE"),
        };
        self.run("removing deleted mail", &command).await
    }

    /// `STATUS <folder> (UIDVALIDITY UIDNEXT)`: where the next copy made
    /// in a folder will be. `None` when the server will not say.
    pub(super) async fn uid_next(&mut self, name: &str) -> Outcome<Option<(u32, u32)>> {
        let Some(command) =
            Command::with_mailbox("STATUS", name).map(|c| c.then(" (UIDVALIDITY UIDNEXT)"))
        else {
            return Ok(None);
        };
        let (mut validity, mut next) = (None, None);
        let done = self
            .command("asking where new mail goes", &command, |response| {
                if let Response::MailboxData(MailboxDatum::Status { status, .. }) = response {
                    for attribute in status {
                        match attribute {
                            StatusAttribute::UidValidity(v) => validity = Some(*v),
                            StatusAttribute::UidNext(n) => next = Some(*n),
                            _ => {}
                        }
                    }
                }
                Ok(())
            })
            .await?;
        Ok(match done {
            Done::Ok(_) => validity.zip(next),
            Done::No(_) | Done::FolderBusy(_) => None,
        })
    }

    /// The flags, dates and headers of the selected folder's copies from
    /// UID `from` on, to find the copies a command made without saying
    /// where (no UIDPLUS).
    pub(super) async fn copies_from(&mut self, from: u32) -> Outcome<FetchReply> {
        self.headers(&format!("{from}:*")).await
    }

    /// The flags, dates and headers of these copies in the selected folder.
    pub(super) async fn headers_of(&mut self, uids: &[u32]) -> Outcome<FetchReply> {
        self.headers(&uid_set(uids)).await
    }

    async fn headers(&mut self, set: &str) -> Outcome<FetchReply> {
        let server = self.describe();
        self.uid_fetch(
            set,
            "(UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER])",
            None,
        )
        .await
        .map_err(|e| Failure::in_folder(e, server))
    }

    /// A command on folders (`CREATE`, `RENAME`, `DELETE`, `SUBSCRIBE`)
    /// that answers nothing but how it went.
    pub(super) async fn folder_command(&mut self, what: &str, command: &Command) -> Outcome<()> {
        self.run(what, command).await
    }

    async fn run(&mut self, what: &str, command: &Command) -> Outcome<()> {
        let done = self.command(what, command, |_| Ok(())).await?;
        self.ended(what, done)
    }

    fn ended(&mut self, what: &str, done: Done) -> Outcome<()> {
        match done {
            Done::Ok(_) => Ok(()),
            Done::No(no) => Err(refusal(what, &no)),
            Done::FolderBusy(message) => Err(Failure::Wait(crate::SyncError::Busy {
                service: "IMAP",
                server: self.describe(),
                message,
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copyuid_pairs_ranges_in_order() {
        let from = [UidSetMember::UidRange(3..=5), UidSetMember::Uid(9)];
        let to = [UidSetMember::Uid(20), UidSetMember::UidRange(21..=23)];
        let copied = CopyUid::of(7, &from, &to);
        assert_eq!(copied.uidvalidity, 7);
        assert_eq!(copied.pairs, [(3, 20), (4, 21), (5, 22), (9, 23)]);
    }

    #[test]
    fn names_are_shown_decoded() {
        assert_eq!(shown("Entw&APw-rfe"), "\"Entwürfe\"");
    }
}
