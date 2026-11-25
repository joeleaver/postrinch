//! A server search's page ([`super::search`]): of the UIDs each folder
//! found, the newest `max` messages the search holds for ([`Holds`]),
//! each once, the copies the store lacks fetched by their headers and
//! stored.
//!
//! The found are walked newest first ([`Walk`]) a batch at a time: a
//! batch is as many as the page still needs, its copies the store lacks
//! are fetched, and those that hold fill the page. A batch the check
//! leaves out wholly is followed by the next, older one, so a page of 50
//! is 50 that hold, not the 50 newest the server found less those that
//! do not. The walk takes at most [`LOOKED_PER_SLOT`] UIDs for each slot.
//!
//! A message can be found in more than one folder (a copy in the inbox
//! and one in an archive), and a page slot is a message's, not a copy's.
//! Copies the store has are told apart by the message they belong to;
//! when there are more than a page across folders, copies it lacks are
//! told apart by their `Message-ID`, which names the message as a pass
//! would ([`message_id`]), and ordered by when they arrived
//! (`INTERNALDATE`), so the page keeps the newest across folders.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use mail_parser::MessageParser;
use postrinch_model::{EmailAddress, MessageHeaders, ProviderMessageId};
use postrinch_store::Store;

use super::ImapSync;
use super::folder_error::FolderError;
use super::places::internal_date;
use super::search::{Hits, Holds, Turn};
use super::search_walk::{Candidate, Walk};
use super::wire::{Conn, uid_set};
use crate::imap::message_id;
use crate::{ProgressFn, SyncReport, SyncResult};

/// How many found messages the walk looks through, at most, for each
/// slot of the page: 500 for a page of 50.
pub(super) const LOOKED_PER_SLOT: usize = 10;

/// What is asked of the copies the store lacks, when there are more than
/// a page: when each arrived, and which message it is.
const IDENTIFY: &str = "(UID INTERNALDATE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])";

/// What a page's walk works with.
struct Walking<'a> {
    account: &'a EmailAddress,
    hits: &'a [Hits],
    /// Whether to ask which message each unknown copy is, and when it
    /// arrived.
    identify: bool,
    turn: &'a Turn,
}

/// A page of what a server search found.
pub(super) struct Page {
    /// The messages that hold, newest first, each once.
    pub ids: Vec<ProviderMessageId>,
    /// When the page is short because the walk reached its bound: the
    /// bound.
    pub looked_through: Option<usize>,
}

impl ImapSync {
    /// The newest `max` messages of the hits that `holds` holds for, each
    /// once, the copies the store lacks fetched by their headers and
    /// stored.
    pub(super) async fn page(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        hits: &[Hits],
        holds: &Holds,
        max: usize,
        turn: &Turn,
    ) -> SyncResult<Page> {
        let most = max.saturating_mul(LOOKED_PER_SLOT);
        let found: usize = hits.iter().map(|h| h.uids.len()).sum();
        // Only when several folders found more than a page is it worth
        // asking which messages the unknown ones are, and when they
        // arrived: within a folder, UIDs say which is newer.
        let identify = found > max && hits.iter().filter(|h| !h.uids.is_empty()).count() > 1;
        let mut walk = Walk::new(hits, most);
        let w = Walking {
            account,
            hits,
            identify,
            turn,
        };
        // Messages walked past: a slot is a message's, not a copy's.
        let mut walked: HashSet<ProviderMessageId> = HashSet::new();
        // Messages checked, so a copy stored as one already checked is not
        // checked again.
        let mut checked: HashSet<ProviderMessageId> = HashSet::new();
        let mut ids: Vec<ProviderMessageId> = Vec::new();
        while ids.len() < max {
            let mut batch = Vec::new();
            while batch.len() < max - ids.len() {
                for (i, uids) in walk.take_next(max) {
                    let taken = self.taken(conn, &w, i, uids).await?;
                    walk.add(i, taken);
                }
                let Some(c) = walk.next() else {
                    break;
                };
                if let Some(message) = &c.message
                    && !walked.insert(message.clone())
                {
                    continue;
                }
                batch.push(c);
            }
            if batch.is_empty() {
                break;
            }
            let mut stored = self.store_batch(conn, &w, batch).await?;
            stored.retain(|id| checked.insert(id.clone()));
            if stored.is_empty() {
                continue;
            }
            turn.go_on()?;
            let check = holds.clone();
            let asked = stored.clone();
            let hold = self.blocking(move |s| check.check(s, &asked)).await?;
            ids.extend(stored.into_iter().filter(|id| hold.contains(id)));
        }
        let short = ids.len() < max && walk.cut();
        Ok(Page {
            ids,
            looked_through: short.then_some(most),
        })
    }

    /// The candidates of UIDs taken from one folder, in the order taken:
    /// each with the message the store has for it, if any, and, when
    /// asked to identify, which message each unknown one is and when it
    /// arrived.
    async fn taken(
        &self,
        conn: &mut Conn,
        w: &Walking<'_>,
        folder: usize,
        uids: Vec<u32>,
    ) -> SyncResult<Vec<Candidate>> {
        let (account, hits) = (w.account, &w.hits[folder]);
        let (a, label, uidvalidity) = (
            account.clone(),
            hits.folder.label_id.clone(),
            hits.uidvalidity,
        );
        let mut candidates = self
            .with_store(move |s| {
                let mut out = Vec::with_capacity(uids.len());
                for uid in uids {
                    let place = s.imap_place(&a, &label, uidvalidity, uid)?;
                    let here = place.as_ref().map(|p| p.message.clone());
                    out.push(Candidate {
                        folder,
                        uid,
                        arrived: place.and_then(|p| p.arrived),
                        message: here.clone(),
                        here,
                    });
                }
                Ok(out)
            })
            .await?;
        if w.identify && candidates.iter().any(|c| c.here.is_none()) {
            w.turn.go_on()?;
            self.identify(conn, account, hits, &mut candidates).await?;
            let a = account.clone();
            candidates = self
                .with_store(move |s| {
                    for c in candidates.iter_mut().filter(|c| c.here.is_none()) {
                        if let Some(message) = &c.message
                            && s.has_message(&a, message)?
                        {
                            c.here = Some(message.clone());
                        }
                    }
                    Ok(candidates)
                })
                .await?;
        }
        Ok(candidates)
    }

    /// Which message each unknown candidate is, and when it arrived. A
    /// folder that cannot say leaves its candidates undated, after the
    /// dated ones, and each a message of its own.
    async fn identify(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        hits: &Hits,
        candidates: &mut [Candidate],
    ) -> SyncResult<()> {
        let uids: Vec<u32> = candidates
            .iter()
            .filter(|c| c.here.is_none())
            .map(|c| c.uid)
            .collect();
        if !self.reopen(conn, hits).await? {
            return Ok(());
        }
        let reply = match conn.uid_fetch(&uid_set(&uids), IDENTIFY, None).await {
            Ok(reply) => reply,
            Err(FolderError::Busy(_)) => return Ok(()),
            Err(FolderError::Sync(e)) => return Err(e),
        };
        for f in reply.messages {
            let Some(c) = candidates.iter_mut().find(|c| c.uid == f.uid) else {
                continue;
            };
            c.arrived = f.internal_date.as_deref().and_then(internal_date);
            c.message = f.header.as_deref().and_then(|h| named(account, h));
        }
        Ok(())
    }

    /// Stores a batch's copies the store lacks, by their headers: the
    /// batch's messages then, in its order, each once. A copy that could
    /// not be stored (its folder busy now) is left out.
    async fn store_batch(
        &self,
        conn: &mut Conn,
        w: &Walking<'_>,
        batch: Vec<Candidate>,
    ) -> SyncResult<Vec<ProviderMessageId>> {
        let (account, hits) = (w.account, w.hits);
        let mut wanted: BTreeMap<usize, Vec<u32>> = BTreeMap::new();
        for c in batch.iter().filter(|c| c.here.is_none()) {
            wanted.entry(c.folder).or_default().push(c.uid);
        }
        for (i, uids) in wanted {
            w.turn.go_on()?;
            self.fetch_found(conn, account, &hits[i], uids).await?;
        }
        let places: Vec<(Option<ProviderMessageId>, String, u32, u32)> = batch
            .into_iter()
            .map(|c| {
                let h = &hits[c.folder];
                (c.here, h.folder.label_id.clone(), h.uidvalidity, c.uid)
            })
            .collect();
        let a = account.clone();
        self.blocking(move |s| stored_ids(s, &a, places)).await
    }

    /// Fetches and stores found copies by their headers, as a pass does.
    /// A folder the server is busy for now keeps them for the next pass.
    async fn fetch_found(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        hits: &Hits,
        uids: Vec<u32>,
    ) -> SyncResult<()> {
        if !self.reopen(conn, hits).await? {
            return Ok(());
        }
        // Stored at the UIDVALIDITY searched, which the folder's state may
        // not have yet (a folder no pass has read, or one whose
        // UIDVALIDITY changed): a pass that reads the folder then finds
        // them kept, and they open as any other (bodies.rs).
        let mut state = hits.folder.clone();
        state.uidvalidity = hits.uidvalidity;
        let wanted = uids.into_iter().map(|uid| (uid, 0)).collect();
        let quiet: ProgressFn = Arc::new(|_| {});
        let mut report = SyncReport::default();
        match self
            .fetch_headers(conn, account, &state, wanted, &quiet, &mut report)
            .await
        {
            Ok(_) => Ok(()),
            Err(FolderError::Busy(text)) => {
                tracing::info!(
                    "IMAP: {} is busy for the folder {:?}: {text}",
                    conn.describe(),
                    hits.folder.name
                );
                Ok(())
            }
            Err(FolderError::Sync(e)) => Err(e),
        }
    }

    /// Opens a searched folder again; `false` when it cannot be opened or
    /// its UIDVALIDITY changed since it was searched.
    async fn reopen(&self, conn: &mut Conn, hits: &Hits) -> SyncResult<bool> {
        match conn.examine(&hits.folder.name).await {
            Ok(Some(selected)) => Ok(selected.uidvalidity == hits.uidvalidity),
            Ok(None) | Err(FolderError::Busy(_)) => Ok(false),
            Err(FolderError::Sync(e)) => Err(e),
        }
    }
}

/// The message a `Message-ID` header field names, as a pass names it.
fn named(account: &EmailAddress, field: &[u8]) -> Option<ProviderMessageId> {
    let parsed = MessageParser::default().parse(field)?;
    let mut headers = MessageHeaders::default();
    headers.set_message_id(parsed.message_id()?);
    let id = headers.message_id.filter(|id| !id.trim().is_empty())?;
    Some(message_id(account, &id))
}

/// The stored messages a batch is, in its order, each once: a message
/// the store had, or the one stored at the copy's place (folder,
/// UIDVALIDITY, UID).
fn stored_ids(
    store: &Store,
    account: &EmailAddress,
    places: Vec<(Option<ProviderMessageId>, String, u32, u32)>,
) -> SyncResult<Vec<ProviderMessageId>> {
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for (here, label, uidvalidity, uid) in places {
        let message = match here {
            Some(message) => message,
            None => match store.imap_place(account, &label, uidvalidity, uid)? {
                Some(place) => place.message,
                None => continue,
            },
        };
        if seen.insert(message.clone()) && store.has_message(account, &message)? {
            ids.push(message);
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_id_field_names_the_message_as_a_pass_does() {
        let me = EmailAddress::parse("ada@example.org").unwrap();
        assert_eq!(
            named(&me, b"Message-ID: <abc@x.org>\r\n\r\n"),
            Some(message_id(&me, "abc@x.org"))
        );
        assert_eq!(named(&me, b"\r\n"), None);
    }
}
