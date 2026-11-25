//! Searching an IMAP server for mail not downloaded yet (docs/imap.md,
//! "Searching the server"): the search the command layer made from the
//! search language ([`ImapSearch`]) asked in each folder it covers
//! ([`plan`]), over the account's search connection, so that neither a
//! pass nor opening a message waits behind it.
//!
//! A search begun while another of the account's is running takes its
//! place: the older one stops at its next command
//! ([`SyncError::SearchSuperseded`]), keeping the connection, and the
//! newer one goes next. Only the newest is the one the person waits for.
//!
//! The server is asked for at least what the search matches, never less
//! (docs/imap.md), so what it finds is checked against the whole search
//! as search here reads it ([`Holds`], which the caller gives: the
//! command layer's check). What is found is kept to a page of messages
//! that hold ([`search_page`](super::search_page)), newest first, each
//! message once: the found are walked newest first, a batch at a time,
//! the copies the store does not have yet fetched by their headers and
//! stored as a pass stores them, then checked, until a page holds or the
//! found run out. Stored, they show in the results and open normally
//! (their bodies come when opened, or by backfill); a pass that reads the
//! folder later finds them kept. The walk looks through at most
//! [`LOOKED_PER_SLOT`](super::search_page::LOOKED_PER_SLOT) found
//! messages for each slot of the page, so a search the check narrows
//! hard (`ALL` with only `-from:` to say) cannot fetch a whole mailbox;
//! when that stopped it short, [`ServerFound::looked_through`] says so.
//!
//! A server busy for the account is [`SyncError::Busy`], never a sign-in
//! prompt, and a server whose busy wait is still running is not asked. A
//! folder the server is busy for, or refuses to search in, is left out
//! and said so ([`ServerFound::incomplete`]). When no folder was searched,
//! a search refused in one is [`SyncError::SearchRefused`] (the window
//! keeps to the mail here), and one every folder was busy for is
//! [`SyncError::Busy`]: busy for those folders, so the account's busy
//! wait does not start, and the next search asks again.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use postrinch_model::{EmailAddress, ImapFolder, ImapSearch, ProviderMessageId};
use postrinch_store::Store;

use super::folder_error::{FolderError, FolderResult};
use super::folders::GMAIL;
use super::search_folders::{FolderSearch, inexact, plan};
use super::wire::{Conn, Refusal};
use super::{ImapSync, Lane};
use crate::imap::connect::describe;
use crate::{SyncError, SyncResult};

/// What a server search found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerFound {
    /// The messages found, newest first, each stored (by its headers at
    /// least).
    pub ids: Vec<ProviderMessageId>,
    /// Why some folders were not searched, in the server's words, when
    /// some were not.
    pub incomplete: Option<String>,
    /// When fewer than were asked for hold and the walk stopped at its
    /// bound with found messages left unchecked: how many it looked
    /// through at most.
    pub looked_through: Option<usize>,
}

/// The check what a server finds must pass to be shown: of the stored
/// messages given, those the whole search holds for, as search here
/// reads it. The server is asked for a superset; this narrows it.
/// Runs on a blocking thread, with the store.
#[derive(Clone)]
pub struct Holds(Arc<CheckFn>);

type CheckFn = dyn Fn(&Store, &[ProviderMessageId]) -> Result<HashSet<ProviderMessageId>, String>
    + Send
    + Sync;

impl Holds {
    pub fn new(
        check: impl Fn(&Store, &[ProviderMessageId]) -> Result<HashSet<ProviderMessageId>, String>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Arc::new(check))
    }

    /// No check: everything found holds (a search the server answers
    /// exactly).
    pub fn every() -> Self {
        Self::new(|_, ids| Ok(ids.iter().cloned().collect()))
    }

    /// Of `ids`, those that hold.
    pub(super) fn check(
        &self,
        store: &Store,
        ids: &[ProviderMessageId],
    ) -> SyncResult<HashSet<ProviderMessageId>> {
        (self.0)(store, ids).map_err(SyncError::SearchCheck)
    }
}

impl fmt::Debug for Holds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Holds")
    }
}

/// A folder's answer to its search.
pub(super) struct Hits {
    pub folder: ImapFolder,
    pub uidvalidity: u32,
    /// Every UID found, ascending: the walk takes them from the top.
    pub uids: Vec<u32>,
}

/// One search's turn on the account's search connection: it lasts until
/// a newer search of the account begins.
pub(super) struct Turn {
    begun: Arc<AtomicU64>,
    mine: u64,
}

impl Turn {
    /// Begins a search of the account, ending the turn of any before it.
    fn begin(begun: Arc<AtomicU64>) -> Self {
        let mine = begun.fetch_add(1, Ordering::SeqCst) + 1;
        Self { begun, mine }
    }

    /// [`SyncError::SearchSuperseded`] once a newer search began: checked
    /// before each command, so the newer one waits for one at most.
    pub(super) fn go_on(&self) -> SyncResult<()> {
        if self.begun.load(Ordering::SeqCst) == self.mine {
            Ok(())
        } else {
            Err(SyncError::SearchSuperseded)
        }
    }
}

impl ImapSync {
    /// Searches the account's server, and stores by their headers the
    /// copies of what it finds that the store does not have. At most
    /// `max` messages that `holds` holds for, newest first.
    pub async fn search_server(
        &self,
        account: &EmailAddress,
        search: &ImapSearch,
        holds: &Holds,
        max: usize,
    ) -> SyncResult<ServerFound> {
        let turn = Turn::begin(self.searches(account));
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
            .over(account, Lane::Search, async |conn| {
                self.search_in(conn, account, search, holds, max, &turn)
                    .await
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
        holds: &Holds,
        max: usize,
        turn: &Turn,
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
        // Why folders were not searched: refused, and busy.
        let mut refused: Vec<String> = Vec::new();
        let mut busy: Vec<String> = Vec::new();
        for FolderSearch { folder, command } in plan(&folders, search, gmail) {
            turn.go_on()?;
            let name = folder.name.clone();
            match self.search_folder(conn, folder, &command).await {
                Ok(Ok(found)) => hits.push(found),
                Ok(Err(why)) => refused.push(why),
                Err(FolderError::Busy(text)) => busy.push(format!("busy for {name:?}: {text}")),
                Err(FolderError::Sync(e)) => return Err(e),
            }
        }
        if hits.is_empty() {
            if let Some(why) = refused.first() {
                return Err(SyncError::SearchRefused {
                    server: conn.describe(),
                    message: why.clone(),
                });
            }
            if let Some(why) = busy.first() {
                return Err(SyncError::Busy {
                    service: "IMAP",
                    server: conn.describe(),
                    message: match busy.len() {
                        1 => format!("the folder searched was {why}"),
                        n => format!("all {n} folders searched were busy ({why})"),
                    },
                    connections: false,
                });
            }
        }
        turn.go_on()?;
        let page = self.page(conn, account, &hits, holds, max, turn).await?;
        refused.extend(busy);
        let incomplete = refused.first().map(|why| match refused.len() {
            1 => format!("1 folder was not searched ({why})"),
            n => format!("{n} folders were not searched ({why})"),
        });
        Ok(ServerFound {
            ids: page.ids,
            incomplete,
            looked_through: page.looked_through,
        })
    }

    /// Searches one folder: every UID found, or why not.
    async fn search_folder(
        &self,
        conn: &mut Conn,
        folder: ImapFolder,
        command: &super::names::Command,
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
        uids.sort_unstable();
        uids.dedup();
        Ok(Ok(Hits {
            folder,
            uidvalidity: selected.uidvalidity,
            uids,
        }))
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
