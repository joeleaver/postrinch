//! Reading one selected folder: whole, when its UIDVALIDITY is new, or
//! what changed since the last pass (docs/imap.md, "Sync").
//!
//! Memory stays bounded on big folders: a folder's UIDs are listed by
//! `UID SEARCH` (four bytes a message), and flags are read
//! [`FLAG_BATCH`] UIDs at a time and applied as each batch comes, so no
//! listing of a whole folder's flags is held. The places kept for the
//! folder are held once while it is read, as they are compared with every
//! batch: they are the store's own records, a few dozen bytes a copy.

use std::collections::{HashMap, HashSet};

use postrinch_model::{EmailAddress, ImapFolder, ImapPlace};

use super::ImapSync;
use super::pending;
use super::places::{self, Applied, FlagChange, Folder, Stored};
use super::wire::{Conn, Fetched, Selected, uid_set};
use crate::SyncResult;
use crate::google::{Phase, Progress, ProgressFn, SyncReport};

/// How many messages' headers are asked for at once.
const HEADER_BATCH: usize = 100;

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
    /// The lowest UID asked for by its headers that the server did not
    /// answer: the folder is not read whole until it is.
    pub unanswered: Option<u32>,
}

impl ImapSync {
    /// Reads a folder from the start.
    pub(super) async fn read_whole(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        state: &ImapFolder,
        selected: &Selected,
        progress: &ProgressFn,
        report: &mut SyncReport,
    ) -> SyncResult<Read> {
        let uids = if selected.exists == 0 {
            Vec::new()
        } else {
            conn.uid_search("ALL").await?
        };
        let (a, s) = (account.clone(), state.clone());
        let (kept, stale) = self
            .blocking(move |store| places::folder_places(store, &a, folder(&s)))
            .await?;
        let here: HashSet<u32> = uids.iter().copied().collect();
        let kept = self
            .forget_gone(account, kept, |uid| !here.contains(&uid))
            .await?;
        let (_, applied) = self.read_flags(conn, account, &uids, kept).await?;
        report.relabelled += applied.relabelled;
        let unanswered = self
            .fetch_headers(conn, account, state, applied.fetch, progress, report)
            .await?;
        let a = account.clone();
        self.blocking(move |s| pending::forget(s, &a, &stale))
            .await?;
        Ok(Read {
            highest: uids.last().copied(),
            unanswered,
        })
    }

    /// Reads what changed in a folder since its last pass.
    pub(super) async fn read_changes(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        state: &ImapFolder,
        selected: &Selected,
        progress: &ProgressFn,
        report: &mut SyncReport,
    ) -> SyncResult<Read> {
        let known_end = state.uidnext - 1;
        // New mail.
        let mut fetch = Vec::new();
        let more = selected.uidnext.is_none_or(|next| next > state.uidnext);
        if selected.exists > 0 && more {
            let listing = conn
                .uid_fetch(&format!("{}:*", state.uidnext), "(UID)", None)
                .await?;
            fetch.extend(
                listing
                    .messages
                    .iter()
                    .map(|m| m.uid)
                    .filter(|&uid| uid >= state.uidnext),
            );
        }
        let highest = fetch.iter().copied().max();
        // Changed flags, and mail gone.
        let (a, s) = (account.clone(), state.clone());
        let (kept, _) = self
            .blocking(move |store| places::folder_places(store, &a, folder(&s)))
            .await?;
        let mut applied = Applied::default();
        let condstore =
            conn.condstore() && state.highest_modseq > 0 && selected.highest_modseq.is_some();
        if known_end == 0 {
            // Nothing was kept before: there is nothing to compare.
        } else if selected.exists == 0 {
            self.forget_gone(account, kept, |uid| uid <= known_end)
                .await?;
        } else if condstore {
            let mut changes = Vec::new();
            let mut gone = HashSet::new();
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
                changes.extend(reply.messages.iter().map(FlagChange::of));
                gone.extend(reply.vanished);
            }
            let present = if conn.qresync() {
                None
            } else {
                let uids = conn.uid_search(&format!("UID 1:{known_end}")).await?;
                Some(uids.into_iter().collect::<HashSet<u32>>())
            };
            let kept = self
                .forget_gone(account, kept, |uid| {
                    uid <= known_end
                        && (gone.contains(&uid)
                            || present.as_ref().is_some_and(|p| !p.contains(&uid)))
                })
                .await?;
            applied = self.apply(account, kept, changes).await?.1;
        } else {
            let uids = conn.uid_search(&format!("UID 1:{known_end}")).await?;
            let here: HashSet<u32> = uids.iter().copied().collect();
            let kept = self
                .forget_gone(account, kept, |uid| {
                    uid <= known_end && !here.contains(&uid)
                })
                .await?;
            applied = self.read_flags(conn, account, &uids, kept).await?.1;
        }
        report.relabelled += applied.relabelled;
        fetch.extend(applied.fetch.into_iter().filter(|&uid| uid <= known_end));
        let unanswered = self
            .fetch_headers(conn, account, state, fetch, progress, report)
            .await?;
        Ok(Read {
            highest,
            unanswered,
        })
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
    /// applies each batch as it comes.
    async fn read_flags(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        uids: &[u32],
        mut kept: Kept,
    ) -> SyncResult<(Kept, Applied)> {
        let items = flag_items(conn);
        let mut applied = Applied::default();
        for batch in uids.chunks(FLAG_BATCH) {
            let (first, last) = (batch[0], batch[batch.len() - 1]);
            let reply = conn
                .uid_fetch(&format!("{first}:{last}"), items, None)
                .await?;
            let changes = reply.messages.iter().map(FlagChange::of).collect();
            let (k, more) = self.apply(account, kept, changes).await?;
            kept = k;
            applied.add(more);
        }
        Ok((kept, applied))
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

    /// Fetches and stores copies by their headers, newest first. Returns
    /// the lowest UID the server gave no headers for.
    async fn fetch_headers(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        state: &ImapFolder,
        mut uids: Vec<u32>,
        progress: &ProgressFn,
        report: &mut SyncReport,
    ) -> SyncResult<Option<u32>> {
        uids.sort_unstable_by(|a, b| b.cmp(a));
        uids.dedup();
        let total = uids.len();
        let items = if conn.condstore() {
            "(UID FLAGS MODSEQ RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER])"
        } else {
            "(UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER])"
        };
        let mut unanswered: Option<u32> = None;
        for (i, batch) in uids.chunks(HEADER_BATCH).enumerate() {
            progress(Progress::Working {
                phase: Phase::Headers,
                done: i * HEADER_BATCH,
                total,
            });
            let fetched = conn.uid_fetch(&uid_set(batch), items, None).await?.messages;
            let answered: HashSet<u32> = fetched
                .iter()
                .filter(|f| f.header.is_some())
                .map(|f| f.uid)
                .collect();
            if let Some(&missing) = batch.iter().filter(|u| !answered.contains(u)).min() {
                tracing::debug!(
                    "IMAP: no headers came for UID {missing} in {:?} of {account}",
                    state.name
                );
                unanswered = Some(unanswered.map_or(missing, |u| u.min(missing)));
            }
            let (a, s) = (account.clone(), state.clone());
            let stored = self
                .blocking(move |store| store_batch(store, &a, folder(&s), fetched))
                .await?;
            report.headers += stored.iter().filter(|s| **s == Stored::New).count();
            report.relabelled += stored.iter().filter(|s| **s == Stored::Known).count();
        }
        if total > 0 {
            progress(Progress::Working {
                phase: Phase::Headers,
                done: total,
                total,
            });
        }
        Ok(unanswered)
    }
}

/// Where a folder's copies are.
fn folder(state: &ImapFolder) -> Folder<'_> {
    Folder {
        label: &state.label_id,
        name: &state.name,
        uidvalidity: state.uidvalidity,
    }
}

/// Stores the copies of a batch, newest first, but for those the server
/// did not answer with their headers (one that went while the batch was
/// read, say).
fn store_batch(
    store: &postrinch_store::Store,
    account: &EmailAddress,
    folder: Folder<'_>,
    mut fetched: Vec<Fetched>,
) -> SyncResult<Vec<Stored>> {
    fetched.sort_unstable_by_key(|f| std::cmp::Reverse(f.uid));
    let mut stored = Vec::new();
    for f in &fetched {
        let Some(header) = f.header.as_deref() else {
            continue;
        };
        stored.push(places::store_copy(store, account, folder, f, header)?);
    }
    Ok(stored)
}

/// The items that list a folder's UIDs and flags.
fn flag_items(conn: &Conn) -> &'static str {
    if conn.condstore() {
        "(UID FLAGS MODSEQ)"
    } else {
        "(UID FLAGS)"
    }
}
