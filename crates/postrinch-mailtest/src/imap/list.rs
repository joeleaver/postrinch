//! LIST and LSUB, with RFC 5258 LIST-EXTENDED and RFC 6154 SPECIAL-USE.
//!
//! Plain `LIST reference pattern` always works. The extended forms (a
//! selection list before the reference, several patterns in parentheses,
//! and `RETURN (...)` after them) are BAD unless the server offers
//! [`Extension::ListExtended`], as a server without LIST-EXTENDED would
//! answer. A plain LIST still shows the special-use attributes, which RFC
//! 6154 section 2 allows a SPECIAL-USE server to do.

use super::config::Extension;
use super::mailbox::{Account, DELIMITER, Folder};
use super::parser::Value;
use super::response::{Args, push_string};
use super::session::{Reply, Session};

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

/// What a LIST asks for beyond the names.
#[derive(Debug, Default, PartialEq, Eq)]
struct ListOptions {
    /// Selection SUBSCRIBED: only subscribed folders.
    only_subscribed: bool,
    /// Selection SPECIAL-USE: only folders with a special use.
    only_special: bool,
    /// Selection RECURSIVEMATCH: also folders whose descendants match the
    /// selection, marked with CHILDINFO (RFC 5258 3.5).
    recursive_match: bool,
    /// Show `\Subscribed` (selection or return SUBSCRIBED).
    show_subscribed: bool,
}

/// A parsed LIST or LSUB.
#[derive(Debug, PartialEq, Eq)]
struct ListCommand {
    options: ListOptions,
    reference: String,
    patterns: Vec<String>,
}

/// What the server offers that changes how LIST parses.
#[derive(Debug, Clone, Copy)]
struct Offers {
    extended: bool,
    special_use: bool,
}

fn pattern(value: &Value) -> Result<String, String> {
    value
        .as_astring()
        .and_then(|b| String::from_utf8(b.to_vec()).ok())
        .ok_or_else(|| "Bad pattern".to_string())
}

fn parse_selection(selection: Vec<Value>, offers: Offers) -> Result<ListOptions, String> {
    let mut options = ListOptions::default();
    let mut other_than_recursive = false;
    for option in selection {
        match option.as_atom().map(str::to_ascii_uppercase).as_deref() {
            Some("SUBSCRIBED") => {
                options.only_subscribed = true;
                options.show_subscribed = true;
                other_than_recursive = true;
            }
            Some("SPECIAL-USE") if offers.special_use => {
                options.only_special = true;
                other_than_recursive = true;
            }
            // The fake has no remote folders, so REMOTE selects nothing more.
            Some("REMOTE") => {}
            Some("RECURSIVEMATCH") => options.recursive_match = true,
            _ => return Err(format!("Unsupported LIST selection option {option:?}")),
        }
    }
    // RFC 5258 3.1: RECURSIVEMATCH alone (or with only REMOTE) is BAD.
    if options.recursive_match && !other_than_recursive {
        return Err("RECURSIVEMATCH needs another selection option".into());
    }
    Ok(options)
}

fn parse_list(mut args: Args, lsub: bool, offers: Offers) -> Result<ListCommand, String> {
    let extended = offers.extended && !lsub;
    let mut options = match args.peek() {
        Some(Value::List(_)) if !extended => {
            return Err("LIST selection options need LIST-EXTENDED".into());
        }
        Some(Value::List(_)) => parse_selection(args.list()?, offers)?,
        _ => ListOptions::default(),
    };
    let reference = args.text()?;
    let patterns = match args.next()? {
        Value::List(_) if !extended => {
            return Err("Several LIST patterns need LIST-EXTENDED".into());
        }
        Value::List(list) => list.iter().map(pattern).collect::<Result<Vec<_>, _>>()?,
        value => vec![pattern(&value)?],
    };
    if args.peek().is_some() {
        if !extended {
            return Err("LIST RETURN options need LIST-EXTENDED".into());
        }
        if !args.atom()?.eq_ignore_ascii_case("RETURN") {
            return Err("Expected RETURN".into());
        }
        for option in args.list()? {
            match option.as_atom().map(str::to_ascii_uppercase).as_deref() {
                Some("SPECIAL-USE") if offers.special_use => {}
                Some("SUBSCRIBED") => options.show_subscribed = true,
                Some("CHILDREN") => {}
                _ => return Err(format!("Unsupported RETURN option {option:?}")),
            }
        }
    }
    args.end()?;
    Ok(ListCommand {
        options,
        reference,
        patterns,
    })
}

/// Whether a folder passes the selection options (name aside).
fn selected_by(folder: &Folder, options: &ListOptions, lsub: bool) -> bool {
    (!(lsub || options.only_subscribed) || folder.subscribed)
        && (!options.only_special || folder.special_use.is_some())
}

fn descendants<'a>(account: &'a Account, name: &str) -> impl Iterator<Item = &'a Folder> {
    let prefix = format!("{name}{DELIMITER}");
    account
        .folders
        .values()
        .filter(move |f| f.name.starts_with(&prefix))
}

impl Session {
    /// LIST (with LIST-EXTENDED when offered) and LSUB.
    pub(crate) fn list(&mut self, args: Args, lsub: bool) -> Reply {
        let offers = {
            let state = self.shared.lock();
            Offers {
                extended: state.has(Extension::ListExtended),
                special_use: state.has(Extension::SpecialUse),
            }
        };
        let ListCommand {
            options,
            reference,
            patterns,
        } = match parse_list(args, lsub, offers) {
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
            let name_matches = patterns
                .iter()
                .any(|p| pattern_matches(&format!("{reference}{p}"), &folder.name));
            if !name_matches {
                continue;
            }
            let itself = selected_by(folder, &options, lsub);
            // RFC 5258 3.5: with RECURSIVEMATCH and SUBSCRIBED, a folder
            // that is not itself subscribed but has a subscribed
            // descendant is listed with CHILDINFO ("SUBSCRIBED").
            let child_info = options.recursive_match
                && options.only_subscribed
                && !itself
                && descendants(account, &folder.name).any(|d| selected_by(d, &options, lsub));
            if !itself && !child_info {
                continue;
            }
            let has_children = descendants(account, &folder.name).next().is_some();
            let attributes = attributes(
                folder,
                has_children,
                offers.special_use && !lsub,
                options.show_subscribed,
            );
            let mut line =
                format!("* {word} ({}) \"{DELIMITER}\" ", attributes.join(" ")).into_bytes();
            push_string(&mut line, folder.name.as_bytes());
            if child_info {
                line.extend_from_slice(b" (\"CHILDINFO\" (\"SUBSCRIBED\"))");
            }
            line.extend_from_slice(b"\r\n");
            self.out.extend_from_slice(&line);
        }
        Reply::Ok(format!("{word} completed"))
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

    fn atom(s: &str) -> Value {
        Value::Atom(s.to_string())
    }

    fn parse(values: Vec<Value>, extended: bool) -> Result<ListCommand, String> {
        let offers = Offers {
            extended,
            special_use: true,
        };
        parse_list(Args::new(values), false, offers)
    }

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

    #[test]
    fn extended_forms_need_list_extended() {
        let selection = vec![
            Value::List(vec![atom("SUBSCRIBED")]),
            Value::Str(Vec::new()),
            atom("*"),
        ];
        assert!(parse(selection.clone(), false).is_err());
        assert!(parse(selection, true).unwrap().options.only_subscribed);

        let several = vec![
            Value::Str(Vec::new()),
            Value::List(vec![atom("a"), atom("b")]),
        ];
        assert!(parse(several.clone(), false).is_err());
        assert_eq!(parse(several, true).unwrap().patterns, ["a", "b"]);

        let ret = vec![
            Value::Str(Vec::new()),
            atom("*"),
            atom("RETURN"),
            Value::List(vec![atom("CHILDREN")]),
        ];
        assert!(parse(ret.clone(), false).is_err());
        assert!(parse(ret, true).is_ok());

        let plain = vec![Value::Str(Vec::new()), atom("*")];
        assert!(parse(plain, false).is_ok());
    }

    #[test]
    fn recursivematch_alone_is_bad() {
        let only = |options: Vec<Value>| {
            parse(
                vec![Value::List(options), Value::Str(Vec::new()), atom("*")],
                true,
            )
        };
        assert!(only(vec![atom("RECURSIVEMATCH")]).is_err());
        assert!(only(vec![atom("RECURSIVEMATCH"), atom("REMOTE")]).is_err());
        assert!(
            only(vec![atom("RECURSIVEMATCH"), atom("SUBSCRIBED")])
                .unwrap()
                .options
                .recursive_match
        );
        assert!(only(vec![atom("NONSENSE")]).is_err());
    }
}
