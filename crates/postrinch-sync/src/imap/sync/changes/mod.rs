//! Sending the outbox of an IMAP account: the changes made here, as
//! `STORE`, `COPY`, `MOVE` and `EXPUNGE` on the copies of each message
//! (docs/imap.md, "Changing mail").
//!
//! The store applies a change at once and keeps it as an outbox entry.
//! This sends what waits, oldest first, over the connection passes use
//! (so a change and a pass never cross in one folder), and marks each
//! entry sent; a sync that began after it was sent and finishes forgets
//! it ([`ImapSync::sync`]). As each command succeeds the store's copies
//! are brought in step with it ([`record`]), so what a later pass reads
//! agrees with what the store already has, and a change sent again
//! after a failure starts from where the last one stopped.
//!
//! Entries in a row that make the same change go together, as Gmail's
//! do, so a thousand archives are a few commands. What becomes of one
//! the server will not take is [`failure`]'s to say. One that waits (for
//! sync, for other mail to go) holds back only the later changes to its
//! messages, so each message's changes keep their order and the rest of
//! the outbox goes on.

mod apply;
mod attempts;
mod change;
mod commands;
mod drop_label;
mod expunge;
mod failure;
mod labels;
mod plan;
mod record;
mod transfer;
mod verify;

use std::collections::HashSet;

use postrinch_model::{EmailAddress, ProviderMessageId};
use postrinch_store::OutboxEntry;

pub(super) use self::attempts::sweep as sweep_attempts;
use self::failure::{Failure, GIVE_UP};
use self::plan::Folders;
use super::folders::GMAIL;
use super::wire::Conn;
use super::{ImapSync, Lane};
use crate::google::SendReport;
use crate::outbox_runs::{merged, runs};
use crate::{SyncError, SyncResult};

/// How many messages one run of identical changes names at most.
const BATCH: usize = 1000;

/// The changes that wait, as sending goes on past them.
#[derive(Default)]
struct Behind {
    /// The messages whose earlier change waits: their later changes wait
    /// behind it.
    messages: HashSet<ProviderMessageId>,
    /// Why the first change that waits does.
    error: Option<SyncError>,
}

impl Behind {
    /// Whether `entry` changes a message whose earlier change waits.
    fn holds(&self, entry: &OutboxEntry) -> bool {
        entry.messages.iter().any(|m| self.messages.contains(m))
    }

    /// `entry` waits, for `error`, and so do the later changes to its
    /// messages.
    fn wait(&mut self, entry: &OutboxEntry, error: Option<SyncError>) {
        self.messages.extend(entry.messages.iter().cloned());
        if self.error.is_none() {
            self.error = error;
        }
    }
}

impl ImapSync {
    /// How many of the account's changes wait to be sent.
    pub async fn waiting(&self, account: &EmailAddress) -> SyncResult<usize> {
        let account = account.clone();
        self.with_store(move |s| {
            Ok(s.outbox(&account)?
                .iter()
                .filter(|e| e.sent.is_none())
                .count())
        })
        .await
    }

    /// Sends the account's waiting changes, oldest first. One the server
    /// refuses for good, or has declined [`GIVE_UP`] times, is dropped
    /// with its reason in the report and its messages shown as the server
    /// has them. One that may pass, or waits for the server, stays to be
    /// tried again, the ones after it wait behind it, and the error is
    /// returned. One that waits for something that comes in time (sync,
    /// other mail to go) stays too, and only the later changes to its
    /// messages wait behind it: the rest are sent, and then its reason is
    /// returned.
    pub async fn send_outbox(&self, account: &EmailAddress) -> SyncResult<SendReport> {
        let owner = account.clone();
        let entries = self.with_store(move |s| s.outbox(&owner)).await?;
        let waiting: Vec<OutboxEntry> = entries.into_iter().filter(|e| e.sent.is_none()).collect();
        let mut report = SendReport::default();
        if waiting.is_empty() {
            return Ok(report);
        }
        let result = self
            .over(account, Lane::Sync, async |conn| {
                let mut folders = self.folders(conn, account).await?;
                self.settle_marks(conn, account, &folders).await?;
                let mut behind = Behind::default();
                for run in runs(waiting, BATCH) {
                    let mut free = Vec::with_capacity(run.len());
                    for entry in run {
                        match behind.holds(&entry) {
                            true => behind.wait(&entry, None),
                            false => free.push(entry),
                        }
                    }
                    if free.is_empty() {
                        continue;
                    }
                    let sending = (&mut report, &mut behind);
                    self.send_run(conn, account, &mut folders, &free, sending)
                        .await?;
                }
                behind.error.map_or(Ok(()), Err)
            })
            .await;
        self.note_sign_in(account, &result).await?;
        result.map(|()| report)
    }

    /// The account's folders as kept, with which hold every message.
    async fn folders(&self, conn: &mut Conn, account: &EmailAddress) -> SyncResult<Folders> {
        let gmail = conn.offers(GMAIL);
        let listed = conn.session_mut().list_folders().await?;
        let a = account.clone();
        let stored = self.with_store(move |s| s.imap_folders(&a)).await?;
        Ok(Folders::new(stored, &listed, gmail))
    }

    /// One run of identical changes: together, or, when the server would
    /// not take them together, each alone, so only the one it refuses is
    /// dropped.
    async fn send_run(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        folders: &mut Folders,
        run: &[OutboxEntry],
        (report, behind): (&mut SendReport, &mut Behind),
    ) -> SyncResult<()> {
        if run.len() == 1 {
            return self
                .send_one(conn, account, folders, &run[0], (report, behind))
                .await;
        }
        let together = merged(run);
        match self.make_change(conn, account, folders, &together).await {
            Ok(()) => {
                let ids: Vec<u64> = run.iter().map(|e| e.id).collect();
                self.with_store(move |s| ids.iter().try_for_each(|id| s.outbox_sent(*id)))
                    .await?;
                report.sent += run.len();
                Ok(())
            }
            Err(Failure::Refused(_) | Failure::Declined(_) | Failure::Waits(_)) => {
                for entry in run {
                    match behind.holds(entry) {
                        true => behind.wait(entry, None),
                        false => {
                            self.send_one(conn, account, folders, entry, (report, behind))
                                .await?;
                        }
                    }
                }
                Ok(())
            }
            Err(Failure::Wait(e)) => {
                let (ids, error) = (
                    run.iter().map(|e| e.id).collect::<Vec<u64>>(),
                    e.to_string(),
                );
                self.with_store(move |s| ids.iter().try_for_each(|id| s.outbox_waits(*id, &error)))
                    .await?;
                Err(e)
            }
        }
    }

    /// One entry, as [`Self::send_outbox`] says.
    async fn send_one(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        folders: &mut Folders,
        entry: &OutboxEntry,
        (report, behind): (&mut SendReport, &mut Behind),
    ) -> SyncResult<()> {
        let id = entry.id;
        let reason = match self.make_change(conn, account, folders, entry).await {
            Ok(()) => {
                self.with_store(move |s| s.outbox_sent(id)).await?;
                report.sent += 1;
                return Ok(());
            }
            Err(Failure::Refused(reason)) => reason,
            Err(Failure::Declined(reason)) if entry.attempts + 1 >= GIVE_UP => {
                format!("{reason} (given up after {GIVE_UP} tries)")
            }
            Err(Failure::Declined(reason)) => {
                let error = reason.clone();
                self.with_store(move |s| s.outbox_failed(id, &error))
                    .await?;
                return Err(server_said(conn, reason));
            }
            Err(Failure::Waits(reason)) => {
                let error = reason.clone();
                self.with_store(move |s| s.outbox_waits(id, &error)).await?;
                behind.wait(entry, Some(server_said(conn, reason)));
                return Ok(());
            }
            Err(Failure::Wait(e)) => {
                let error = e.to_string();
                self.with_store(move |s| s.outbox_waits(id, &error)).await?;
                return Err(e);
            }
        };
        tracing::warn!("IMAP: {account}: a change was dropped: {reason}");
        let (a, messages) = (account.clone(), entry.messages.clone());
        self.blocking(move |s| {
            s.outbox_drop(id)?;
            record::settle(s, &a, &messages)
        })
        .await?;
        report.refused.push(reason);
        Ok(())
    }
}

/// Why a change did not go, as the server's (or the outbox's) words.
fn server_said(conn: &Conn, message: String) -> SyncError {
    SyncError::Server {
        service: "IMAP",
        server: conn.describe(),
        message,
    }
}
