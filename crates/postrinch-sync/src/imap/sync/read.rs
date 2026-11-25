//! Reading one selected folder: whole, when its UIDVALIDITY is new, or
//! what changed since the last pass (docs/imap.md, "Sync"). What is read
//! here are UIDs and flags; the copies to fetch by their headers are
//! handed back for [`super::headers`].
//!
//! Memory stays bounded on big folders: a folder's UIDs are listed by
//! `UID SEARCH` (four bytes a message), flags are read [`FLAG_BATCH`]
//! UIDs at a time and applied as each batch comes, so no listing of a
//! whole folder's flags is held, and `VANISHED` is kept as ranges. The
//! places kept for the folder are held once while it is read, as they
//! are compared with every batch: they are the store's own records, a
//! few dozen bytes a copy.
//!
//! A command the server refuses (`NO`) ends nothing but that command:
//! what it did answer is applied, and the read is marked stale, so the
//! folder's position does not move and the next pass reads the same
//! changes again. Nothing is taken for gone from a listing that did not
//! come. Mail at or above the position that an earlier such pass (or one
//! the server was busy in) already stored is not fetched by its headers
//! again: it is told from new mail by the places kept, as when a folder
//! is read whole, and only its flags are read.

use std::collections::{BTreeMap, HashMap, HashSet};

use postrinch_model::{EmailAddress, ImapFolder, ImapPlace};

use super::ImapSync;
use super::folder_error::FolderResult;
use super::headers::folder;
use super::pending;
use super::places::{self, Applied, FlagChange};
use super::uids::UidSet;
use super::wire::{Conn, Selected, uid_set};
use crate::SyncResult;
use crate::google::SyncReport;

/// How many UIDs' flags are asked for at once when a folder's flags are
/// read whole (without CONDSTORE, or when it is read from the start).
pub(super) const FLAG_BATCH: usize = 1000;

/// A folder's places of its current UIDVALIDITY, by UID.
type Kept = HashMap<u32, ImapPlace>;

/// What reading a folder found.
#[derive(Debug, Default)]
pub(super) struct Read {
    /// The highest UID of the mail read (all of it, or the new mail).
    pub highest: Option<u32>,
    /// The UIDs to fetch by their headers, each with how many passes it
    /// went unanswered in before (0 for one asked for the first time).
    pub fetch: BTreeMap<u32, u32>,
    /// Whether the server refused a command: the folder's position must
    /// not move.
    pub stale: bool,
    /// Copies of an earlier UIDVALIDITY, to forget once the folder's
    /// copies are stored (so mail still there keeps its body).
    pub replaced: Vec<ImapPlace>,
}

impl Read {
    /// Asks for these UIDs by their headers, those unanswered before with
    /// their counts (`silent`).
    fn want(&mut self, uids: impl IntoIterator<Item = u32>, silent: &BTreeMap<u32, u32>) {
        for uid in uids {
            let tries = silent.get(&uid).copied().unwrap_or(0);
            self.fetch.insert(uid, tries);
        }
    }
}

impl ImapSync {
    /// Reads a folder from the start. `silent` holds the UIDs earlier
    /// passes asked for in vain, with how many times.
    pub(super) async fn read_whole(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        state: &ImapFolder,
        selected: &Selected,
        silent: &BTreeMap<u32, u32>,
        report: &mut SyncReport,
    ) -> FolderResult<Read> {
        let mut read = Read::default();
        let uids = if selected.exists == 0 {
            Vec::new()
        } else {
            match conn.uid_search("ALL").await? {
                Some(uids) => uids,
                None => {
                    read.stale = true;
                    return Ok(read);
                }
            }
        };
        let (a, s) = (account.clone(), state.clone());
        let (kept, replaced) = self
            .blocking(move |store| places::folder_places(store, &a, folder(&s)))
            .await?;
        let here: HashSet<u32> = uids.iter().copied().collect();
        let kept = self
            .forget_gone(account, kept, |uid| !here.contains(&uid))
            .await?;
        let (_, applied, stale) = self.read_flags(conn, account, &uids, kept).await?;
        report.relabelled += applied.relabelled;
        read.want(applied.fetch, silent);
        read.highest = uids.last().copied();
        read.stale = stale;
        read.replaced = replaced;
        Ok(read)
    }

    /// Reads what changed in a folder since its last pass, as
    /// [`Self::read_whole`].
    pub(super) async fn read_changes(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        state: &ImapFolder,
        selected: &Selected,
        silent: &BTreeMap<u32, u32>,
        report: &mut SyncReport,
    ) -> FolderResult<Read> {
        let mut read = Read::default();
        let known_end = state.uidnext - 1;
        let (a, s) = (account.clone(), state.clone());
        let (kept, _) = self
            .blocking(move |store| places::folder_places(store, &a, folder(&s)))
            .await?;
        // New mail, and mail at or above the position stored by a pass
        // that did not move it.
        let new = self.read_new(conn, account, state, selected, kept).await?;
        let kept = new.kept;
        read.highest = new.highest;
        read.stale |= new.stale;
        report.relabelled += new.relabelled;
        read.want(new.fetch, silent);
        // Mail asked for in vain before, if it is still there.
        if selected.exists > 0 && !silent.is_empty() {
            let asked: Vec<u32> = silent.keys().copied().collect();
            match conn.uid_search(&format!("UID {}", uid_set(&asked))).await? {
                Some(here) => read.want(here, silent),
                None => {
                    read.stale = true;
                    // Asked for again, as nothing says they went.
                    read.want(asked, silent);
                }
            }
        }
        // Changed flags, and mail gone.
        let applied = if known_end == 0 {
            // Nothing was kept before: there is nothing to compare.
            Applied::default()
        } else if selected.exists == 0 {
            self.forget_gone(account, kept, |uid| uid <= known_end)
                .await?;
            Applied::default()
        } else if conn.condstore() && state.highest_modseq > 0 && selected.highest_modseq.is_some()
        {
            self.read_condstore(conn, account, state, selected, kept, &mut read)
                .await?
        } else {
            match conn.uid_search(&format!("UID 1:{known_end}")).await? {
                None => {
                    read.stale = true;
                    Applied::default()
                }
                Some(uids) => {
                    let here: HashSet<u32> = uids.iter().copied().collect();
                    let kept = self
                        .forget_gone(account, kept, |uid| {
                            uid <= known_end && !here.contains(&uid)
                        })
                        .await?;
                    let (_, applied, stale) = self.read_flags(conn, account, &uids, kept).await?;
                    read.stale |= stale;
                    applied
                }
            }
        };
        report.relabelled += applied.relabelled;
        read.want(
            applied.fetch.into_iter().filter(|&uid| uid <= known_end),
            silent,
        );
        Ok(read)
    }

    /// Lists the mail at or above the folder's position (`UIDNEXT`):
    /// what is not kept is to be fetched by its headers, and what is (an
    /// earlier pass stored it, then could not move the position) only
    /// has its flags read. Kept places there that the listing does not
    /// name are gone.
    async fn read_new(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        state: &ImapFolder,
        selected: &Selected,
        kept: Kept,
    ) -> FolderResult<New> {
        let from = state.uidnext;
        if selected.exists == 0 {
            // Nothing at or above the position is on the server.
            let kept = self.forget_gone(account, kept, |uid| uid >= from).await?;
            return Ok(New::none(kept));
        }
        // Listed only when UIDNEXT says there is new mail, or a copy is
        // kept up there to be told present or gone (by the listing, not
        // by UIDNEXT alone).
        let more = selected.uidnext.is_none_or(|next| next > from);
        if !more && !kept.keys().any(|&uid| uid >= from) {
            return Ok(New::none(kept));
        }
        let listing = conn.uid_fetch(&format!("{from}:*"), "(UID)", None).await?;
        let listed: Vec<u32> = listing
            .messages
            .iter()
            .map(|m| m.uid)
            .filter(|&uid| uid >= from)
            .collect();
        let highest = listed.iter().copied().max();
        let mut stale = listing.refused.is_some();
        let kept = if stale {
            kept
        } else {
            let here: HashSet<u32> = listed.iter().copied().collect();
            self.forget_gone(account, kept, |uid| uid >= from && !here.contains(&uid))
                .await?
        };
        let (stored, mut fetch): (Vec<u32>, Vec<u32>) =
            listed.into_iter().partition(|uid| kept.contains_key(uid));
        if stored.is_empty() {
            return Ok(New {
                kept,
                highest,
                fetch,
                stale,
                relabelled: 0,
            });
        }
        let (kept, applied, refused) = self.read_flags(conn, account, &stored, kept).await?;
        stale |= refused;
        // A copy kept without its message (it went from the store) is
        // fetched again, as anywhere else.
        fetch.extend(applied.fetch.into_iter().filter(|&uid| uid >= from));
        Ok(New {
            kept,
            highest,
            fetch,
            stale,
            relabelled: applied.relabelled,
        })
    }

    /// Reads changed flags by `CHANGEDSINCE`, and gone mail by QRESYNC's
    /// `VANISHED` (or by listing the folder's UIDs without it).
    async fn read_condstore(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        state: &ImapFolder,
        selected: &Selected,
        kept: Kept,
        read: &mut Read,
    ) -> FolderResult<Applied> {
        let known_end = state.uidnext - 1;
        let mut changes = Vec::new();
        let mut gone = UidSet::default();
        if selected.highest_modseq != Some(state.highest_modseq) {
            let mut modifiers = format!("CHANGEDSINCE {}", state.highest_modseq);
            if conn.qresync() {
                modifiers.push_str(" VANISHED");
            }
            let reply = conn
                .uid_fetch(
                    &format!("1:{known_end}"),
                    "(UID FLAGS MODSEQ)",
                    Some(&modifiers),
                )
                .await?;
            read.stale |= reply.refused.is_some();
            changes.extend(reply.messages.iter().map(FlagChange::of));
            gone = reply.vanished;
        }
        let present = if conn.qresync() {
            None
        } else {
            match conn.uid_search(&format!("UID 1:{known_end}")).await? {
                Some(uids) => Some(uids.into_iter().collect::<HashSet<u32>>()),
                None => {
                    read.stale = true;
                    None
                }
            }
        };
        let kept = self
            .forget_gone(account, kept, |uid| {
                uid <= known_end
                    && (gone.contains(uid) || present.as_ref().is_some_and(|p| !p.contains(&uid)))
            })
            .await?;
        Ok(self.apply(account, kept, changes).await?.1)
    }

    /// Forgets the kept places whose UID `gone` names; returns the rest.
    async fn forget_gone(
        &self,
        account: &EmailAddress,
        mut kept: Kept,
        gone: impl Fn(u32) -> bool,
    ) -> SyncResult<Kept> {
        let uids: Vec<u32> = kept.keys().copied().filter(|&uid| gone(uid)).collect();
        if uids.is_empty() {
            return Ok(kept);
        }
        let places: Vec<ImapPlace> = uids.iter().filter_map(|uid| kept.remove(uid)).collect();
        let a = account.clone();
        self.blocking(move |s| pending::forget(s, &a, &places))
            .await?;
        Ok(kept)
    }

    /// Reads the flags of these UIDs (ascending), a batch at a time, and
    /// applies each batch as it comes. Says too whether the server
    /// refused a batch.
    async fn read_flags(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        uids: &[u32],
        mut kept: Kept,
    ) -> FolderResult<(Kept, Applied, bool)> {
        let items = flag_items(conn);
        let mut applied = Applied::default();
        let mut refused = false;
        for batch in uids.chunks(FLAG_BATCH) {
            let (first, last) = (batch[0], batch[batch.len() - 1]);
            let reply = conn
                .uid_fetch(&format!("{first}:{last}"), items, None)
                .await?;
            refused |= reply.refused.is_some();
            let changes = reply.messages.iter().map(FlagChange::of).collect();
            let (k, more) = self.apply(account, kept, changes).await?;
            kept = k;
            applied.add(more);
        }
        Ok((kept, applied, refused))
    }

    async fn apply(
        &self,
        account: &EmailAddress,
        mut kept: Kept,
        changes: Vec<FlagChange>,
    ) -> SyncResult<(Kept, Applied)> {
        if changes.is_empty() {
            return Ok((kept, Applied::default()));
        }
        let a = account.clone();
        self.blocking(move |s| {
            let applied = places::apply_flags(s, &a, &mut kept, &changes)?;
            Ok((kept, applied))
        })
        .await
    }
}

/// What listing the mail at or above a folder's position found.
struct New {
    /// The kept places left, those found gone forgotten.
    kept: Kept,
    /// The highest UID listed.
    highest: Option<u32>,
    /// The UIDs to fetch by their headers.
    fetch: Vec<u32>,
    /// Whether the server refused a command.
    stale: bool,
    /// How many stored messages changed by their flags.
    relabelled: usize,
}

impl New {
    /// Nothing at or above the position.
    fn none(kept: Kept) -> Self {
        Self {
            kept,
            highest: None,
            fetch: Vec::new(),
            stale: false,
            relabelled: 0,
        }
    }
}

/// The items that list a folder's UIDs and flags.
fn flag_items(conn: &Conn) -> &'static str {
    if conn.condstore() {
        "(UID FLAGS MODSEQ)"
    } else {
        "(UID FLAGS)"
    }
}
