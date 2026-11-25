//! One outbox entry made on the server: each message's change planned
//! from its copies as kept ([`super::plan`]), the copies a delete leaves
//! checked on the server ([`super::verify`]), then the ops carried out
//! ([`super::apply`]) and the messages brought in step with their copies,
//! whatever happened.

use postrinch_model::{EmailAddress, ImapPlace, ProviderMessageId};
use postrinch_store::{OutboxEntry, Store};

use super::super::ImapSync;
use super::super::wire::Conn;
use super::apply::{Exec, Grouped};
use super::failure::{Failure, Outcome};
use super::plan::{Folders, Op, Planned, plan};
use super::record;
use crate::SyncResult;

/// One message's copies as kept.
pub(super) struct Held {
    pub places: Vec<ImapPlace>,
    /// The label ids of the folders where a copy of it was made whose UID
    /// is not known yet, and that sync has not read past since.
    pub unplaced: Vec<String>,
}

/// A message's copies as kept, forgetting the copies whose UID was not
/// known once sync has read their folder past them.
fn held(store: &Store, account: &EmailAddress, message: &ProviderMessageId) -> SyncResult<Held> {
    let places = store.imap_message_places(account, message)?;
    let mut unplaced = Vec::new();
    for copy in store.imap_unplaced_copies(account, message)? {
        match store.imap_folder(account, &copy.folder)? {
            Some(read) if copy.waits(&read) => unplaced.push(copy.folder),
            _ => store.remove_imap_unplaced_copy(account, &copy.folder, message)?,
        }
    }
    Ok(Held { places, unplaced })
}

impl ImapSync {
    /// Makes `entry`'s change on the server, keeping the store's copies in
    /// step, and brings its messages in step with their copies whatever
    /// happened. A message the change cannot be made to does not stop the
    /// others: the entry waits, or is refused, once the rest is done.
    pub(super) async fn make_change(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        folders: &mut Folders,
        entry: &OutboxEntry,
    ) -> Outcome<()> {
        let (a, ids) = (account.clone(), entry.messages.clone());
        let held: Vec<Held> = self
            .blocking(move |s| ids.iter().map(|id| held(s, &a, id)).collect())
            .await?;
        let mut planned: Vec<Vec<Op>> = Vec::with_capacity(held.len());
        let (mut refused, mut waits) = (None, None);
        for copies in &held {
            let mut one = plan(&copies.places, &copies.unplaced, &entry.change, folders);
            if one == Planned::NeedsArchive {
                self.make_archive(conn, account, folders).await?;
                one = plan(&copies.places, &copies.unplaced, &entry.change, folders);
            }
            planned.push(match one {
                Planned::Ops(ops) => ops,
                Planned::Refused(reason) => {
                    refused.get_or_insert(reason);
                    Vec::new()
                }
                Planned::NeedsArchive => {
                    refused.get_or_insert("the account has no archive folder".to_owned());
                    Vec::new()
                }
                Planned::Waits(reason) => {
                    waits.get_or_insert(reason);
                    Vec::new()
                }
            });
        }
        let mut exec = Exec::new(conn);
        let done = self
            .carry_out(&mut exec, account, folders, &held, planned, &mut refused)
            .await;
        if let Some(reason) = exec.take_waits() {
            waits.get_or_insert(reason);
        }
        let (a, ids) = (account.clone(), entry.messages.clone());
        self.blocking(move |s| record::settle(s, &a, &ids)).await?;
        done?;
        // What waits is tried again, so it goes before a refusal, which
        // would drop the entry and the part still to come with it.
        match (waits, refused) {
            (Some(reason), _) => Err(Failure::Waits(reason)),
            (None, Some(reason)) => Err(Failure::Refused(reason)),
            (None, None) => Ok(()),
        }
    }

    /// The planned ops, once each message's deletes are known to leave a
    /// copy on the server.
    async fn carry_out(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &mut Folders,
        held: &[Held],
        mut planned: Vec<Vec<Op>>,
        refused: &mut Option<String>,
    ) -> Outcome<()> {
        self.keep_one_copy(exec, account, folders, held, &mut planned, refused)
            .await?;
        let mut grouped = Grouped::default();
        planned.into_iter().flatten().for_each(|op| grouped.add(op));
        self.run(exec, account, folders, grouped).await
    }
}
