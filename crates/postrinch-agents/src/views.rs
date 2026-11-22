//! Each call's answer, as an agent reads it.
//!
//! Mail is hostile input (DESIGN.md "Safety"): whatever a sender wrote
//! (subjects, names, addresses, snippets, bodies, attachment names) is put
//! under an `untrusted` key, so an agent can tell the mail's words from
//! Postrinch's. Ids, dates, counts, flags and labels are Postrinch's own.

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use postrinch_commands::{Commands, Folder, FolderEntry, Start, ThreadRow};
use postrinch_model::{
    AttachmentOrigin, DraftAttachment, EmailAddress, Mailbox, Message, Proposal, Timestamp,
};
use serde_json::{Value, json};

use crate::change;
use crate::gmail::{self, Gmail};
use crate::registry::Agents;
use crate::serve::OnChange;
use crate::wire::{Call, ProposedFile};
use crate::{views_rules, views_saved};

/// The most body text one message carries, in chars.
const BODY_CHARS: usize = 100_000;

/// Who asks, and what the app gave the server to answer them with.
pub(crate) struct Asker<'a> {
    pub agent: &'a str,
    pub agents: &'a Agents,
    pub gmail: Option<&'a dyn Gmail>,
    /// To tell the window about changes that finish after the answer.
    pub on_change: &'a OnChange,
}

/// Run a call and shape its answer.
pub(crate) fn run(commands: &Commands, call: Call, asker: &Asker) -> Result<Value, String> {
    let agent = asker.agent;
    let mut labels = LabelNames::default();
    match call {
        Call::Rules { .. }
        | Call::SaveRule { .. }
        | Call::DeleteRule { .. }
        | Call::SetRuleOn { .. }
        | Call::MoveRule { .. }
        | Call::RunRule { .. } => views_rules::run(commands, call, asker),
        Call::Views | Call::ListView { .. } | Call::SaveView { .. } | Call::DeleteView { .. } => {
            views_saved::run(commands, call)
        }
        Call::Calendars
        | Call::Events { .. }
        | Call::FreeTime { .. }
        | Call::AnswerInvitation { .. } => crate::calendar::run(commands, call, agent),
        Call::CreateLabel { .. } | Call::RenameLabel { .. } | Call::DeleteLabel { .. } => {
            gmail::run(commands, call, asker.gmail)
        }
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
            attachments,
        } => {
            let files = attachments
                .into_iter()
                .map(|f| proposed_file(commands, f))
                .collect::<Result<Vec<_>, String>>()?;
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
            draft.attachments = files;
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
        Call::Attachment {
            account,
            message,
            attachment,
        } => {
            let part = commands
                .attachment(&account, &message, attachment)
                .map_err(say)?
                .ok_or_else(|| {
                    format!(
                        "Message {} has no attachment {attachment} here (it may not be downloaded yet).",
                        message.0
                    )
                })?;
            Ok(json!({
                "attachment": attachment,
                "size": part.bytes.len(),
                "data": STANDARD.encode(&part.bytes),
                "untrusted": {
                    "name": part.filename,
                    "type": part.mime_type,
                },
            }))
        }
        Call::Act {
            threads,
            search,
            messages,
            actions,
        } => change::act(
            commands,
            threads,
            search,
            messages,
            actions,
            asker.on_change,
        ),
    }
}

/// A file an agent attaches, kept for its proposal.
fn proposed_file(commands: &Commands, file: ProposedFile) -> Result<DraftAttachment, String> {
    match file {
        ProposedFile::File { path, name, data } => {
            let bytes = STANDARD
                .decode(data)
                .map_err(|_| format!("{name}: its bytes are not base64."))?;
            commands
                .attach_bytes(&name, None, &bytes, AttachmentOrigin::File { path })
                .map_err(say)
        }
        ProposedFile::Mail {
            account,
            message,
            attachment,
        } => commands
            .attach_from_message(&account, &message, attachment, AttachmentOrigin::Mail)
            .map_err(say)?
            .ok_or_else(|| {
                format!(
                    "Message {} has no attachment {attachment} here (it may not be downloaded yet).",
                    message.0
                )
            }),
    }
}

/// An error as one sentence.
pub(crate) fn say(e: postrinch_commands::CommandError) -> String {
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

pub(crate) fn rows(commands: &Commands, labels: &mut LabelNames, rows: &[ThreadRow]) -> Vec<Value> {
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
            // The event a calendar part describes, if it carries one.
            "invitation": crate::invitations::of(commands, m),
        },
        // Numbered as read_attachment takes them. Inline ones are pictures
        // the body shows.
        "attachments": m.attachments.iter().enumerate().map(|(i, a)| json!({
            "attachment": i,
            "size": a.size,
            "inline": a.inline,
            "untrusted": { "name": a.filename, "type": a.mime_type },
        })).collect::<Vec<_>>(),
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
pub(crate) struct LabelNames {
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
