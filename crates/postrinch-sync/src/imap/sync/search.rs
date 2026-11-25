//! Searching an IMAP server for mail not downloaded yet (docs/imap.md,
//! "Searching the server"): the search the command layer made from the
//! search language ([`ImapSearch`]) asked in each folder it covers
//! ([`plan`]), over the account's reader connection, as the person is
//! waiting and it must not wait behind a pass.
//!
//! What is found is kept to a page, newest first: the highest UIDs of
//! each folder, and across folders by when they arrived. The copies the
//! store does not have yet are fetched by their headers and stored as a
//! pass stores them, so they show in the results and open normally (their
//! bodies come when opened, or by backfill); a pass that reads the folder
//! later finds them kept.
//!
//! A server busy for the account is [`SyncError::Busy`], never a sign-in
//! prompt, and a server whose busy wait is still running is not asked. A
//! folder the server is busy for, or refuses to search in, is left out
//! and said so ([`ServerFound::incomplete`]); a search refused in every
//! folder is [`SyncError::SearchRefused`], and the window keeps to the
//! mail here.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use postrinch_model::{EmailAddress, ImapFolder, ImapSearch, ProviderMessageId, Timestamp};
use postrinch_store::Store;

use super::folder_error::{FolderError, FolderResult};
use super::folders::GMAIL;
use super::places::internal_date;
use super::search_folders::{FolderSearch, inexact, plan};
use super::wire::{Conn, Refusal, uid_set};
use super::{ImapSync, Lane};
use crate::imap::connect::describe;
use crate::{ProgressFn, SyncError, SyncReport, SyncResult};

/// What a server search found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerFound {
    /// The messages found, newest first, each stored (by its headers at
    /// least).
    pub ids: Vec<ProviderMessageId>,
    /// Why some folders were not searched, in the server's words, when
    /// some were not.
    pub incomplete: Option<String>,
}

/// A folder's answer to its search.
struct Hits {
    folder: ImapFolder,
    uidvalidity: u32,
    /// The highest UIDs found, at most a page of them.
    uids: Vec<u32>,
}

/// One UID found, as it stands here.
struct Candidate {
    folder: usize,
    uid: u32,
    /// The message, when the store has the copy.
    known: Option<ProviderMessageId>,
    arrived: Option<Timestamp>,
}

impl ImapSync {
    /// Searches the account's server, and stores by their headers the
    /// copies of what it finds that the store does not have. At most
    /// `max` messages, newest first.
    pub async fn search_server(
        &self,
        account: &EmailAddress,
        search: &ImapSearch,
        max: usize,
    ) -> SyncResult<ServerFound> {
        if search.alternatives.is_empty() || max == 0 {
            return Ok(ServerFound::default());
        }
        if let Some(wait) = self.busy().remaining(account, "IMAP") {
            let settings = self.settings(account).await?;
            return Err(SyncError::Busy {
                service: "IMAP",
                server: describe(&settings.imap),
                message: format!(
                    "it was busy, so it is asked again in {} s",
                    wait.as_secs().max(1)
                ),
                connections: false,
            });
        }
        let result = self
            .over(account, Lane::Reader, async |conn| {
                self.search_in(conn, account, search, max).await
            })
            .await;
        self.note_sign_in(account, &result).await?;
        result
    }

    async fn search_in(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        search: &ImapSearch,
        max: usize,
    ) -> SyncResult<ServerFound> {
        let gmail = conn.offers(GMAIL);
        if !gmail && let Some(term) = inexact(search) {
            return Err(SyncError::SearchRefused {
                server: conn.describe(),
                message: format!("IMAP has no way to ask for {term}"),
            });
        }
        let a = account.clone();
        let folders = self.with_store(move |s| s.imap_folders(&a)).await?;
        let mut hits = Vec::new();
        let mut refused: Vec<String> = Vec::new();
        for FolderSearch { folder, command } in plan(&folders, search, gmail) {
            let name = folder.name.clone();
            match self.search_folder(conn, folder, &command, max).await {
                Ok(Ok(found)) => hits.push(found),
                Ok(Err(why)) => refused.push(why),
                Err(FolderError::Busy(text)) => refused.push(format!("busy for {name:?}: {text}")),
                Err(FolderError::Sync(e)) => return Err(e),
            }
        }
        if hits.is_empty()
            && let Some(why) = refused.first()
        {
            return Err(SyncError::SearchRefused {
                server: conn.describe(),
                message: why.clone(),
            });
        }
        let ids = self.page(conn, account, hits, max).await?;
        let incomplete = refused.first().map(|why| match refused.len() {
            1 => format!("1 folder was not searched ({why})"),
            n => format!("{n} folders were not searched ({why})"),
        });
        Ok(ServerFound { ids, incomplete })
    }

    /// Searches one folder: its highest `max` UIDs found, or why not.
    async fn search_folder(
        &self,
        conn: &mut Conn,
        folder: ImapFolder,
        command: &super::names::Command,
        max: usize,
    ) -> FolderResult<Result<Hits, String>> {
        let Some(selected) = conn.examine(&folder.name).await? else {
            return Ok(Err(format!("{:?} could not be opened", folder.name)));
        };
        let mut uids = if selected.exists == 0 {
            Vec::new()
        } else {
            match conn.uid_search_command(command).await {
                Ok(Ok(uids)) => uids,
                Ok(Err(refusal)) => return Ok(Err(refused(&refusal))),
                // A BAD: the server cannot parse what was asked.
                Err(FolderError::Sync(SyncError::Server { message, .. })) => {
                    return Ok(Err(message));
                }
                Err(e) => return Err(e),
            }
        };
        let keep_from = uids.len().saturating_sub(max);
        uids.drain(..keep_from);
        Ok(Ok(Hits {
            folder,
            uidvalidity: selected.uidvalidity,
            uids,
        }))
    }

    /// The newest `max` of the hits, the copies the store lacks fetched by
    /// their headers and stored.
    async fn page(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        hits: Vec<Hits>,
        max: usize,
    ) -> SyncResult<Vec<ProviderMessageId>> {
        let mut candidates = self.candidates(account, &hits).await?;
        let unknown = candidates.iter().filter(|c| c.known.is_none()).count();
        // Only when there are more than a page is it worth asking when the
        // unknown ones arrived, to keep the newest.
        if candidates.len() > max && unknown > 0 {
            self.arrivals(conn, &hits, &mut candidates).await?;
        }
        candidates.sort_by(|a, b| b.arrived.cmp(&a.arrived).then(b.uid.cmp(&a.uid)));
        candidates.truncate(max);
        let mut wanted: BTreeMap<usize, Vec<u32>> = BTreeMap::new();
        for c in candidates.iter().filter(|c| c.known.is_none()) {
            wanted.entry(c.folder).or_default().push(c.uid);
        }
        for (i, uids) in wanted {
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
                out.push(Candidate {
                    folder,
                    uid,
                    arrived: place.as_ref().and_then(|p| p.arrived),
                    known: place.map(|p| p.message),
                });
            }
            Ok(out)
        })
        .await
    }

    /// When the unknown candidates arrived (`INTERNALDATE`), so the page
    /// keeps the newest across folders. A folder that cannot say leaves
    /// its candidates undated, after the dated ones.
    async fn arrivals(
        &self,
        conn: &mut Conn,
        hits: &[Hits],
        candidates: &mut [Candidate],
    ) -> SyncResult<()> {
        for (i, h) in hits.iter().enumerate() {
            let uids: Vec<u32> = candidates
                .iter()
                .filter(|c| c.folder == i && c.known.is_none())
                .map(|c| c.uid)
                .collect();
            if uids.is_empty() || !self.reopen(conn, h).await? {
                continue;
            }
            let reply = match conn
                .uid_fetch(&uid_set(&uids), "(UID INTERNALDATE)", None)
                .await
            {
                Ok(reply) => reply,
                Err(FolderError::Busy(_)) => continue,
                Err(FolderError::Sync(e)) => return Err(e),
            };
            for f in reply.messages {
                let when = f.internal_date.as_deref().and_then(internal_date);
                if let Some(c) = candidates
                    .iter_mut()
                    .find(|c| c.folder == i && c.uid == f.uid)
                {
                    c.arrived = when;
                }
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
        // Stored at the UIDVALIDITY searched: a pass that reads the folder
        // after a change of UIDVALIDITY finds them kept.
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

/// A refused search in the person's words.
fn refused(refusal: &Refusal) -> String {
    if refusal.code.as_deref() == Some("BADCHARSET") {
        "it cannot search for text that is not plain ASCII".to_owned()
    } else {
        refusal.text.clone()
    }
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
