//! One outbox entry made on the server: the copies earlier sends asked for
//! and did not see made settled ([`super::attempts`]), each message's
//! change planned from its copies as kept ([`super::plan`]), the copies it
//! copies, moves or deletes from looked for on the server
//! ([`super::verify`]), then the ops carried out ([`super::apply`]) and the
//! messages brought in step with their copies, whatever happened.

use postrinch_model::{EmailAddress, ImapPlace, MessageChange, ProviderMessageId};
use postrinch_store::{OutboxEntry, Store};

use super::super::ImapSync;
use super::super::wire::Conn;
use super::apply::{Exec, Grouped};
use super::attempts::attempts;
use super::failure::{Failure, Outcome};
use super::plan::{Folders, Op, Planned, plan};
use super::record;
use crate::SyncResult;

/// One message's copies as kept.
pub(super) struct Held {
    pub places: Vec<ImapPlace>,
    /// The label ids of the folders a copy of it was asked for in and not
    /// yet seen made or known not to be ([`super::attempts`]): not made
    /// again, and counted for nothing else.
    pub attempted: Vec<String>,
}

/// Why part of a change is not made: refused for good, or waiting. The
/// first reason of each is kept.
#[derive(Default)]
pub(super) struct Verdict {
    pub refused: Option<String>,
    pub waits: Option<String>,
}

/// A message's copies as kept, with the folders a copy of it was asked for
/// in.
pub(super) fn held(
    store: &Store,
    account: &EmailAddress,
    message: &ProviderMessageId,
) -> SyncResult<Held> {
    let attempted = attempts(store, account, message)?
        .into_iter()
        .map(|a| a.folder)
        .collect();
    let places = store.imap_message_places(account, message)?;
    Ok(Held { places, attempted })
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
        let mut exec = Exec::new(conn);
        self.settle_attempts(&mut exec, account, folders, &entry.messages)
            .await?;
        let (a, ids) = (account.clone(), entry.messages.clone());
        let held: Vec<Held> = self
            .blocking(move |s| ids.iter().map(|id| held(s, &a, id)).collect())
            .await?;
        let mut planned: Vec<Vec<Op>> = Vec::with_capacity(held.len());
        let mut verdict = Verdict::default();
        for copies in &held {
            let ops = self
                .plan_one(
                    exec.conn,
                    account,
                    folders,
                    copies,
                    &entry.change,
                    &mut verdict,
                )
                .await?;
            planned.push(ops);
        }
        let done = self
            .carry_out(
                &mut exec,
                account,
                folders,
                (&held, &entry.change),
                planned,
                &mut verdict,
            )
            .await;
        if let Some(reason) = exec.take_waits() {
            verdict.waits.get_or_insert(reason);
        }
        let (a, ids) = (account.clone(), entry.messages.clone());
        self.blocking(move |s| record::settle(s, &a, &ids)).await?;
        done?;
        // What waits is tried again, so it goes before a refusal, which
        // would drop the entry and the part still to come with it.
        match (verdict.waits, verdict.refused) {
            (Some(reason), _) => Err(Failure::Waits(reason)),
            (None, Some(reason)) => Err(Failure::Refused(reason)),
            (None, None) => Ok(()),
        }
    }

    /// What to do to one message's copies (`copies`, which need not be
    /// all those kept) for `change`, making the archive folder where the
    /// change needs one; none, with the reason noted, where the change is
    /// refused for the message or waits.
    pub(super) async fn plan_one(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        folders: &mut Folders,
        copies: &Held,
        change: &MessageChange,
        verdict: &mut Verdict,
    ) -> Outcome<Vec<Op>> {
        let mut one = plan(&copies.places, &copies.attempted, change, folders);
        if one == Planned::NeedsArchive {
            self.make_archive(conn, account, folders).await?;
            one = plan(&copies.places, &copies.attempted, change, folders);
        }
        Ok(match one {
            Planned::Ops(ops) => ops,
            Planned::Refused(reason) => {
                verdict.refused.get_or_insert(reason);
                Vec::new()
            }
            Planned::NeedsArchive => {
                verdict
                    .refused
                    .get_or_insert("the account has no archive folder".to_owned());
                Vec::new()
            }
        })
    }

    /// The planned ops, from copies the server is seen to have, each
    /// message's deletes going only where it keeps a confirmed copy.
    async fn carry_out(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &mut Folders,
        (held, change): (&[Held], &MessageChange),
        mut planned: Vec<Vec<Op>>,
        verdict: &mut Verdict,
    ) -> Outcome<()> {
        let anchored = self
            .keep_one_copy(
                exec,
                account,
                folders,
                (held, change),
                &mut planned,
                verdict,
            )
            .await?;
        let mut grouped = Grouped::new(exec.has_move);
        planned.into_iter().flatten().for_each(|op| grouped.add(op));
        self.run(exec, account, folders, grouped, (&anchored, verdict))
            .await
    }
}
