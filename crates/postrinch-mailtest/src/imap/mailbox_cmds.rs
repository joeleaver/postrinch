//! Commands on folders as a whole: CREATE, DELETE, RENAME, SUBSCRIBE,
//! UNSUBSCRIBE, STATUS and APPEND. LIST and LSUB are in `list`.

use std::sync::Arc;

use chrono::Utc;

use super::config::Extension;
use super::mailbox::DELIMITER;
use super::parser::Value;
use super::response::{Args, parse_datetime, parse_flags, push_string};
use super::session::{Reply, Session};
use super::state::State;

impl Session {
    pub(crate) fn create(&mut self, mut args: Args) -> Reply {
        let name = match args.text().and_then(|n| args.end().map(|()| n)) {
            Ok(n) => n,
            Err(e) => return Reply::Bad(e),
        };
        if name.trim_end_matches(DELIMITER).is_empty() || name.contains(['*', '%']) {
            return Reply::No("[CANNOT] Invalid mailbox name".into());
        }
        self.change_tree(|state, user| {
            let State {
                accounts,
                next_folder_id,
                ..
            } = state;
            let account = accounts.get_mut(user).ok_or("No such user")?;
            account.create(&name, next_folder_id).map_err(|e| e.text())
        })
    }

    pub(crate) fn delete(&mut self, mut args: Args) -> Reply {
        let name = match args.text().and_then(|n| args.end().map(|()| n)) {
            Ok(n) => n,
            Err(e) => return Reply::Bad(e),
        };
        self.change_tree(|state, user| {
            let account = state.accounts.get_mut(user).ok_or("No such user")?;
            account.delete(&name).map_err(|e| e.text())
        })
    }

    pub(crate) fn rename(&mut self, mut args: Args) -> Reply {
        let parsed = (|| -> Result<_, String> {
            let from = args.text()?;
            let to = args.text()?;
            args.end()?;
            Ok((from, to))
        })();
        let (from, to) = match parsed {
            Ok(p) => p,
            Err(e) => return Reply::Bad(e),
        };
        self.change_tree(|state, user| {
            let State {
                accounts,
                next_folder_id,
                ..
            } = state;
            let account = accounts.get_mut(user).ok_or("No such user")?;
            account
                .rename(&from, &to, next_folder_id)
                .map_err(|e| e.text())
        })
    }

    pub(crate) fn subscribe(&mut self, mut args: Args, on: bool) -> Reply {
        let name = match args.text().and_then(|n| args.end().map(|()| n)) {
            Ok(n) => n,
            Err(e) => return Reply::Bad(e),
        };
        self.change_tree(|state, user| {
            let folder = state
                .accounts
                .get_mut(user)
                .and_then(|a| a.folder_mut(&name))
                .ok_or("[NONEXISTENT] No such mailbox")?;
            folder.subscribed = on;
            Ok(())
        })
    }

    /// Runs a change to the user's folders and tells other sessions.
    fn change_tree(
        &mut self,
        change: impl FnOnce(&mut State, &str) -> Result<(), &'static str>,
    ) -> Reply {
        let user = self.user.clone().unwrap_or_default();
        let result = change(&mut self.shared.lock(), &user);
        match result {
            Ok(()) => {
                self.shared.notify();
                Reply::Ok("Done".into())
            }
            Err(text) => Reply::No(text.into()),
        }
    }

    pub(crate) fn status(&mut self, mut args: Args) -> Reply {
        let parsed = (|| -> Result<_, String> {
            let name = args.text()?;
            let items = args.list()?;
            args.end()?;
            Ok((name, items))
        })();
        let (name, items) = match parsed {
            Ok(p) => p,
            Err(e) => return Reply::Bad(e),
        };
        let user = self.user.clone().unwrap_or_default();
        let state = self.shared.lock();
        let condstore = state.has(Extension::Condstore);
        let Some(folder) = state
            .accounts
            .get(&user)
            .and_then(|a| a.folder(&name))
            .filter(|f| f.selectable)
        else {
            return Reply::No("[NONEXISTENT] No such mailbox".into());
        };
        let mut values = Vec::new();
        for item in &items {
            let item = item.as_atom().unwrap_or_default().to_ascii_uppercase();
            let value = match item.as_str() {
                "MESSAGES" => folder.messages.len() as u64,
                "RECENT" => 0,
                "UIDNEXT" => u64::from(folder.uid_next),
                "UIDVALIDITY" => u64::from(folder.uid_validity),
                "UNSEEN" => folder.unseen() as u64,
                "HIGHESTMODSEQ" if condstore => folder.highest_modseq,
                _ => return Reply::Bad(format!("Unknown STATUS item {item}")),
            };
            values.push(format!("{item} {value}"));
        }
        let mut line = b"* STATUS ".to_vec();
        push_string(&mut line, folder.name.as_bytes());
        line.extend_from_slice(format!(" ({})\r\n", values.join(" ")).as_bytes());
        drop(state);
        if items.iter().any(|i| {
            i.as_atom()
                .is_some_and(|a| a.eq_ignore_ascii_case("HIGHESTMODSEQ"))
        }) {
            self.condstore = true;
        }
        self.out.extend_from_slice(&line);
        Reply::Ok("STATUS completed".into())
    }

    /// APPEND mailbox [(flags)] [date-time] message.
    pub(crate) fn append(&mut self, mut args: Args) -> Reply {
        let parsed = (|| -> Result<_, String> {
            let name = args.text()?;
            let flags = match args.optional_list() {
                Some(list) => parse_flags(&list)?,
                None => Default::default(),
            };
            let mut rest = args.rest();
            let body = match rest.pop() {
                Some(Value::Str(body)) => body,
                _ => return Err("APPEND needs the message as a literal".into()),
            };
            let date = match rest.pop() {
                None => Utc::now().fixed_offset(),
                Some(Value::Str(text)) => {
                    let text = String::from_utf8(text).map_err(|_| "Bad date-time")?;
                    parse_datetime(&text).ok_or_else(|| format!("Bad date-time {text}"))?
                }
                Some(other) => return Err(format!("Unexpected {other:?}")),
            };
            if !rest.is_empty() {
                return Err("Too many arguments".into());
            }
            Ok((name, flags, date, body))
        })();
        let (name, flags, date, body) = match parsed {
            Ok(p) => p,
            Err(e) => return Reply::Bad(e),
        };
        let user = self.user.clone().unwrap_or_default();
        let reply = {
            let mut state = self.shared.lock();
            let uidplus = state.has(Extension::Uidplus);
            let Some(folder) = state
                .accounts
                .get_mut(&user)
                .and_then(|a| a.folder_mut(&name))
                .filter(|f| f.selectable)
            else {
                return Reply::No("[TRYCREATE] No such mailbox".into());
            };
            let uid = folder.append(Arc::new(body), flags, date);
            if uidplus {
                Reply::Ok(format!(
                    "[APPENDUID {} {uid}] APPEND completed",
                    folder.uid_validity
                ))
            } else {
                Reply::Ok("APPEND completed".into())
            }
        };
        self.shared.notify();
        reply
    }
}
