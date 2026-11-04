//! Each call's answer, as an agent reads it.
//!
//! Mail is hostile input (DESIGN.md "Safety"): whatever a sender wrote
//! (subjects, names, addresses, snippets, bodies, attachment names) is put
//! under an `untrusted` key, so an agent can tell the mail's words from
//! Postrinch's. Ids, dates, counts, flags and labels are Postrinch's own.

use std::collections::HashMap;

use postrinch_commands::{
    Commands, Folder, FolderEntry, Start, ThreadAction, ThreadRef, ThreadRow,
};
use postrinch_model::{EmailAddress, Mailbox, Message, Proposal, Timestamp};
use serde_json::{Value, json};

use crate::wire::Call;

/// The most body text one message carries, in chars.
const BODY_CHARS: usize = 100_000;

/// Run a call and shape its answer.
pub(crate) fn run(commands: &Commands, call: Call, agent: &str) -> Result<Value, String> {
    let mut labels = LabelNames::default();
    match call {
        Call::Accounts => {
            let accounts = commands.accounts().map_err(say)?;
            Ok(Value::Array(
                accounts
                    .into_iter()
                    .map(|a| {
                        json!({
                            "account": a.account.address,
                            "provider": a.account.provider,
                            "state": a.account.state,
                            "messages": a.messages,
                            "downloaded": a.downloaded,
                        })
                    })
                    .collect(),
            ))
        }
        Call::Folders => {
            let sidebar = commands.sidebar().map_err(say)?;
            let everywhere: Vec<Value> = sidebar.everywhere.iter().map(folder).collect();
            let accounts: Vec<Value> = sidebar
                .accounts
                .iter()
                .map(|a| {
                    let mut entries = vec![&a.inbox];
                    entries.extend(&a.labels.pinned);
                    entries.extend(a.labels.entries());
                    entries.dedup_by(|x, y| x.folder == y.folder);
                    json!({
                        "account": a.account.address,
                        "folders": entries.into_iter().map(folder).collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(json!({ "everywhere": everywhere, "accounts": accounts }))
        }
        Call::ListThreads {
            folder,
            account,
            category,
            offset,
            limit,
        } => {
            let narrowed;
            let commands = match &account {
                Some(account) => {
                    commands.check(account).map_err(say)?;
                    narrowed = commands.limited_to(std::slice::from_ref(account));
                    &narrowed
                }
                None => commands,
            };
            let page = commands
                .list_threads(&folder, category, offset, limit)
                .map_err(say)?;
            let threads = rows(commands, &mut labels, &page.threads);
            Ok(json!({ "total": page.total, "offset": page.offset, "threads": threads }))
        }
        Call::Search { query, limit } => {
            let found = commands.search(&query, limit).map_err(say)?;
            let threads = rows(commands, &mut labels, &found.threads);
            // Where words were not searched: bodies not downloaded yet.
            let not_downloaded: Vec<Value> = found
                .remote
                .iter()
                .flat_map(|r| &r.accounts)
                .map(|(account, n)| json!({ "account": account, "messages": n }))
                .collect();
            Ok(json!({
                "query": found.query,
                "more": found.more,
                "threads": threads,
                "not_downloaded": not_downloaded,
            }))
        }
        Call::ReadThread { account, thread } => {
            let Some(read) = commands.read_thread(&account, &thread).map_err(say)? else {
                return Err(format!("{account} has no thread {}.", thread.0));
            };
            let messages: Vec<Value> = read
                .messages
                .iter()
                .map(|m| message(commands, &mut labels, m))
                .collect();
            Ok(json!({
                "account": account,
                "thread": thread.0,
                "messages": messages,
            }))
        }
        Call::LabelChoices { account } => {
            let choices = commands.label_choices(&account).map_err(say)?;
            Ok(Value::Array(
                choices
                    .into_iter()
                    .map(|c| json!({ "label": c.label, "name": c.name }))
                    .collect(),
            ))
        }
        Call::Propose {
            account,
            thread,
            to,
            cc,
            bcc,
            subject,
            body,
            reason,
        } => {
            let mut draft = match &thread {
                Some(thread) => {
                    let read = commands
                        .read_thread(&account, thread)
                        .map_err(say)?
                        .ok_or_else(|| format!("{account} has no thread {}.", thread.0))?;
                    let latest = read.messages.last().ok_or("That thread has no messages.")?;
                    let mut draft = commands
                        .start_draft(&account, Some((thread, &latest.id)), &Start::ReplyAll)
                        .map_err(say)?;
                    // The agent's words above the quoted message.
                    draft.body = format!("{}{}", body.trim_end(), draft.body);
                    draft
                }
                None => {
                    let mut draft = commands
                        .start_draft(&account, None, &Start::New)
                        .map_err(say)?;
                    draft.body = body;
                    draft
                }
            };
            if let Some(to) = to {
                draft.to = to;
            }
            if let Some(cc) = cc {
                draft.cc = cc;
            }
            if let Some(bcc) = bcc {
                draft.bcc = bcc;
            }
            if let Some(subject) = subject {
                draft.subject = subject;
            }
            draft.proposal = Some(Proposal {
                agent: agent.to_owned(),
                reason,
            });
            commands.propose_draft(&draft).map_err(say)?;
            Ok(json!({
                "proposed": true,
                "note": "It is open in Postrinch for the person to check. Nothing is sent until they press Send; they may change it or discard it.",
            }))
        }
        Call::Act { threads, action } => {
            let action = match action {
                ThreadAction::AddLabel(l) => {
                    ThreadAction::AddLabel(label_id(commands, &threads, l)?)
                }
                ThreadAction::RemoveLabel(l) => {
                    ThreadAction::RemoveLabel(label_id(commands, &threads, l)?)
                }
                other => other,
            };
            commands.act(&threads, &action).map_err(say)?;
            Ok(json!({ "done": threads.len() }))
        }
    }
}

/// A label named by its id or by the name agents are shown. The threads
/// must all be in one account, as a label belongs to one.
fn label_id(commands: &Commands, threads: &[ThreadRef], label: String) -> Result<String, String> {
    let Some((account, _)) = threads.first() else {
        return Ok(label);
    };
    if threads.iter().any(|(a, _)| a != account) {
        return Err("A label belongs to one account: label each account's threads apart.".into());
    }
    let choices = commands.label_choices(account).map_err(say)?;
    if choices.iter().any(|c| c.label == label) {
        return Ok(label);
    }
    match choices.iter().find(|c| c.name.eq_ignore_ascii_case(&label)) {
        Some(c) => Ok(c.label.clone()),
        None => Err(format!(
            "{account} has no label {label:?}. list_labels shows the ones it has."
        )),
    }
}

/// An error as one sentence.
fn say(e: postrinch_commands::CommandError) -> String {
    let mut text = e.to_string();
    if let Some(first) = text.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    format!("{text}.")
}

fn folder(entry: &FolderEntry) -> Value {
    let place = match &entry.folder {
        Folder::Everywhere(system) => json!({ "everywhere": system }),
        Folder::Label { account, label } => json!({ "account": account, "label": label }),
    };
    json!({ "name": entry.name, "folder": place, "unread": entry.unread })
}

fn rows(commands: &Commands, labels: &mut LabelNames, rows: &[ThreadRow]) -> Vec<Value> {
    rows.iter()
        .map(|row| {
            let s = &row.summary;
            let mut untrusted = json!({
                "subject": s.subject,
                "people": s.participants.iter().map(mailbox).collect::<Vec<_>>(),
                "snippet": s.snippet,
            });
            if let Some(excerpt) = &row.excerpt {
                untrusted["match"] = Value::String(format!(
                    "{}[{}]{}",
                    excerpt.before, excerpt.hit, excerpt.after
                ));
            }
            json!({
                "account": s.account,
                "thread": s.id.0,
                "latest": when(s.latest),
                "messages": s.message_count,
                "unread": s.unread_count,
                "starred": s.starred,
                "attachments": s.has_attachments,
                "labels": labels.names(commands, &s.account, &s.labels),
                "untrusted": untrusted,
            })
        })
        .collect()
}

fn message(commands: &Commands, labels: &mut LabelNames, m: &Message) -> Value {
    let h = &m.headers;
    let people = |list: &[Mailbox]| list.iter().map(mailbox).collect::<Vec<_>>();
    // What the person sees (an HTML message's text as drawn), and what the
    // message hides from them, set apart.
    let read = if m.downloaded {
        commands.message_text(&m.account, &m.id).ok().flatten()
    } else {
        None
    };
    let (full, hidden) = match read {
        Some(read) => (read.text, read.hidden),
        None => (m.body_text.clone(), Vec::new()),
    };
    let mut body: String = full.chars().take(BODY_CHARS).collect();
    if body.len() < full.len() {
        body.push_str("\n[… the rest of this message is left out]");
    }
    json!({
        "message": m.id.0,
        "received": when(m.received),
        "unread": m.flags.unread,
        "starred": m.flags.starred,
        "labels": labels.names(commands, &m.account, &m.labels),
        "downloaded": m.downloaded,
        "untrusted": {
            "subject": h.subject,
            "from": people(&h.from),
            "to": people(&h.to),
            "cc": people(&h.cc),
            "reply_to": people(&h.reply_to),
            "body": if m.downloaded { Value::String(body) } else { Value::Null },
            "hidden_text": hidden,
            "attachments": m.attachments.iter().map(|a| json!({
                "name": a.filename,
                "type": a.mime_type,
                "size": a.size,
            })).collect::<Vec<_>>(),
        },
    })
}

fn mailbox(m: &Mailbox) -> Value {
    match &m.name {
        Some(name) => json!({ "name": name, "address": m.address }),
        None => json!({ "address": m.address }),
    }
}

/// A moment in RFC 3339, UTC.
fn when(t: Timestamp) -> String {
    jiff::Timestamp::from_millisecond(t.as_millis())
        .map_or_else(|_| String::new(), |t| t.to_string())
}

/// Labels by the person's names for them, read once per account per call.
#[derive(Default)]
struct LabelNames {
    by_account: HashMap<String, HashMap<String, String>>,
}

impl LabelNames {
    fn names(
        &mut self,
        commands: &Commands,
        account: &EmailAddress,
        ids: &[String],
    ) -> Vec<String> {
        let names = self.by_account.entry(account.key()).or_insert_with(|| {
            commands
                .label_choices(account)
                .unwrap_or_default()
                .into_iter()
                .map(|c| (c.label, c.name))
                .collect()
        });
        ids.iter()
            .map(|id| names.get(id).cloned().unwrap_or_else(|| id.clone()))
            .collect()
    }
}
