//! A server search's page ([`super::search`]): of the UIDs each folder
//! found, the newest `max` messages, each once, the copies the store
//! lacks fetched by their headers and stored.
//!
//! A message can be found in more than one folder (a copy in the inbox
//! and one in an archive), and a page slot is a message's, not a copy's.
//! Copies the store has are told apart by the message they belong to;
//! when there are more than a page, copies it lacks are told apart by
//! their `Message-ID`, which names the message as a pass would
//! ([`message_id`]), and by when they arrived (`INTERNALDATE`), so the
//! page keeps the newest across folders.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use mail_parser::MessageParser;
use postrinch_model::{EmailAddress, MessageHeaders, ProviderMessageId, Timestamp};
use postrinch_store::Store;

use super::ImapSync;
use super::folder_error::FolderError;
use super::places::internal_date;
use super::search::{Hits, Turn};
use super::wire::{Conn, uid_set};
use crate::imap::message_id;
use crate::{ProgressFn, SyncReport, SyncResult};

/// What is asked of the copies the store lacks, when there are more than
/// a page: when each arrived, and which message it is.
const IDENTIFY: &str = "(UID INTERNALDATE BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])";

/// One UID found, as it stands here.
#[derive(Debug, Clone)]
struct Candidate {
    folder: usize,
    uid: u32,
    /// The message, when the store has the copy.
    known: Option<ProviderMessageId>,
    /// The message it is, when that is known: the store's, or the one its
    /// `Message-ID` names. `None` for a copy without one, which is a
    /// message of its own.
    message: Option<ProviderMessageId>,
    arrived: Option<Timestamp>,
}

impl ImapSync {
    /// The newest `max` messages of the hits, each once, the copies the
    /// store lacks fetched by their headers and stored.
    pub(super) async fn page(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        hits: Vec<Hits>,
        max: usize,
        turn: &Turn,
    ) -> SyncResult<Vec<ProviderMessageId>> {
        let mut candidates = self.candidates(account, &hits).await?;
        // Only when there are more than a page is it worth asking which
        // messages the unknown ones are, and when they arrived.
        if candidates.len() > max && candidates.iter().any(|c| c.known.is_none()) {
            self.identify(conn, account, &hits, &mut candidates, turn)
                .await?;
        }
        let candidates = one_per_message(candidates, max);
        let mut wanted: BTreeMap<usize, Vec<u32>> = BTreeMap::new();
        for c in candidates.iter().filter(|c| c.known.is_none()) {
            wanted.entry(c.folder).or_default().push(c.uid);
        }
        for (i, uids) in wanted {
            turn.go_on()?;
            self.fetch_found(conn, account, &hits[i], uids).await?;
        }
        // What the store has now of each, newest first, each once.
        let keys: Vec<(String, u32, u32)> = candidates
            .iter()
            .map(|c| {
                let h = &hits[c.folder];
                (h.folder.label_id.clone(), h.uidvalidity, c.uid)
            })
            .collect();
        let a = account.clone();
        self.blocking(move |s| stored_ids(s, &a, &keys)).await
    }

    /// Each UID found, with the message the store has for it, if any.
    async fn candidates(
        &self,
        account: &EmailAddress,
        hits: &[Hits],
    ) -> SyncResult<Vec<Candidate>> {
        let keys: Vec<(usize, String, u32, u32)> = hits
            .iter()
            .enumerate()
            .flat_map(|(i, h)| {
                h.uids
                    .iter()
                    .map(move |&uid| (i, h.folder.label_id.clone(), h.uidvalidity, uid))
            })
            .collect();
        let a = account.clone();
        self.with_store(move |s| {
            let mut out = Vec::with_capacity(keys.len());
            for (folder, label, uidvalidity, uid) in keys {
                let place = s.imap_place(&a, &label, uidvalidity, uid)?;
                let known = place.as_ref().map(|p| p.message.clone());
                out.push(Candidate {
                    folder,
                    uid,
                    arrived: place.and_then(|p| p.arrived),
                    message: known.clone(),
                    known,
                });
            }
            Ok(out)
        })
        .await
    }

    /// Which message each unknown candidate is, and when it arrived. A
    /// folder that cannot say leaves its candidates undated, after the
    /// dated ones, and each a message of its own.
    async fn identify(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        hits: &[Hits],
        candidates: &mut [Candidate],
        turn: &Turn,
    ) -> SyncResult<()> {
        for (i, h) in hits.iter().enumerate() {
            let uids: Vec<u32> = candidates
                .iter()
                .filter(|c| c.folder == i && c.known.is_none())
                .map(|c| c.uid)
                .collect();
            if uids.is_empty() {
                continue;
            }
            turn.go_on()?;
            if !self.reopen(conn, h).await? {
                continue;
            }
            let reply = match conn.uid_fetch(&uid_set(&uids), IDENTIFY, None).await {
                Ok(reply) => reply,
                Err(FolderError::Busy(_)) => continue,
                Err(FolderError::Sync(e)) => return Err(e),
            };
            for f in reply.messages {
                let Some(c) = candidates
                    .iter_mut()
                    .find(|c| c.folder == i && c.uid == f.uid)
                else {
                    continue;
                };
                c.arrived = f.internal_date.as_deref().and_then(internal_date);
                c.message = f.header.as_deref().and_then(|h| named(account, h));
            }
        }
        Ok(())
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

/// The newest `max` candidates, one per message: of a message's copies,
/// one the store has if any (nothing to fetch), else the newest.
fn one_per_message(mut candidates: Vec<Candidate>, max: usize) -> Vec<Candidate> {
    candidates.sort_by(|a, b| b.arrived.cmp(&a.arrived).then(b.uid.cmp(&a.uid)));
    let here: HashSet<ProviderMessageId> =
        candidates.iter().filter_map(|c| c.known.clone()).collect();
    let mut taken: HashSet<ProviderMessageId> = HashSet::new();
    let mut page = Vec::new();
    for c in candidates {
        if page.len() == max {
            break;
        }
        if let Some(message) = &c.message {
            if c.known.is_none() && here.contains(message) {
                continue;
            }
            if !taken.insert(message.clone()) {
                continue;
            }
        }
        page.push(c);
    }
    page
}

/// The stored messages at these places (folder, UIDVALIDITY, UID), in
/// their order, each once.
fn stored_ids(
    store: &Store,
    account: &EmailAddress,
    places: &[(String, u32, u32)],
) -> SyncResult<Vec<ProviderMessageId>> {
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for (label, uidvalidity, uid) in places {
        let Some(place) = store.imap_place(account, label, *uidvalidity, *uid)? else {
            continue;
        };
        if seen.insert(place.message.clone()) && store.has_message(account, &place.message)? {
            ids.push(place.message);
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(folder: usize, uid: u32, at: i64, known: bool, message: Option<&str>) -> Candidate {
        let message = message.map(|m| ProviderMessageId(m.into()));
        Candidate {
            folder,
            uid,
            known: if known { message.clone() } else { None },
            message,
            arrived: Some(Timestamp::from_millis(at)),
        }
    }

    fn picked(page: &[Candidate]) -> Vec<(usize, u32)> {
        page.iter().map(|c| (c.folder, c.uid)).collect()
    }

    #[test]
    fn a_page_slot_is_a_messages_not_a_copys() {
        let page = one_per_message(
            vec![
                // One message in the inbox and in an archive.
                found(0, 9, 300, false, Some("ma")),
                found(1, 4, 300, false, Some("ma")),
                found(0, 8, 200, false, Some("mb")),
                found(1, 3, 100, false, Some("mc")),
            ],
            2,
        );
        assert_eq!(picked(&page), [(0, 9), (0, 8)]);
    }

    #[test]
    fn the_copy_here_stands_for_its_message() {
        let page = one_per_message(
            vec![
                found(0, 9, 300, false, Some("ma")),
                found(1, 4, 250, true, Some("ma")),
                found(0, 8, 200, false, Some("mb")),
            ],
            2,
        );
        assert_eq!(picked(&page), [(1, 4), (0, 8)]);
    }

    #[test]
    fn copies_without_a_message_id_are_messages_of_their_own() {
        let page = one_per_message(
            vec![found(0, 2, 200, false, None), found(1, 1, 100, false, None)],
            5,
        );
        assert_eq!(page.len(), 2);
    }

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
