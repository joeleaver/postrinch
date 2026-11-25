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
//! the server will not take is [`failure`]'s to say.

mod apply;
mod commands;
mod failure;
mod labels;
mod plan;
mod record;

use postrinch_model::EmailAddress;
use postrinch_store::OutboxEntry;

use self::failure::{Failure, GIVE_UP};
use self::plan::Folders;
use super::wire::Conn;
use super::{ImapSync, Lane};
use crate::google::SendReport;
use crate::outbox_runs::{merged, runs};
use crate::{SyncError, SyncResult};

/// How many messages one run of identical changes names at most.
const BATCH: usize = 1000;

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
    /// has them; one that may pass stays to be tried again, the ones after
    /// it wait behind it, and the error is returned.
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
                for run in runs(waiting, BATCH) {
                    self.send_run(conn, account, &mut folders, &run, &mut report)
                        .await?;
                }
                Ok(())
            })
            .await;
        self.note_sign_in(account, &result).await?;
        result.map(|()| report)
    }

    /// The account's folders as kept, with which hold every message.
    async fn folders(&self, conn: &mut Conn, account: &EmailAddress) -> SyncResult<Folders> {
        let listed = conn.session_mut().list_folders().await?;
        let a = account.clone();
        let stored = self.with_store(move |s| s.imap_folders(&a)).await?;
        Ok(Folders::new(stored, &listed))
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
        report: &mut SendReport,
    ) -> SyncResult<()> {
        if run.len() == 1 {
            return self.send_one(conn, account, folders, &run[0], report).await;
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
            Err(Failure::Refused(_) | Failure::Declined(_)) => {
                for entry in run {
                    self.send_one(conn, account, folders, entry, report).await?;
                }
                Ok(())
            }
            Err(Failure::Wait(e)) => {
                let (ids, error) = (
                    run.iter().map(|e| e.id).collect::<Vec<u64>>(),
                    e.to_string(),
                );
                self.with_store(move |s| {
                    ids.iter().try_for_each(|id| s.outbox_failed(*id, &error))
                })
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
        report: &mut SendReport,
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
                return Err(SyncError::Server {
                    service: "IMAP",
                    server: conn.describe(),
                    message: reason,
                });
            }
            Err(Failure::Wait(e)) => {
                let error = e.to_string();
                self.with_store(move |s| s.outbox_failed(id, &error))
                    .await?;
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
