//! Copying and moving copies between folders (`UID COPY`, `UID MOVE`), and
//! keeping where the copies went (docs/imap.md, "Keeping every message
//! safe").
//!
//! The destination's UIDVALIDITY and UIDNEXT are read first (`STATUS`)
//! and an attempt written for each message ([`super::attempts`]), so that
//! a change sent again, whatever happened to the answer, never makes the
//! copy twice. The places the command makes are recorded at once from
//! `COPYUID` (UIDPLUS); those it did not name are looked for among the
//! destination's new mail by `Message-ID`. Each transfer says what became
//! of each message's copy ([`Made`]): only a confirmed one lets the
//! deletes that count on it go ([`super::apply`]).
//!
//! A move without `MOVE` is not made here: it is a copy and then a delete
//! of the source, which goes as any delete does.

use postrinch_model::{EmailAddress, ImapPlace, ProviderMessageId};

use super::super::ImapSync;
use super::apply::{Exec, known, uids};
use super::attempts::{self, Made};
use super::commands::{CHUNK, CopyUid, shown};
use super::failure::{Failure, Outcome};
use super::plan::Folders;
use super::record;

impl ImapSync {
    /// Copies (or, `moving`, moves with `UID MOVE`) copies from the folder
    /// `from` into the folder `to`, as the module says, and says what
    /// became of each message's copy.
    pub(super) async fn transfer(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &Folders,
        (from, to): (&str, &str),
        places: Vec<ImapPlace>,
        moving: bool,
    ) -> Outcome<Made> {
        let dest = known(folders, to)?.name.clone();
        let Some(before) = exec.conn.uid_next(&dest).await? else {
            return Err(Failure::Declined(format!(
                "the server would not say where new mail goes in {}, which it must for \
                 the copies made there to be found",
                shown(&dest)
            )));
        };
        let validity = exec.open(folders, from).await?;
        let places = exec.current(folders, validity, places);
        let mut made = Made::default();
        if places.is_empty() {
            return Ok(made);
        }
        let (a, label, written) = (account.clone(), to.to_owned(), places.clone());
        self.blocking(move |s| attempts::write(s, &a, &label, before, &written))
            .await?;
        for chunk in places.chunks(CHUNK) {
            if let Some(copied) = exec.conn.transfer(&uids(chunk), &dest, moving).await? {
                made.landed
                    .extend(self.keep_copies(account, to, chunk, copied).await?);
            }
        }
        if moving {
            // `UID MOVE` answered OK: the sources are not there, moved or
            // expunged before it came.
            self.forget_copies(account, &places).await?;
        }
        let rest: Vec<ProviderMessageId> = places
            .iter()
            .map(|p| p.message.clone())
            .filter(|m| !made.landed.contains(m))
            .collect();
        if !rest.is_empty() {
            let looked = self
                .look_for(exec, account, folders, to, before, &rest)
                .await?;
            made.add(looked);
        }
        Ok(made)
    }

    /// Keeps the copies `COPYUID` says were made of `chunk` in `to`,
    /// settling their attempts, and returns their messages.
    async fn keep_copies(
        &self,
        account: &EmailAddress,
        to: &str,
        chunk: &[ImapPlace],
        copied: CopyUid,
    ) -> Outcome<Vec<ProviderMessageId>> {
        let (a, to, chunk) = (account.clone(), to.to_owned(), chunk.to_vec());
        let landed = self
            .blocking(move |s| {
                let mut landed = Vec::new();
                for (source, uid) in copied.pairs {
                    if let Some(place) = chunk.iter().find(|p| p.uid == source) {
                        record::add_copy(s, &a, place, &to, copied.uidvalidity, uid)?;
                        s.remove_imap_copy_attempt(&a, &to, &place.message)?;
                        landed.push(place.message.clone());
                    }
                }
                Ok(landed)
            })
            .await?;
        Ok(landed)
    }
}
