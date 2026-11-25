//! The selected folder as one client sees it, SELECT and EXAMINE, and
//! telling a client what changed in its folder.
//!
//! A session keeps its own view of the selected folder: the UIDs it has
//! been told about, in order (so message sequence numbers are positions in
//! that list), and the MODSEQ of each message's flags as last told. After
//! a command, and while it IDLEs, the session compares that view with the
//! folder and reports the difference: EXPUNGE (or VANISHED under QRESYNC)
//! for messages gone, EXISTS for new ones, FETCH FLAGS for changed flags.
//! The view only changes as the client is told, so sequence numbers in
//! every response agree with what the client has heard.

use std::collections::HashMap;

use super::config::Extension;
use super::mailbox::{Folder, Message};
use super::parser::Value;
use super::response::{Args, flag_list};
use super::seqset::{SeqSet, format_set};
use super::session::{Reply, Session};

/// A selected folder as this session's client knows it.
#[derive(Debug, Clone)]
pub(crate) struct Selected {
    pub(crate) folder_id: u64,
    pub(crate) uid_validity: u32,
    pub(crate) read_only: bool,
    /// UIDs the client knows, in order: sequence number `n` is `uids[n-1]`.
    pub(crate) uids: Vec<u32>,
    /// The MODSEQ of each message's flags as the client last heard them.
    pub(crate) known: HashMap<u32, u64>,
}

impl Selected {
    fn new(folder: &Folder, read_only: bool) -> Self {
        Self {
            folder_id: folder.id,
            uid_validity: folder.uid_validity,
            read_only,
            uids: folder.messages.iter().map(|m| m.uid).collect(),
            known: folder.messages.iter().map(|m| (m.uid, m.modseq)).collect(),
        }
    }

    pub(crate) fn seq_of(&self, uid: u32) -> Option<usize> {
        self.uids.binary_search(&uid).ok().map(|i| i + 1)
    }

    /// The messages a set names, as `(sequence number, message)`, in
    /// order; messages expunged but not yet reported are left out.
    pub(crate) fn resolve<'f>(
        &self,
        set: &SeqSet,
        uid_mode: bool,
        folder: &'f Folder,
    ) -> Vec<(usize, &'f Message)> {
        let largest = if uid_mode {
            self.uids.last().copied().unwrap_or(0)
        } else {
            u32::try_from(self.uids.len()).unwrap_or(u32::MAX)
        };
        self.uids
            .iter()
            .enumerate()
            .filter(|&(i, &uid)| {
                let key = if uid_mode {
                    uid
                } else {
                    u32::try_from(i + 1).unwrap_or(u32::MAX)
                };
                set.contains(key, largest)
            })
            .filter_map(|(i, &uid)| folder.find(uid).map(|m| (i + 1, m)))
            .collect()
    }
}

/// `* n FETCH (UID u FLAGS (...) [MODSEQ (m)])`.
pub(crate) fn flags_response(seq: usize, message: &Message, modseq: bool) -> String {
    let mut line = format!(
        "* {seq} FETCH (UID {} FLAGS {}",
        message.uid,
        flag_list(&message.flags)
    );
    if modseq {
        line.push_str(&format!(" MODSEQ ({})", message.modseq));
    }
    line.push_str(")\r\n");
    line
}

/// The flags of a SELECT: the system flags and any keyword in use.
fn folder_flags(folder: &Folder) -> String {
    let mut flags: Vec<String> = ["\\Answered", "\\Flagged", "\\Deleted", "\\Seen", "\\Draft"]
        .map(String::from)
        .to_vec();
    for message in &folder.messages {
        for flag in &message.flags {
            if !flags.contains(flag) {
                flags.push(flag.clone());
            }
        }
    }
    flag_list(&flags)
}

/// QRESYNC parameters of a SELECT: `(uidvalidity modseq [known-uids])`.
struct Qresync {
    uid_validity: u32,
    modseq: u64,
    known_uids: Option<SeqSet>,
}

fn parse_qresync(values: Vec<Value>) -> Result<Qresync, String> {
    let mut args = Args::new(values);
    let uid_validity = u32::try_from(args.number()?).map_err(|_| "Bad UIDVALIDITY")?;
    let modseq = args.number()?;
    let known_uids = match args.peek() {
        Some(Value::Atom(_)) => {
            let text = args.atom()?;
            Some(SeqSet::parse(&text).ok_or("Bad known UIDs")?)
        }
        _ => None,
    };
    // The optional sequence match data only helps a server save work.
    args.optional_list();
    args.end()?;
    Ok(Qresync {
        uid_validity,
        modseq,
        known_uids,
    })
}

impl Session {
    /// Whether the untagged FETCH responses of a command must carry the
    /// UID: always for a UID command, and for every command once the
    /// session is CONDSTORE-aware on a server that offers QRESYNC (RFC
    /// 7162 3.2.4). A CONDSTORE-only server leaves it out of a plain
    /// STORE's or FETCH's responses, as RFC 9051 does.
    pub(crate) fn fetch_carries_uid(&self, uid_mode: bool) -> bool {
        uid_mode || (self.condstore && self.shared.lock().has(Extension::Qresync))
    }

    /// Reports what changed in the selected folder since the client last
    /// heard. `false` when the folder is gone (deleted, or its UIDVALIDITY
    /// changed), in which case the session must end.
    pub(crate) fn sync_selected(&mut self, allow_expunge: bool) -> bool {
        let Some(selected) = self.selected.as_mut() else {
            return true;
        };
        let Some(user) = self.user.as_deref() else {
            return true;
        };
        let state = self.shared.lock();
        let Some(folder) = state
            .accounts
            .get(user)
            .and_then(|a| a.folder_by_id(selected.folder_id))
        else {
            return false;
        };
        if folder.uid_validity != selected.uid_validity || !folder.selectable {
            return false;
        }
        let out = &mut self.out;
        if allow_expunge {
            let gone: Vec<u32> = selected
                .uids
                .iter()
                .copied()
                .filter(|&uid| folder.find(uid).is_none())
                .collect();
            if !gone.is_empty() {
                if self.qresync {
                    out.extend_from_slice(
                        format!("* VANISHED {}\r\n", format_set(&gone)).as_bytes(),
                    );
                }
                for &uid in gone.iter().rev() {
                    let seq = selected.seq_of(uid).expect("a UID from the view");
                    if !self.qresync {
                        out.extend_from_slice(format!("* {seq} EXPUNGE\r\n").as_bytes());
                    }
                    selected.uids.remove(seq - 1);
                    selected.known.remove(&uid);
                }
            }
        }
        let last = selected.uids.last().copied().unwrap_or(0);
        let new: Vec<&Message> = folder.messages.iter().filter(|m| m.uid > last).collect();
        if !new.is_empty() {
            for message in &new {
                selected.uids.push(message.uid);
                selected.known.insert(message.uid, message.modseq);
            }
            out.extend_from_slice(format!("* {} EXISTS\r\n", selected.uids.len()).as_bytes());
        }
        for (i, uid) in selected.uids.iter().enumerate() {
            let Some(message) = folder.find(*uid) else {
                continue;
            };
            let known = selected.known.entry(*uid).or_insert(0);
            if message.modseq > *known {
                *known = message.modseq;
                out.extend_from_slice(flags_response(i + 1, message, self.condstore).as_bytes());
            }
        }
        true
    }

    /// SELECT and EXAMINE, with `(CONDSTORE)` or `(QRESYNC (...))`.
    pub(crate) fn select(&mut self, mut args: Args, read_only: bool) -> Reply {
        let name = match args.text() {
            Ok(n) => n,
            Err(e) => return Reply::Bad(e),
        };
        let mut qresync = None;
        if let Some(params) = args.optional_list() {
            let mut params = Args::new(params);
            while !params.is_empty() {
                let param = match params.atom() {
                    Ok(p) => p.to_ascii_uppercase(),
                    Err(e) => return Reply::Bad(e),
                };
                match param.as_str() {
                    "CONDSTORE" if self.shared.lock().has(Extension::Condstore) => {
                        self.condstore = true;
                    }
                    "QRESYNC" if self.qresync => match params.list().and_then(parse_qresync) {
                        Ok(q) => qresync = Some(q),
                        Err(e) => return Reply::Bad(e),
                    },
                    _ => return Reply::Bad(format!("Unsupported SELECT parameter {param}")),
                }
            }
        }
        if let Err(e) = args.end() {
            return Reply::Bad(e);
        }
        let user = self.user.clone().unwrap_or_default();
        let shared = self.shared.clone();
        let state = shared.lock();
        let condstore_offered = state.has(Extension::Condstore);
        // RFC 7162 3.2.11: a server offering CONDSTORE or QRESYNC marks
        // every implicit close, whether or not the client enabled either.
        // The old folder is closed even if the new one cannot be opened.
        if self.selected.take().is_some() && (condstore_offered || state.has(Extension::Qresync)) {
            self.out
                .extend_from_slice(b"* OK [CLOSED] Previous mailbox closed\r\n");
        }
        let Some(folder) = state.accounts.get(&user).and_then(|a| a.folder(&name)) else {
            return Reply::No("[NONEXISTENT] No such mailbox".into());
        };
        if !folder.selectable {
            return Reply::No("[CANNOT] Mailbox cannot be selected".into());
        }
        let selected = Selected::new(folder, read_only);
        let permanent = if read_only {
            "()".to_string()
        } else {
            "(\\Answered \\Flagged \\Deleted \\Seen \\Draft \\*)".to_string()
        };
        let mut text = format!(
            "* FLAGS {}\r\n* OK [PERMANENTFLAGS {permanent}] Flags permitted\r\n* {} EXISTS\r\n* 0 RECENT\r\n* OK [UIDVALIDITY {}] UIDs valid\r\n* OK [UIDNEXT {}] Predicted next UID\r\n",
            folder_flags(folder),
            selected.uids.len(),
            folder.uid_validity,
            folder.uid_next,
        );
        if condstore_offered {
            text.push_str(&format!(
                "* OK [HIGHESTMODSEQ {}] Highest\r\n",
                folder.highest_modseq
            ));
        }
        if state.has(Extension::ObjectId) {
            text.push_str(&format!("* OK [MAILBOXID (F{})] Ok\r\n", folder.id));
        }
        self.out.extend_from_slice(text.as_bytes());
        if let Some(q) = qresync.filter(|q| q.uid_validity == folder.uid_validity) {
            self.qresync_report(&selected, folder, &q);
        }
        drop(state);
        self.selected = Some(selected);
        if read_only {
            Reply::Ok("[READ-ONLY] EXAMINE completed".into())
        } else {
            Reply::Ok("[READ-WRITE] SELECT completed".into())
        }
    }

    /// What changed since the client's MODSEQ, for SELECT (QRESYNC).
    fn qresync_report(&mut self, selected: &Selected, folder: &Folder, q: &Qresync) {
        let largest = folder.uid_next.saturating_sub(1);
        let vanished: Vec<u32> = folder
            .vanished
            .iter()
            .filter(|&&(uid, modseq)| {
                modseq > q.modseq
                    && q.known_uids
                        .as_ref()
                        .is_none_or(|set| set.contains(uid, largest))
            })
            .map(|&(uid, _)| uid)
            .collect();
        if !vanished.is_empty() {
            self.out.extend_from_slice(
                format!("* VANISHED (EARLIER) {}\r\n", format_set(&vanished)).as_bytes(),
            );
        }
        for (i, uid) in selected.uids.iter().enumerate() {
            if let Some(message) = folder.find(*uid).filter(|m| m.modseq > q.modseq) {
                self.out
                    .extend_from_slice(flags_response(i + 1, message, true).as_bytes());
            }
        }
    }
}
