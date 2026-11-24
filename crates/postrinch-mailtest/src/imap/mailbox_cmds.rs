//! Commands on folders as a whole: LIST, LSUB, CREATE, DELETE, RENAME,
//! SUBSCRIBE, UNSUBSCRIBE, STATUS and APPEND.

use std::sync::Arc;

use chrono::Utc;

use super::config::Extension;
use super::mailbox::{DELIMITER, Folder};
use super::parser::Value;
use super::response::{Args, parse_datetime, parse_flags, push_string};
use super::session::{Reply, Session};
use super::state::State;

/// Whether `name` matches a LIST pattern: `*` matches anything, `%`
/// anything but the hierarchy delimiter.
fn wildcard(pattern: &[u8], name: &[u8]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some((b'*', rest)) => (0..=name.len()).any(|i| wildcard(rest, &name[i..])),
        Some((b'%', rest)) => (0..=name.len())
            .take_while(|&i| i == 0 || name[i - 1] != DELIMITER as u8)
            .any(|i| wildcard(rest, &name[i..])),
        Some((&c, rest)) => name.first() == Some(&c) && wildcard(rest, &name[1..]),
    }
}

/// Matches a pattern against a folder name, INBOX case-insensitively.
fn pattern_matches(pattern: &str, name: &str) -> bool {
    if name == "INBOX" {
        return wildcard(pattern.to_ascii_uppercase().as_bytes(), b"INBOX");
    }
    wildcard(pattern.as_bytes(), name.as_bytes())
}

#[derive(Default)]
struct ListOptions {
    only_subscribed: bool,
    only_special: bool,
    show_subscribed: bool,
}

impl Session {
    /// LIST (with LIST-EXTENDED's selection and RETURN options as far as
    /// SPECIAL-USE and SUBSCRIBED go) and LSUB.
    pub(crate) fn list(&mut self, mut args: Args, lsub: bool) -> Reply {
        let special_use = self.shared.lock().has(Extension::SpecialUse);
        let parsed = (|| -> Result<_, String> {
            let mut options = ListOptions::default();
            if !lsub && let Some(selection) = args.optional_list() {
                for option in selection {
                    match option.as_atom().map(str::to_ascii_uppercase).as_deref() {
                        Some("SUBSCRIBED") => {
                            options.only_subscribed = true;
                            options.show_subscribed = true;
                        }
                        Some("SPECIAL-USE") if special_use => options.only_special = true,
                        Some("RECURSIVEMATCH") => {}
                        _ => return Err(format!("Unsupported LIST option {option:?}")),
                    }
                }
            }
            let reference = args.text()?;
            let patterns = match args.next()? {
                Value::List(list) => list
                    .iter()
                    .map(|v| {
                        v.as_astring()
                            .and_then(|b| String::from_utf8(b.to_vec()).ok())
                            .ok_or_else(|| "Bad pattern".to_string())
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                value => vec![
                    String::from_utf8(value.as_astring().ok_or("Bad pattern")?.to_vec())
                        .map_err(|_| "Bad pattern")?,
                ],
            };
            if !lsub && args.peek().is_some() {
                if !args.atom()?.eq_ignore_ascii_case("RETURN") {
                    return Err("Expected RETURN".into());
                }
                for option in args.list()? {
                    match option.as_atom().map(str::to_ascii_uppercase).as_deref() {
                        Some("SPECIAL-USE") if special_use => {}
                        Some("SUBSCRIBED") => options.show_subscribed = true,
                        Some("CHILDREN") => {}
                        _ => return Err(format!("Unsupported RETURN option {option:?}")),
                    }
                }
            }
            args.end()?;
            Ok((options, reference, patterns))
        })();
        let (options, reference, patterns) = match parsed {
            Ok(p) => p,
            Err(e) => return Reply::Bad(e),
        };
        let word = if lsub { "LSUB" } else { "LIST" };
        if patterns.iter().all(String::is_empty) {
            self.out.extend_from_slice(
                format!("* {word} (\\Noselect) \"{DELIMITER}\" \"\"\r\n").as_bytes(),
            );
            return Reply::Ok(format!("{word} completed"));
        }
        let user = self.user.clone().unwrap_or_default();
        let state = self.shared.lock();
        let Some(account) = state.accounts.get(&user) else {
            return Reply::No("No such user".into());
        };
        for folder in account.folders.values() {
            let full_matches = patterns
                .iter()
                .any(|p| pattern_matches(&format!("{reference}{p}"), &folder.name));
            let wanted = full_matches
                && (!(lsub || options.only_subscribed) || folder.subscribed)
                && (!options.only_special || folder.special_use.is_some());
            if !wanted {
                continue;
            }
            let has_children = account
                .folders
                .keys()
                .any(|k| k.starts_with(&format!("{}{DELIMITER}", folder.name)));
            let attributes = attributes(
                folder,
                has_children,
                special_use && !lsub,
                options.show_subscribed,
            );
            let mut line =
                format!("* {word} ({}) \"{DELIMITER}\" ", attributes.join(" ")).into_bytes();
            push_string(&mut line, folder.name.as_bytes());
            line.extend_from_slice(b"\r\n");
            self.out.extend_from_slice(&line);
        }
        Reply::Ok(format!("{word} completed"))
    }

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

fn attributes(
    folder: &Folder,
    has_children: bool,
    special_use: bool,
    subscribed: bool,
) -> Vec<&'static str> {
    let mut attributes = Vec::new();
    if !folder.selectable {
        attributes.push("\\Noselect");
    }
    attributes.push(if has_children {
        "\\HasChildren"
    } else {
        "\\HasNoChildren"
    });
    if special_use && let Some(role) = folder.special_use {
        attributes.push(role.attribute());
    }
    if subscribed && folder.subscribed {
        attributes.push("\\Subscribed");
    }
    attributes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wildcard(b"*", b"a/b/c"));
        assert!(wildcard(b"%", b"abc"));
        assert!(!wildcard(b"%", b"a/b"));
        assert!(wildcard(b"a/%", b"a/b"));
        assert!(!wildcard(b"a/%", b"a/b/c"));
        assert!(wildcard(b"a/*", b"a/b/c"));
        assert!(wildcard(b"%/b", b"a/b"));
        assert!(!wildcard(b"a", b"ab"));
        assert!(pattern_matches("inbox", "INBOX"));
        assert!(pattern_matches("%", "INBOX"));
    }
}
