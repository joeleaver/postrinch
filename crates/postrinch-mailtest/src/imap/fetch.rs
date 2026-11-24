//! FETCH and UID FETCH, with CONDSTORE's CHANGEDSINCE and QRESYNC's
//! VANISHED modifiers.
//!
//! Supported items: UID, FLAGS, INTERNALDATE, RFC822.SIZE, MODSEQ, the FAST
//! macro, RFC822, RFC822.HEADER, RFC822.TEXT, and BODY[section] /
//! BODY.PEEK[section] with an optional `<origin.count>`, where the section
//! is empty, HEADER, TEXT, HEADER.FIELDS (...) or HEADER.FIELDS.NOT (...).
//! ENVELOPE, BODYSTRUCTURE and MIME part numbers are refused with BAD.

use std::collections::BTreeSet;

use super::config::Extension;
use super::mailbox::FlagOp;
use super::message::{header_fields, split_message};
use super::parser::Value;
use super::response::{Args, flag_list, format_datetime, push_literal};
use super::seqset::{SeqSet, format_set};
use super::session::{Reply, Session};

/// A part of a message a FETCH can ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Section {
    Full,
    Header,
    Text,
    Fields(Vec<String>),
    FieldsNot(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Item {
    Uid,
    Flags,
    InternalDate,
    Size,
    ModSeq,
    Body {
        section: Section,
        /// Whether reading it leaves `\Seen` alone.
        peek: bool,
        partial: Option<(usize, usize)>,
        /// The name the response uses, e.g. `BODY[HEADER]` or `RFC822`.
        label: String,
    },
}

fn parse_section(text: &str) -> Result<Section, String> {
    let upper = text.to_ascii_uppercase();
    let fields = |rest: &str| -> Result<Vec<String>, String> {
        let list = rest
            .trim()
            .strip_prefix('(')
            .and_then(|r| r.strip_suffix(')'))
            .ok_or("HEADER.FIELDS needs a list")?;
        Ok(list.split_whitespace().map(str::to_string).collect())
    };
    match upper.as_str() {
        "" => Ok(Section::Full),
        "HEADER" => Ok(Section::Header),
        "TEXT" => Ok(Section::Text),
        _ => {
            if let Some(rest) = upper.strip_prefix("HEADER.FIELDS.NOT ") {
                Ok(Section::FieldsNot(fields(rest)?))
            } else if let Some(rest) = upper.strip_prefix("HEADER.FIELDS ") {
                Ok(Section::Fields(fields(rest)?))
            } else {
                Err(format!(
                    "Section {text} is not supported by the fake server"
                ))
            }
        }
    }
}

fn parse_partial(text: &str) -> Result<Option<(usize, usize)>, String> {
    if text.is_empty() {
        return Ok(None);
    }
    let inner = text
        .strip_prefix('<')
        .and_then(|t| t.strip_suffix('>'))
        .ok_or("Bad partial")?;
    let (origin, count) = inner.split_once('.').ok_or("Bad partial")?;
    let origin = origin.parse().map_err(|_| "Bad partial")?;
    let count = count.parse().map_err(|_| "Bad partial")?;
    Ok(Some((origin, count)))
}

fn parse_item(atom: &str) -> Result<Vec<Item>, String> {
    let upper = atom.to_ascii_uppercase();
    let body = |section, peek, label: &str| Item::Body {
        section,
        peek,
        partial: None,
        label: label.to_string(),
    };
    Ok(match upper.as_str() {
        "UID" => vec![Item::Uid],
        "FLAGS" => vec![Item::Flags],
        "INTERNALDATE" => vec![Item::InternalDate],
        "RFC822.SIZE" => vec![Item::Size],
        "MODSEQ" => vec![Item::ModSeq],
        "FAST" => vec![Item::Flags, Item::InternalDate, Item::Size],
        "RFC822" => vec![body(Section::Full, false, "RFC822")],
        "RFC822.HEADER" => vec![body(Section::Header, true, "RFC822.HEADER")],
        "RFC822.TEXT" => vec![body(Section::Text, false, "RFC822.TEXT")],
        _ => {
            let (peek, rest) = if let Some(r) = upper.strip_prefix("BODY.PEEK[") {
                (true, r)
            } else if let Some(r) = upper.strip_prefix("BODY[") {
                (false, r)
            } else {
                return Err(format!(
                    "Fetch item {atom} is not supported by the fake server"
                ));
            };
            let close = rest.rfind(']').ok_or("Unclosed section")?;
            let section_text = &rest[..close];
            let partial = parse_partial(&rest[close + 1..])?;
            let mut label = format!("BODY[{section_text}]");
            if let Some((origin, _)) = partial {
                label.push_str(&format!("<{origin}>"));
            }
            vec![Item::Body {
                section: parse_section(section_text)?,
                peek,
                partial,
                label,
            }]
        }
    })
}

fn parse_items(value: Value) -> Result<Vec<Item>, String> {
    match value {
        Value::Atom(a) => parse_item(&a),
        Value::List(list) => {
            let mut items = Vec::new();
            for v in list {
                let atom = v.as_atom().ok_or("Fetch items must be atoms")?;
                items.extend(parse_item(atom)?);
            }
            Ok(items)
        }
        Value::Str(_) => Err("Fetch items must be atoms".into()),
    }
}

/// FETCH modifiers: `(CHANGEDSINCE n [VANISHED])`.
#[derive(Default)]
struct Modifiers {
    changed_since: Option<u64>,
    vanished: bool,
}

fn parse_modifiers(values: Vec<Value>) -> Result<Modifiers, String> {
    let mut args = Args::new(values);
    let mut modifiers = Modifiers::default();
    while !args.is_empty() {
        match args.atom()?.to_ascii_uppercase().as_str() {
            "CHANGEDSINCE" => modifiers.changed_since = Some(args.number()?),
            "VANISHED" => modifiers.vanished = true,
            other => return Err(format!("Unknown FETCH modifier {other}")),
        }
    }
    Ok(modifiers)
}

impl Session {
    pub(crate) fn fetch(&mut self, mut args: Args, uid_mode: bool) -> Reply {
        let parsed = (|| -> Result<_, String> {
            let set = SeqSet::parse(&args.atom()?).ok_or("Bad sequence set")?;
            let mut items = parse_items(args.next()?)?;
            let modifiers = match args.optional_list() {
                Some(list) => parse_modifiers(list)?,
                None => Modifiers::default(),
            };
            args.end()?;
            if uid_mode && !items.contains(&Item::Uid) {
                items.insert(0, Item::Uid);
            }
            Ok((set, items, modifiers))
        })();
        let (set, mut items, modifiers) = match parsed {
            Ok(p) => p,
            Err(e) => return Reply::Bad(e),
        };
        let uses_condstore = modifiers.changed_since.is_some() || items.contains(&Item::ModSeq);
        if uses_condstore && !self.shared.lock().has(Extension::Condstore) {
            return Reply::Bad("CONDSTORE is not offered".into());
        }
        if modifiers.vanished && !(uid_mode && self.qresync && modifiers.changed_since.is_some()) {
            return Reply::Bad("VANISHED needs UID FETCH, QRESYNC and CHANGEDSINCE".into());
        }
        if uses_condstore {
            self.condstore = true;
        }
        // Once CONDSTORE is on, every FETCH with FLAGS carries MODSEQ.
        let wants_modseq =
            (self.condstore && items.contains(&Item::Flags)) || modifiers.changed_since.is_some();
        if wants_modseq && !items.contains(&Item::ModSeq) {
            items.push(Item::ModSeq);
        }
        // RFC 7162 3.2.4: on a QRESYNC server, a CONDSTORE-aware FETCH
        // that reports MODSEQ reports the UID with it.
        let with_uid = self.fetch_carries_uid(uid_mode);
        if with_uid && items.contains(&Item::ModSeq) && !items.contains(&Item::Uid) {
            items.insert(0, Item::Uid);
        }
        self.sync_selected(false);
        let changed = self.run_fetch(&set, &items, &modifiers, uid_mode, with_uid);
        if changed {
            self.shared.notify();
        }
        Reply::Ok("FETCH completed".into())
    }

    /// Writes the FETCH responses; returns whether it set `\Seen` anywhere.
    /// `with_uid` says whether a response that sets `\Seen` must carry the
    /// UID (see [`Session::fetch_carries_uid`]).
    fn run_fetch(
        &mut self,
        set: &SeqSet,
        items: &[Item],
        modifiers: &Modifiers,
        uid_mode: bool,
        with_uid: bool,
    ) -> bool {
        let shared = self.shared.clone();
        let mut state = shared.lock();
        let user = self.user.clone().unwrap_or_default();
        let Some(selected) = self.selected.as_mut() else {
            return false;
        };
        let Some(folder) = state
            .accounts
            .get_mut(&user)
            .and_then(|a| a.folder_by_id_mut(selected.folder_id))
        else {
            return false;
        };
        if modifiers.vanished {
            let since = modifiers.changed_since.unwrap_or(0);
            let largest = folder.uid_next.saturating_sub(1);
            let gone: Vec<u32> = folder
                .vanished
                .iter()
                .filter(|&&(uid, modseq)| modseq > since && set.contains(uid, largest))
                .map(|&(uid, _)| uid)
                .collect();
            if !gone.is_empty() {
                self.out.extend_from_slice(
                    format!("* VANISHED (EARLIER) {}\r\n", format_set(&gone)).as_bytes(),
                );
            }
        }
        let targets: Vec<(usize, u32)> = selected
            .resolve(set, uid_mode, folder)
            .into_iter()
            .filter(|(_, m)| modifiers.changed_since.is_none_or(|since| m.modseq > since))
            .map(|(seq, m)| (seq, m.uid))
            .collect();
        let seen: BTreeSet<String> = ["\\Seen".to_string()].into();
        let mut changed = false;
        for (seq, uid) in targets {
            let marks_seen = items
                .iter()
                .any(|i| matches!(i, Item::Body { peek: false, .. }));
            let mut items = items.to_vec();
            if marks_seen
                && !selected.read_only
                && folder.change_flags(uid, FlagOp::Add, &seen) == Some(true)
            {
                // Setting \Seen took a new MODSEQ: the response tells the
                // new flags and, once CONDSTORE is on, that MODSEQ (and
                // the UID where RFC 7162 3.2.4 asks for it).
                changed = true;
                if !items.contains(&Item::Flags) {
                    items.push(Item::Flags);
                }
                if self.condstore && !items.contains(&Item::ModSeq) {
                    items.push(Item::ModSeq);
                }
                if with_uid && !items.contains(&Item::Uid) {
                    items.insert(0, Item::Uid);
                }
            }
            let message = folder.find(uid).expect("resolved from this folder");
            if items.contains(&Item::Flags) {
                selected.known.insert(uid, message.modseq);
            }
            self.out
                .extend_from_slice(format!("* {seq} FETCH (").as_bytes());
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    self.out.push(b' ');
                }
                render(&mut self.out, item, message);
            }
            self.out.extend_from_slice(b")\r\n");
        }
        changed
    }
}

fn render(out: &mut Vec<u8>, item: &Item, message: &super::mailbox::Message) {
    match item {
        Item::Uid => out.extend_from_slice(format!("UID {}", message.uid).as_bytes()),
        Item::Flags => {
            out.extend_from_slice(format!("FLAGS {}", flag_list(&message.flags)).as_bytes())
        }
        Item::InternalDate => out.extend_from_slice(
            format!("INTERNALDATE {}", format_datetime(&message.internal_date)).as_bytes(),
        ),
        Item::Size => {
            out.extend_from_slice(format!("RFC822.SIZE {}", message.body.len()).as_bytes())
        }
        Item::ModSeq => out.extend_from_slice(format!("MODSEQ ({})", message.modseq).as_bytes()),
        Item::Body {
            section,
            partial,
            label,
            ..
        } => {
            let (header, text) = split_message(&message.body);
            let data: Vec<u8> = match section {
                Section::Full => message.body.to_vec(),
                Section::Header => header.to_vec(),
                Section::Text => text.to_vec(),
                Section::Fields(names) => header_fields(header, names, true),
                Section::FieldsNot(names) => header_fields(header, names, false),
            };
            let data = match partial {
                Some((origin, count)) => {
                    let start = (*origin).min(data.len());
                    let end = start.saturating_add(*count).min(data.len());
                    data[start..end].to_vec()
                }
                None => data,
            };
            out.extend_from_slice(label.as_bytes());
            out.push(b' ');
            push_literal(out, &data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items() {
        let items = parse_item("BODY.PEEK[HEADER.FIELDS (From To)]<0.10>").unwrap();
        assert_eq!(
            items,
            vec![Item::Body {
                section: Section::Fields(vec!["FROM".into(), "TO".into()]),
                peek: true,
                partial: Some((0, 10)),
                label: "BODY[HEADER.FIELDS (FROM TO)]<0>".into(),
            }]
        );
        assert_eq!(parse_item("fast").unwrap().len(), 3);
        assert!(parse_item("ENVELOPE").is_err());
        assert!(parse_item("BODY[1]").is_err());
        assert!(matches!(
            parse_item("BODY[]").unwrap()[0],
            Item::Body {
                peek: false,
                section: Section::Full,
                ..
            }
        ));
    }
}
