//! Filing a delivered message in Sent, and looking there for one whose
//! data went without an answer (docs/imap.md, "Writing and sending").
//!
//! A message whose data went without an answer (the connection lost, or
//! the app stopped) is looked for in Sent by its `Message-ID`; found, it
//! was delivered, otherwise it waits for the person, as it may have gone.
//! A delivered message is filed in Sent (APPEND, `\Seen`) unless the
//! provider files it itself. A provider said to that has not a moment
//! later gets the copy, and after several such misses in a row its
//! setting is turned off; one whose setting is off that files the
//! message all the same is seen to, and its setting comes back on, once
//! a back-off that grows each time Postrinch turned it off has passed
//! (`Store::sent_copy_filed`, `Store::server_files_sent`). A Sent folder
//! the server will not open (deleted there) holds nothing to find or
//! count, and refuses the copy, which then waits for the person. Then the
//! drafts it came from are deleted, whether or not Sent kept the copy:
//! the message went.

use std::time::Duration;

use postrinch_model::{EmailAddress, ImapSettings, SystemLabel};

use super::super::places::{SEEN, now};
use super::super::wire::Conn;
use super::drafts::Replaced;
use super::mailbox::{self, Appended, Target};
use super::{Built, Delivery, SENT_MISSES_ALLOWED};
use crate::SyncResult;
use crate::imap::sync::{ImapSync, Lane};

/// How a delivered message is filed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Filing {
    /// Just delivered: appended, unless the provider files it.
    New,
    /// Delivered on an earlier attempt that did not finish: looked for
    /// first, as that attempt may have filed it.
    Again,
    /// Found in Sent already.
    Found,
}

/// What looking in Sent for a transaction that went without an answer
/// showed.
#[derive(Debug)]
pub(super) enum Settled {
    /// Sent has it: the transaction went.
    Went,
    /// It may have gone: the message waits for the person.
    Waits(Delivery),
}

impl ImapSync {
    /// A transaction to `sending` whose data went without an answer: it
    /// went if Sent has the message, otherwise the message waits for the
    /// person, saying `why`. When earlier transactions delivered it (to
    /// `delivered`), Sent has it from them, which says nothing of this
    /// one: it waits for the person.
    pub(super) async fn settle(
        &self,
        account: &EmailAddress,
        settings: &ImapSettings,
        built: &Built,
        (delivered, sending): (&[EmailAddress], &[EmailAddress]),
        why: &str,
    ) -> SyncResult<Settled> {
        if !delivered.is_empty() {
            return Ok(Settled::Waits(Delivery::MayHaveGone(format!(
                "It went to {}; to {} it may have gone: {why}.",
                list(delivered),
                list(sending)
            ))));
        }
        let found = self
            .over(account, Lane::Outgoing, async |conn| {
                let sent = self
                    .target(conn, account, settings, SystemLabel::Sent)
                    .await?;
                let Some(sent) = sent else {
                    return Ok(false);
                };
                let files = self.files_sent(conn, account, &sent).await?;
                look(conn, &sent, &built.header, self.sent_waits(files)).await
            })
            .await?;
        if !found {
            return Ok(Settled::Waits(Delivery::MayHaveGone(format!(
                "It may have gone: {why}, and it is not in Sent."
            ))));
        }
        let a = account.clone();
        self.with_store(move |s| s.sent_copy_found(&a)).await?;
        Ok(Settled::Went)
    }

    /// Files a delivered message in Sent and deletes the drafts it came
    /// from. A failure here leaves it delivered: the next attempt does
    /// only this.
    pub(super) async fn after_delivery(
        &self,
        account: &EmailAddress,
        settings: &ImapSettings,
        built: &Built,
        replaced: &Replaced,
        filing: Filing,
    ) -> SyncResult<Delivery> {
        self.over(account, Lane::Outgoing, async |conn| {
            let refused = match filing {
                Filing::Found => None,
                Filing::New | Filing::Again => {
                    self.file(conn, account, settings, built, filing).await?
                }
            };
            let drafts = if replaced.is_empty() {
                None
            } else {
                self.target(conn, account, settings, SystemLabel::Drafts)
                    .await?
            };
            if let Some(drafts) = drafts {
                self.delete_replaced(conn, account, &drafts, built, replaced)
                    .await?;
            }
            self.retire_superseded(replaced).await?;
            Ok(refused.map_or(Delivery::Done, Delivery::Refused))
        })
        .await
    }

    /// Files `built` in Sent, unless the provider has; returns why Sent
    /// refused it, if it did.
    async fn file(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        settings: &ImapSettings,
        built: &Built,
        filing: Filing,
    ) -> SyncResult<Option<String>> {
        let Some(sent) = self
            .target(conn, account, settings, SystemLabel::Sent)
            .await?
        else {
            tracing::warn!("IMAP: {account} has no Sent folder: a sent message is not filed");
            return Ok(None);
        };
        let files = self.files_sent(conn, account, &sent).await?;
        let look_first = files || filing == Filing::Again;
        if look_first && look(conn, &sent, &built.header, self.sent_waits(files)).await? {
            let a = account.clone();
            self.with_store(move |s| s.sent_copy_found(&a)).await?;
            return Ok(None);
        }
        if !built.missing.is_empty() {
            tracing::warn!(
                "IMAP: {account}: a sent message is filed in Sent without {} of its \
                 attachments, no longer on this computer",
                built.missing.len()
            );
        }
        match mailbox::append(conn, &sent.name, SEEN, &built.raw).await? {
            Appended::Kept(at) => {
                self.record(account, &sent, at, &built.id, &[SEEN]).await?;
            }
            Appended::Refused(text) => {
                return Ok(Some(format!(
                    "It was sent, but {} would not keep a copy: {text}",
                    sent.name
                )));
            }
        }
        let (a, id) = (account.clone(), built.id.clone());
        let turned_off = self
            .with_store(move |s| s.sent_copy_filed(&a, &id, files, SENT_MISSES_ALLOWED, now()))
            .await?;
        if turned_off {
            tracing::info!(
                "IMAP: {account}'s server did not file {SENT_MISSES_ALLOWED} sent messages \
                 in a row in Sent; Postrinch files them from now on"
            );
        }
        Ok(None)
    }

    /// Whether the provider files sent mail in `sent` itself, as the
    /// account's settings say now (turned back on first if it was seen
    /// to, by the copies kept under `sent`'s UIDVALIDITY now).
    async fn files_sent(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        sent: &Target,
    ) -> SyncResult<bool> {
        // A Sent the server will not open has no copies to count.
        let kept = match &sent.label {
            Some(label) => mailbox::examine(conn, &sent.name)
                .await?
                .map(|opened| (label.clone(), opened.uidvalidity)),
            None => None,
        };
        let (a, back_off) = (account.clone(), self.sent_lookup.back_off);
        self.with_store(move |s| {
            let kept = kept.as_ref().map(|(label, v)| (label.as_str(), *v));
            s.server_files_sent(&a, kept, &back_off, now())
        })
        .await
    }
}

/// Addresses, for the person to read.
pub(super) fn list(addresses: &[EmailAddress]) -> String {
    addresses
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether `target` holds the message whose `Message-ID` is `header`:
/// looked for at once, then after each of `waits`. A folder the server
/// will not open does not.
async fn look(
    conn: &mut Conn,
    target: &Target,
    header: &str,
    waits: &[Duration],
) -> SyncResult<bool> {
    for wait in std::iter::once(&Duration::ZERO).chain(waits) {
        tokio::time::sleep(*wait).await;
        // Opened again each time, to see what came meanwhile.
        if mailbox::examine(conn, &target.name).await?.is_none() {
            return Ok(false);
        }
        if !mailbox::find(conn, header).await?.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}
