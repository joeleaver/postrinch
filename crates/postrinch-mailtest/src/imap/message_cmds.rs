//! Commands that change messages in the selected folder: STORE, COPY,
//! MOVE, EXPUNGE, and CLOSE / UNSELECT.

use super::config::Extension;
use super::mailbox::FlagOp;
use super::parser::Value;
use super::response::{Args, flag_list, parse_flags};
use super::seqset::{SeqSet, format_set};
use super::session::{Reply, Session};

/// A parsed STORE.
struct Store {
    set: SeqSet,
    unchanged_since: Option<u64>,
    op: FlagOp,
    silent: bool,
    flags: std::collections::BTreeSet<String>,
}

fn parse_store(mut args: Args) -> Result<Store, String> {
    let set = SeqSet::parse(&args.atom()?).ok_or("Bad sequence set")?;
    let mut unchanged_since = None;
    if let Some(modifiers) = args.optional_list() {
        let mut modifiers = Args::new(modifiers);
        while !modifiers.is_empty() {
            match modifiers.atom()?.to_ascii_uppercase().as_str() {
                "UNCHANGEDSINCE" => unchanged_since = Some(modifiers.number()?),
                other => return Err(format!("Unknown STORE modifier {other}")),
            }
        }
    }
    let item = args.atom()?.to_ascii_uppercase();
    let (op, rest) = match item.as_bytes().first() {
        Some(b'+') => (FlagOp::Add, &item[1..]),
        Some(b'-') => (FlagOp::Remove, &item[1..]),
        _ => (FlagOp::Replace, item.as_str()),
    };
    let silent = match rest {
        "FLAGS" => false,
        "FLAGS.SILENT" => true,
        _ => return Err(format!("Unknown STORE item {item}")),
    };
    let values = match args.peek() {
        Some(Value::List(_)) => args.list()?,
        _ => args.rest(),
    };
    args.end()?;
    Ok(Store {
        set,
        unchanged_since,
        op,
        silent,
        flags: parse_flags(&values)?,
    })
}

impl Session {
    pub(crate) fn store(&mut self, args: Args, uid_mode: bool) -> Reply {
        let store = match parse_store(args) {
            Ok(s) => s,
            Err(e) => return Reply::Bad(e),
        };
        if store.unchanged_since.is_some() {
            if !self.shared.lock().has(Extension::Condstore) {
                return Reply::Bad("CONDSTORE is not offered".into());
            }
            self.condstore = true;
        }
        if self.selected.as_ref().is_some_and(|s| s.read_only) {
            return Reply::No("[READ-ONLY] Mailbox is read-only".into());
        }
        self.sync_selected(false);
        let with_uid = self.fetch_carries_uid(uid_mode);
        let shared = self.shared.clone();
        let mut state = shared.lock();
        let user = self.user.clone().unwrap_or_default();
        let Some(selected) = self.selected.as_mut() else {
            return Reply::No("No mailbox selected".into());
        };
        let Some(folder) = state
            .accounts
            .get_mut(&user)
            .and_then(|a| a.folder_by_id_mut(selected.folder_id))
        else {
            return Reply::No("[NONEXISTENT] The mailbox is gone".into());
        };
        let targets: Vec<(usize, u32, u64)> = selected
            .resolve(&store.set, uid_mode, folder)
            .into_iter()
            .map(|(seq, m)| (seq, m.uid, m.modseq))
            .collect();
        let mut modified = Vec::new();
        let mut any_change = false;
        for (seq, uid, modseq) in targets {
            if store.unchanged_since.is_some_and(|since| modseq > since) {
                modified.push(if uid_mode { uid } else { seq as u32 });
                continue;
            }
            let changed = folder.change_flags(uid, store.op, &store.flags) == Some(true);
            any_change |= changed;
            let message = folder.find(uid).expect("resolved from this folder");
            selected.known.insert(uid, message.modseq);
            // RFC 7162 3.1.3: a STORE with UNCHANGEDSINCE that succeeds
            // answers with MODSEQ even when .SILENT; once CONDSTORE is on,
            // a .SILENT STORE that changed flags reports the new MODSEQ.
            let says_something =
                !store.silent || store.unchanged_since.is_some() || (self.condstore && changed);
            if !says_something {
                continue;
            }
            let mut parts = Vec::new();
            if with_uid {
                parts.push(format!("UID {uid}"));
            }
            if !store.silent {
                parts.push(format!("FLAGS {}", flag_list(&message.flags)));
            }
            if self.condstore {
                parts.push(format!("MODSEQ ({})", message.modseq));
            }
            self.out
                .extend_from_slice(format!("* {seq} FETCH ({})\r\n", parts.join(" ")).as_bytes());
        }
        drop(state);
        if any_change {
            self.shared.notify();
        }
        if modified.is_empty() {
            Reply::Ok("STORE completed".into())
        } else {
            Reply::Ok(format!(
                "[MODIFIED {}] Conditional STORE failed",
                format_set(&modified)
            ))
        }
    }

    /// COPY, UID COPY, MOVE and UID MOVE.
    pub(crate) fn copy(&mut self, mut args: Args, uid_mode: bool, moving: bool) -> Reply {
        let (uidplus, has_move) = {
            let state = self.shared.lock();
            (state.has(Extension::Uidplus), state.has(Extension::Move))
        };
        if moving && !has_move {
            return Reply::Bad("Unknown command MOVE".into());
        }
        let parsed = (|| -> Result<_, String> {
            let set = SeqSet::parse(&args.atom()?).ok_or("Bad sequence set")?;
            let target = args.text()?;
            args.end()?;
            Ok((set, target))
        })();
        let (set, target) = match parsed {
            Ok(p) => p,
            Err(e) => return Reply::Bad(e),
        };
        if moving && self.selected.as_ref().is_some_and(|s| s.read_only) {
            return Reply::No("[READ-ONLY] Mailbox is read-only".into());
        }
        self.sync_selected(false);
        let shared = self.shared.clone();
        let mut state = shared.lock();
        let user = self.user.clone().unwrap_or_default();
        let Some(selected) = self.selected.as_ref() else {
            return Reply::No("No mailbox selected".into());
        };
        let Some(account) = state.accounts.get_mut(&user) else {
            return Reply::No("No such user".into());
        };
        let Some(source) = account.folder_by_id(selected.folder_id) else {
            return Reply::No("[NONEXISTENT] The mailbox is gone".into());
        };
        let messages: Vec<_> = selected
            .resolve(&set, uid_mode, source)
            .into_iter()
            .map(|(_, m)| m.clone())
            .collect();
        let Some(dest) = account.folder_mut(&target).filter(|f| f.selectable) else {
            return Reply::No("[TRYCREATE] No such mailbox".into());
        };
        let dest_validity = dest.uid_validity;
        let mut source_uids = Vec::new();
        let mut dest_uids = Vec::new();
        for message in &messages {
            source_uids.push(message.uid);
            dest_uids.push(dest.append(
                message.body.clone(),
                message.flags.clone(),
                message.internal_date,
            ));
        }
        let copyuid = (uidplus && !messages.is_empty()).then(|| {
            format!(
                "[COPYUID {dest_validity} {} {}]",
                format_set(&source_uids),
                format_set(&dest_uids)
            )
        });
        if moving {
            if let Some(code) = &copyuid {
                self.out
                    .extend_from_slice(format!("* OK {code} Moved\r\n").as_bytes());
            }
            let source = account
                .folder_by_id_mut(selected.folder_id)
                .expect("found above");
            source.expunge(|m| source_uids.contains(&m.uid));
        }
        drop(state);
        if !messages.is_empty() {
            self.shared.notify();
        }
        let verb = if moving { "MOVE" } else { "COPY" };
        match copyuid {
            Some(code) if !moving => Reply::Ok(format!("{code} {verb} completed")),
            _ => Reply::Ok(format!("{verb} completed")),
        }
    }

    /// EXPUNGE, and UID EXPUNGE (UIDPLUS) for only the UIDs given.
    pub(crate) fn expunge(&mut self, mut args: Args, uid_mode: bool) -> Reply {
        let set = if uid_mode {
            if !self.shared.lock().has(Extension::Uidplus) {
                return Reply::Bad("Unknown command UID EXPUNGE".into());
            }
            match args.atom().map(|a| SeqSet::parse(&a)) {
                Ok(Some(set)) => Some(set),
                Ok(None) => return Reply::Bad("Bad UID set".into()),
                Err(e) => return Reply::Bad(e),
            }
        } else {
            None
        };
        if let Err(e) = args.end() {
            return Reply::Bad(e);
        }
        let Some(selected) = self.selected.as_ref() else {
            return Reply::No("No mailbox selected".into());
        };
        if selected.read_only {
            return Reply::No("[READ-ONLY] Mailbox is read-only".into());
        }
        let largest = selected.uids.last().copied().unwrap_or(0);
        let folder_id = selected.folder_id;
        let user = self.user.clone().unwrap_or_default();
        let (gone, modseq) = {
            let mut state = self.shared.lock();
            let Some(folder) = state
                .accounts
                .get_mut(&user)
                .and_then(|a| a.folder_by_id_mut(folder_id))
            else {
                return Reply::No("[NONEXISTENT] The mailbox is gone".into());
            };
            let gone = folder.expunge(|m| {
                m.flags.contains("\\Deleted")
                    && set.as_ref().is_none_or(|s| s.contains(m.uid, largest))
            });
            (gone, folder.highest_modseq)
        };
        if gone.is_empty() {
            return Reply::Ok("EXPUNGE completed".into());
        }
        self.shared.notify();
        // The EXPUNGE (or VANISHED) responses come from the report that
        // follows every command. With QRESYNC on, the tagged OK carries
        // the MODSEQ this expunge took (RFC 7162 3.2.7 and 3.2.9). Changes
        // by others after it have
        // higher MODSEQs and reach the client as FETCH or VANISHED.
        if self.qresync {
            Reply::Ok(format!("[HIGHESTMODSEQ {modseq}] EXPUNGE completed"))
        } else {
            Reply::Ok("EXPUNGE completed".into())
        }
    }

    /// CLOSE (removing `\Deleted` messages silently) and UNSELECT.
    pub(crate) fn close(&mut self, args: Args, expunge: bool) -> Reply {
        if let Err(e) = args.end() {
            return Reply::Bad(e);
        }
        let Some(selected) = self.selected.take() else {
            return Reply::No("No mailbox selected".into());
        };
        if expunge && !selected.read_only {
            let user = self.user.clone().unwrap_or_default();
            let gone = {
                let mut state = self.shared.lock();
                state
                    .accounts
                    .get_mut(&user)
                    .and_then(|a| a.folder_by_id_mut(selected.folder_id))
                    .map(|f| f.expunge(|m| m.flags.contains("\\Deleted")))
                    .unwrap_or_default()
            };
            if !gone.is_empty() {
                self.shared.notify();
            }
        }
        Reply::Ok("Closed".into())
    }
}
