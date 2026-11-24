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
//!   by comparing the UIDs kept with the folder's).
//!
//! A folder's position is written only once everything before it is
//! stored, so an interrupted pass repeats work and never skips mail.

use std::collections::HashSet;

use postrinch_model::{EmailAddress, ImapFolder, ImapPlace};

use super::ImapSync;
use super::folders::{Planned, plan};
use super::places::{self, FlagChange, Folder, Stored};
use super::wire::{Conn, Fetched, Selected, uid_set};
use crate::SyncResult;
use crate::google::{Phase, Progress, ProgressFn, SyncReport};

/// How many messages' headers are asked for at once.
const HEADER_BATCH: usize = 100;

impl ImapSync {
    pub(super) async fn pass(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        progress: &ProgressFn,
    ) -> SyncResult<SyncReport> {
        let mut report = SyncReport::default();
        let listed = conn.session_mut().list_folders().await?;
        let roles = self.settings(account).await?.roles;
        let a = account.clone();
        let stored = self.with_store(move |s| s.imap_folders(&a)).await?;
        let planned = plan(&listed, &stored, &roles);
        self.keep_folders(account, &planned).await?;
        for (i, folder) in planned.iter().enumerate() {
            progress(Progress::Working {
                phase: Phase::Changes,
                done: i,
                total: planned.len(),
            });
            self.folder(conn, account, folder.state.clone(), progress, &mut report)
                .await?;
        }
        let a = account.clone();
        let keep: HashSet<String> = planned
            .iter()
            .map(|p| p.label.provider_id.clone())
            .collect();
        report.removed += self
            .blocking(move |s| {
                // Folders gone from the server go once every folder is
                // read, so mail that moved with a rename keeps its body.
                for label in s.labels(&a)? {
                    if !keep.contains(&label.provider_id) {
                        places::forget_folder(s, &a, &label.provider_id)?;
                        s.remove_label(&a, &label.provider_id)?;
                    }
                }
                for folder in s.imap_folders(&a)? {
                    if !keep.contains(&folder.label_id) {
                        places::forget_folder(s, &a, &folder.label_id)?;
                        s.remove_imap_folder(&a, &folder.label_id)?;
                    }
                }
                places::resolve_pending(s, &a)
            })
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
                    places::forget_folder(s, &a, &p.label.provider_id)?;
                }
                s.put_imap_folder(&a, &p.state)?;
            }
            Ok(())
        })
        .await
    }

    async fn folder(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        mut state: ImapFolder,
        progress: &ProgressFn,
        report: &mut SyncReport,
    ) -> SyncResult<()> {
        let Some(selected) = conn.select(&state.name).await? else {
            return Ok(());
        };
        if selected.uidvalidity != state.uidvalidity {
            state.uidvalidity = selected.uidvalidity;
            state.uidnext = 0;
            state.highest_modseq = 0;
            self.put_folder(account, &state).await?;
        }
        let highest_seen = if state.uidnext == 0 {
            report.full = true;
            self.read_whole(conn, account, &state, &selected, progress, report)
                .await?
        } else {
            self.read_changes(conn, account, &state, &selected, progress, report)
                .await?
        };
        state.uidnext = selected
            .uidnext
            .unwrap_or(0)
            .max(highest_seen.map_or(state.uidnext, |uid| uid.saturating_add(1)));
        state.highest_modseq = selected.highest_modseq.unwrap_or(0);
        self.put_folder(account, &state).await
    }

    /// Reads a folder from the start. Returns the highest UID it has.
    async fn read_whole(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        state: &ImapFolder,
        selected: &Selected,
        progress: &ProgressFn,
        report: &mut SyncReport,
    ) -> SyncResult<Option<u32>> {
        let listing = if selected.exists == 0 {
            Vec::new()
        } else {
            conn.uid_fetch("1:*", &flag_items(conn), None)
                .await?
                .messages
        };
        let (a, label, uv) = (account.clone(), state.label_id.clone(), state.uidvalidity);
        let present: Vec<FlagChange> = listing.iter().map(flag_change).collect();
        let highest = present.iter().map(|c| c.uid).max();
        let (stale, applied) = self
            .blocking(move |s| {
                let folder = Folder {
                    label: &label,
                    uidvalidity: uv,
                };
                let (kept, stale) = places::folder_places(s, &a, folder)?;
                let here: HashSet<u32> = present.iter().map(|c| c.uid).collect();
                let gone: Vec<ImapPlace> = kept
                    .into_values()
                    .filter(|p| !here.contains(&p.uid))
                    .collect();
                places::forget(s, &a, &gone)?;
                let applied = places::apply_flags(s, &a, folder, &present)?;
                Ok((stale, applied))
            })
            .await?;
        report.relabelled += applied.relabelled;
        self.fetch_headers(conn, account, state, applied.fetch, progress, report)
            .await?;
        let a = account.clone();
        self.blocking(move |s| places::forget(s, &a, &stale))
            .await?;
        Ok(highest)
    }

    /// Reads what changed in a folder since its last pass. Returns the
    /// highest UID of the new mail.
    async fn read_changes(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        state: &ImapFolder,
        selected: &Selected,
        progress: &ProgressFn,
        report: &mut SyncReport,
    ) -> SyncResult<Option<u32>> {
        let folder_label = state.label_id.clone();
        let uv = state.uidvalidity;
        let known_end = state.uidnext - 1;
        // New mail.
        let mut fetch = Vec::new();
        let mut highest = None;
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
            highest = fetch.iter().copied().max();
        }
        // Changed flags, and mail gone.
        let mut changes = Vec::new();
        let mut gone_uids = Vec::new();
        let mut present = None;
        if known_end == 0 {
            // Nothing was kept before: there is nothing to compare.
        } else if selected.exists == 0 {
            present = Some(HashSet::new());
        } else if conn.condstore() && state.highest_modseq > 0 && selected.highest_modseq.is_some()
        {
            let unchanged = selected.highest_modseq == Some(state.highest_modseq);
            if !unchanged {
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
                changes.extend(reply.messages.iter().map(flag_change));
                gone_uids = reply.vanished;
            }
            if !conn.qresync() {
                let uids = conn.uid_search(&format!("UID 1:{known_end}")).await?;
                present = Some(uids.into_iter().collect());
            }
        } else {
            let reply = conn
                .uid_fetch(&format!("1:{known_end}"), &flag_items(conn), None)
                .await?;
            present = Some(reply.messages.iter().map(|m| m.uid).collect());
            changes.extend(reply.messages.iter().map(flag_change));
        }
        let a = account.clone();
        let applied = self
            .blocking(move |s| {
                let folder = Folder {
                    label: &folder_label,
                    uidvalidity: uv,
                };
                let (kept, _) = places::folder_places(s, &a, folder)?;
                let gone_set: HashSet<u32> = gone_uids.into_iter().collect();
                let gone: Vec<ImapPlace> = kept
                    .into_values()
                    .filter(|p| p.uid <= known_end)
                    .filter(|p| {
                        gone_set.contains(&p.uid)
                            || present.as_ref().is_some_and(|here| !here.contains(&p.uid))
                    })
                    .collect();
                places::forget(s, &a, &gone)?;
                places::apply_flags(s, &a, folder, &changes)
            })
            .await?;
        report.relabelled += applied.relabelled;
        fetch.extend(applied.fetch.into_iter().filter(|&uid| uid <= known_end));
        self.fetch_headers(conn, account, state, fetch, progress, report)
            .await?;
        Ok(highest)
    }

    /// Fetches and stores copies by their headers, newest first.
    async fn fetch_headers(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        state: &ImapFolder,
        mut uids: Vec<u32>,
        progress: &ProgressFn,
        report: &mut SyncReport,
    ) -> SyncResult<()> {
        uids.sort_unstable_by(|a, b| b.cmp(a));
        uids.dedup();
        let total = uids.len();
        let items = if conn.condstore() {
            "(UID FLAGS MODSEQ RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER])"
        } else {
            "(UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER])"
        };
        for (i, batch) in uids.chunks(HEADER_BATCH).enumerate() {
            progress(Progress::Working {
                phase: Phase::Headers,
                done: i * HEADER_BATCH,
                total,
            });
            let fetched = conn.uid_fetch(&uid_set(batch), items, None).await?.messages;
            let (a, label, uv) = (account.clone(), state.label_id.clone(), state.uidvalidity);
            let stored = self
                .blocking(move |s| {
                    store_batch(
                        s,
                        &a,
                        Folder {
                            label: &label,
                            uidvalidity: uv,
                        },
                        fetched,
                    )
                })
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
        Ok(())
    }

    async fn put_folder(&self, account: &EmailAddress, state: &ImapFolder) -> SyncResult<()> {
        let (a, state) = (account.clone(), state.clone());
        self.with_store(move |s| s.put_imap_folder(&a, &state))
            .await
    }
}

/// Stores the copies of a batch, newest first.
fn store_batch(
    store: &postrinch_store::Store,
    account: &EmailAddress,
    folder: Folder<'_>,
    mut fetched: Vec<Fetched>,
) -> SyncResult<Vec<Stored>> {
    fetched.sort_unstable_by_key(|f| std::cmp::Reverse(f.uid));
    fetched
        .iter()
        .map(|f| places::store_copy(store, account, folder, f))
        .collect()
}

/// The items that list a folder's UIDs and flags.
fn flag_items(conn: &Conn) -> String {
    if conn.condstore() {
        "(UID FLAGS MODSEQ)".into()
    } else {
        "(UID FLAGS)".into()
    }
}

fn flag_change(fetched: &Fetched) -> FlagChange {
    FlagChange {
        uid: fetched.uid,
        flags: fetched.flags.clone().unwrap_or_default(),
        modseq: fetched.modseq,
    }
}
