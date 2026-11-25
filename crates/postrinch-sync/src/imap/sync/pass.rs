//! One pass over an account: the folders, then each folder's mail.
//!
//! Per folder, after `SELECT`:
//! - a UIDVALIDITY that is new (or 0, never read) starts the folder over:
//!   every UID is listed with its flags, the copies not kept are fetched
//!   by their headers, newest first, and only then are the copies of the
//!   earlier UIDVALIDITY forgotten, so mail that is still there keeps its
//!   body;
//! - otherwise new mail is fetched from the UIDNEXT read last time, flags
//!   by `CHANGEDSINCE` the HIGHESTMODSEQ read last time (or all of them
//!   without CONDSTORE), and gone mail found by QRESYNC's `VANISHED` (or
//!   by comparing the UIDs kept with the folder's) ([`super::read`]).
//!
//! A folder's position is written only once everything before it is
//! stored, so an interrupted pass repeats work and never skips mail, and
//! it is not moved when the server refused a command in the folder. A
//! message the server lists but gives no headers for does not hold the
//! position back: it is asked for again by its UID ([`super::reads`]).
//! Messages that went are removed once every folder has been read whole
//! since, and folders once two LISTs left them out ([`super::pending`]).
//!
//! A server busy for one folder (`NO [INUSE] Mailbox is locked`) stops
//! only that folder: it counts as not read this pass, as a refused
//! `SELECT` does, and the pass goes on to the others. A server busy for
//! the account ([`crate::SyncError::Busy`]) stops the pass
//! ([`crate::imap::refusal::BusyScope`]).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use postrinch_model::{EmailAddress, ImapFolder};

use super::ImapSync;
use super::folder_error::{FolderError, FolderResult};
use super::folders::{GMAIL, Planned, plan, view_of_all};
use super::names::sendable;
use super::wire::Conn;
use super::{changes, pending, reads};
use crate::SyncResult;
use crate::google::{Phase, Progress, ProgressFn, SyncReport};

impl ImapSync {
    pub(super) async fn pass(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        progress: &ProgressFn,
    ) -> SyncResult<SyncReport> {
        let mut report = SyncReport::default();
        let a = account.clone();
        let number = self.blocking(move |s| pending::begin_pass(s, &a)).await?;
        let mut listed = conn.session_mut().list_folders().await?;
        listed.retain(|folder| {
            let ok = sendable(&folder.name);
            if !ok {
                tracing::warn!(
                    "IMAP: {account} has a folder whose name cannot be sent back to the \
                     server; it is not synced: {:?}",
                    folder.name
                );
            }
            ok
        });
        let gmail = conn.offers(GMAIL);
        listed.retain(|folder| !view_of_all(folder, gmail));
        let roles = self.settings(account).await?.roles;
        let a = account.clone();
        let stored = self.with_store(move |s| s.imap_folders(&a)).await?;
        let renames = self.renames(conn, &listed, &stored).await?;
        let planned = plan(&listed, &stored, &roles, &renames);
        self.keep_folders(account, &planned).await?;
        for (i, folder) in planned.iter().enumerate() {
            progress(Progress::Working {
                phase: Phase::Changes,
                done: i,
                total: planned.len(),
            });
            let state = folder.state.clone();
            let whole = match self
                .folder(conn, account, state, progress, &mut report)
                .await
            {
                Ok(whole) => whole,
                Err(FolderError::Busy(message)) => {
                    tracing::info!(
                        "IMAP: {} is busy for the folder {:?} of {account}; it is read \
                         next pass: {message}",
                        conn.describe(),
                        folder.state.name
                    );
                    false
                }
                Err(FolderError::Sync(e)) => return Err(e),
            };
            let (a, label) = (account.clone(), folder.state.label_id.clone());
            self.blocking(move |s| reads::note(s, &a, &label, number, whole))
                .await?;
        }
        let a = account.clone();
        let keep: HashSet<String> = planned
            .iter()
            .map(|p| p.label.provider_id.clone())
            .collect();
        report.removed += self.blocking(move |s| finish(s, &a, &keep, number)).await?;
        Ok(report)
    }

    /// Keeps each folder as a label with its state, forgetting the copies
    /// kept under a label whose role moved to a folder of another name.
    async fn keep_folders(&self, account: &EmailAddress, planned: &[Planned]) -> SyncResult<()> {
        let (a, planned) = (account.clone(), planned.to_vec());
        self.blocking(move |s| {
            for p in &planned {
                s.put_label(&a, &p.label)?;
                if p.forget_places {
                    pending::forget_folder(s, &a, &p.label.provider_id)?;
                }
                s.put_imap_folder(&a, &p.state)?;
            }
            Ok(())
        })
        .await
    }

    /// Reads one folder. Returns whether it was read whole: `false` when
    /// the server refused to open it or a command in it, or a message in
    /// it went unanswered. A server busy for the folder stops it where it
    /// is ([`FolderError::Busy`]): its position does not move, and no
    /// message counts as unanswered.
    async fn folder(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        mut state: ImapFolder,
        progress: &ProgressFn,
        report: &mut SyncReport,
    ) -> FolderResult<bool> {
        let Some(selected) = conn.select(&state.name).await? else {
            return Ok(false);
        };
        let mut changed = selected.mailbox_id.is_some() && selected.mailbox_id != state.mailbox_id;
        if changed {
            state.mailbox_id = selected.mailbox_id.clone();
        }
        if selected.uidvalidity != state.uidvalidity {
            state.uidvalidity = selected.uidvalidity;
            state.uidnext = 0;
            state.highest_modseq = 0;
            changed = true;
        }
        if changed {
            self.put_folder(account, &state).await?;
        }
        let (a, label, uidvalidity) = (account.clone(), state.label_id.clone(), state.uidvalidity);
        let silent = self
            .blocking(move |s| reads::silent(s, &a, &label, uidvalidity))
            .await?;
        let read = if state.uidnext == 0 {
            report.full = true;
            self.read_whole(conn, account, &state, &selected, &silent, report)
                .await?
        } else {
            self.read_changes(conn, account, &state, &selected, &silent, report)
                .await?
        };
        let asked: HashSet<u32> = read.fetch.keys().copied().collect();
        let mut unanswered = self
            .fetch_headers(conn, account, &state, read.fetch, progress, report)
            .await?;
        if read.stale {
            // What was not asked for this time is still owed.
            unanswered.extend(silent.into_iter().filter(|(uid, _)| !asked.contains(uid)));
        }
        let whole = !read.stale && unanswered.is_empty();
        self.keep_silent(account, &state, unanswered).await?;
        if read.stale {
            // Read the same changes again next time.
            return Ok(false);
        }
        let a = account.clone();
        let replaced = read.replaced;
        self.blocking(move |s| pending::forget(s, &a, &replaced))
            .await?;
        state.uidnext = selected.uidnext.unwrap_or(0).max(
            read.highest
                .map_or(state.uidnext, |uid| uid.saturating_add(1)),
        );
        state.highest_modseq = selected.highest_modseq.unwrap_or(0);
        self.put_folder(account, &state).await?;
        Ok(whole)
    }

    async fn keep_silent(
        &self,
        account: &EmailAddress,
        state: &ImapFolder,
        unanswered: BTreeMap<u32, u32>,
    ) -> SyncResult<()> {
        let (a, label, uidvalidity) = (account.clone(), state.label_id.clone(), state.uidvalidity);
        self.blocking(move |s| reads::keep_silent(s, &a, &label, uidvalidity, unanswered))
            .await
    }

    async fn put_folder(&self, account: &EmailAddress, state: &ImapFolder) -> SyncResult<()> {
        let (a, state) = (account.clone(), state.clone());
        self.with_store(move |s| s.put_imap_folder(&a, &state))
            .await
    }
}

/// The end of pass `pass`, which listed the folders `keep`: folders
/// missing from LIST twice go, and messages that stayed without a copy
/// since a pass before every folder was last read whole go. Returns how
/// many messages went.
fn finish(
    store: &postrinch_store::Store,
    account: &EmailAddress,
    keep: &HashSet<String>,
    pass: u64,
) -> SyncResult<usize> {
    let labels: BTreeSet<String> = store
        .labels(account)?
        .into_iter()
        .map(|l| l.provider_id)
        .collect();
    let folders: BTreeSet<String> = store
        .imap_folders(account)?
        .into_iter()
        .map(|f| f.label_id)
        .collect();
    let missing: BTreeSet<String> = labels
        .union(&folders)
        .filter(|id| !keep.contains(*id))
        .cloned()
        .collect();
    let gone: BTreeSet<String> = pending::note_missing(store, account, &missing, pass)?
        .into_iter()
        .collect();
    for id in &gone {
        pending::forget_folder(store, account, id)?;
        if labels.contains(id) {
            store.remove_label(account, id)?;
        }
        if folders.contains(id) {
            store.remove_imap_folder(account, id)?;
        }
    }
    // Missing for the first time: it may be there, unread.
    let unread: BTreeSet<String> = missing.difference(&gone).cloned().collect();
    let mut known = unread.clone();
    known.extend(keep.iter().cloned());
    let floor = reads::settle(store, account, &known, &unread, pass)?;
    let removed = pending::resolve(store, account, floor)?;
    changes::sweep_attempts(store, account)?;
    Ok(removed)
}
