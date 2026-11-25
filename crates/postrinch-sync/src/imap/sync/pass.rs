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
//! stored, so an interrupted pass repeats work and never skips mail.
//! Messages and folders that went are removed only at the end of a
//! complete pass ([`super::pending`]).

use std::collections::{BTreeSet, HashSet};

use postrinch_model::{EmailAddress, ImapFolder};

use super::ImapSync;
use super::folders::{Planned, plan};
use super::names::sendable;
use super::pending;
use super::wire::Conn;
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
        let roles = self.settings(account).await?.roles;
        let a = account.clone();
        let stored = self.with_store(move |s| s.imap_folders(&a)).await?;
        let renames = self.renames(conn, &listed, &stored).await?;
        let planned = plan(&listed, &stored, &roles, &renames);
        self.keep_folders(account, &planned).await?;
        let mut complete = true;
        for (i, folder) in planned.iter().enumerate() {
            progress(Progress::Working {
                phase: Phase::Changes,
                done: i,
                total: planned.len(),
            });
            complete &= self
                .folder(conn, account, folder.state.clone(), progress, &mut report)
                .await?;
        }
        let a = account.clone();
        let keep: HashSet<String> = planned
            .iter()
            .map(|p| p.label.provider_id.clone())
            .collect();
        report.removed += self
            .blocking(move |s| finish(s, &a, &keep, number, complete))
            .await?;
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
    /// the server refused to open it, or did not answer for every message
    /// asked for.
    async fn folder(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        mut state: ImapFolder,
        progress: &ProgressFn,
        report: &mut SyncReport,
    ) -> SyncResult<bool> {
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
        let whole = state.uidnext == 0;
        let read = if whole {
            report.full = true;
            self.read_whole(conn, account, &state, &selected, progress, report)
                .await?
        } else {
            self.read_changes(conn, account, &state, &selected, progress, report)
                .await?
        };
        let next = selected.uidnext.unwrap_or(0).max(
            read.highest
                .map_or(state.uidnext, |uid| uid.saturating_add(1)),
        );
        state.uidnext = match read.unanswered {
            None => next,
            // Read whole again next time: only what is missing is fetched.
            Some(_) if whole => 0,
            // From the first message missing, so the next pass asks again.
            Some(missing) => next.min(missing),
        };
        state.highest_modseq = selected.highest_modseq.unwrap_or(0);
        self.put_folder(account, &state).await?;
        Ok(read.unanswered.is_none())
    }

    async fn put_folder(&self, account: &EmailAddress, state: &ImapFolder) -> SyncResult<()> {
        let (a, state) = (account.clone(), state.clone());
        self.with_store(move |s| s.put_imap_folder(&a, &state))
            .await
    }
}

/// The end of pass `pass`, which read the folders `keep`, every one of
/// them whole if `complete`: folders missing from LIST twice go, and, if
/// the pass was complete, messages that stayed without a copy since an
/// earlier pass go. Returns how many messages went.
fn finish(
    store: &postrinch_store::Store,
    account: &EmailAddress,
    keep: &HashSet<String>,
    pass: u64,
    complete: bool,
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
    let gone = pending::note_missing(store, account, &missing, pass)?;
    if !complete || gone.len() < missing.len() {
        // A folder could not be read, or is missing for the first time:
        // what seems gone may be there.
        return Ok(0);
    }
    for id in &gone {
        pending::forget_folder(store, account, id)?;
        if labels.contains(id) {
            store.remove_label(account, id)?;
        }
        if folders.contains(id) {
            store.remove_imap_folder(account, id)?;
        }
    }
    pending::resolve(store, account, pass)
}
