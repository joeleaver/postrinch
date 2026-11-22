//! change_threads: actions done to named threads, or to every thread a
//! search matches, as selecting all and acting does in the window, or to
//! single messages, as one message at a time does.
//! The answer waits for the changes only so long ([`later`]).

use std::sync::atomic::{AtomicUsize, Ordering};

use postrinch_commands::{Commands, MessageRef, ThreadAction, ThreadRef};
use postrinch_model::EmailAddress;
use serde_json::{Value, json};

use crate::later::{self, Finished};
use crate::serve::OnChange;
use crate::views::say;
use crate::wire::Changed;

/// How many threads are changed between counts of how far it got.
const STEP: usize = 100;

pub(crate) fn act(
    commands: &Commands,
    threads: Vec<ThreadRef>,
    search: Option<String>,
    messages: Vec<MessageRef>,
    actions: Vec<ThreadAction>,
    on_change: &OnChange,
) -> Result<Value, String> {
    if actions.is_empty() {
        return Err("Name at least one action.".into());
    }
    if !messages.is_empty() {
        if !threads.is_empty() || search.is_some() {
            return Err("Name messages, threads or a search: one of them.".into());
        }
        return act_messages(commands, messages, &actions, on_change);
    }
    let (threads, not_searched) = match &search {
        Some(_) if !threads.is_empty() => {
            return Err("Name threads or give a search, not both.".into());
        }
        Some(query) => (
            commands.search_threads(query).map_err(say)?,
            commands.words_not_searched(query).map_err(say)?,
        ),
        None if threads.is_empty() => return Err("Name at least one thread.".into()),
        None => (threads, Vec::new()),
    };
    if threads.is_empty() {
        return Ok(json!({ "changed": 0, "note": "Nothing here matches that search." }));
    }
    // Every label is found in every account first, so a mistake changes
    // nothing.
    let mut plan: Vec<(Vec<ThreadRef>, Vec<ThreadAction>)> = Vec::new();
    for (account, group) in by_account(threads.clone()) {
        let resolved = actions
            .iter()
            .map(|a| resolve(commands, &account, a))
            .collect::<Result<Vec<_>, String>>()?;
        plan.push((group, resolved));
    }
    let total = threads.len();
    let work = {
        let commands = commands.clone();
        move |done: &AtomicUsize| change(&commands, &plan, done)
    };
    let mut answer = match later::within(on_change, Changed::Mail, work)? {
        Finished::Done(()) => json!({ "changed": total }),
        Finished::Going(so_far) => json!({
            "changing": total,
            "changed_so_far": so_far.min(total),
            "note": "Postrinch is still making these changes and will finish by itself; the window shows them when done. Search again in a while to see.",
        }),
    };
    if search.is_some() && answer.get("note").is_none() {
        answer["note"] = json!(
            "Every matching thread here was changed; each change can be undone with its opposite."
        );
    }
    if !not_searched.is_empty() {
        answer["words_not_searched"] = json!(
            not_searched
                .iter()
                .map(|(account, n)| json!({ "account": account, "messages": n }))
                .collect::<Vec<_>>()
        );
    }
    Ok(answer)
}

/// Change messages each alone: archiving one leaves the rest of its
/// conversation where it is.
fn act_messages(
    commands: &Commands,
    messages: Vec<MessageRef>,
    actions: &[ThreadAction],
    on_change: &OnChange,
) -> Result<Value, String> {
    let mut plan: Vec<(Vec<MessageRef>, Vec<ThreadAction>)> = Vec::new();
    for (account, group) in by_account(messages) {
        let resolved = actions
            .iter()
            .map(|a| resolve(commands, &account, a))
            .collect::<Result<Vec<_>, String>>()?;
        plan.push((group, resolved));
    }
    let total: usize = plan.iter().map(|(group, _)| group.len()).sum();
    let work = {
        let commands = commands.clone();
        move |done: &AtomicUsize| {
            for (group, actions) in &plan {
                for chunk in group.chunks(STEP) {
                    for action in actions {
                        commands.act_messages(chunk, action).map_err(say)?;
                    }
                    done.fetch_add(chunk.len(), Ordering::Relaxed);
                }
            }
            Ok(())
        }
    };
    Ok(match later::within(on_change, Changed::Mail, work)? {
        Finished::Done(()) => json!({ "changed_messages": total }),
        Finished::Going(so_far) => json!({
            "changing_messages": total,
            "changed_so_far": so_far.min(total),
            "note": "Postrinch is still making these changes and will finish by itself.",
        }),
    })
}

/// Make the planned changes, counting threads done in `done`.
fn change(
    commands: &Commands,
    plan: &[(Vec<ThreadRef>, Vec<ThreadAction>)],
    done: &AtomicUsize,
) -> Result<(), String> {
    for (group, actions) in plan {
        for chunk in group.chunks(STEP) {
            for action in actions {
                commands.act(chunk, action).map_err(say)?;
            }
            done.fetch_add(chunk.len(), Ordering::Relaxed);
        }
    }
    Ok(())
}

/// Threads or messages, each account's together, in the order accounts
/// come.
fn by_account<T>(threads: Vec<(EmailAddress, T)>) -> Vec<(EmailAddress, Vec<(EmailAddress, T)>)> {
    let mut groups: Vec<(EmailAddress, Vec<(EmailAddress, T)>)> = Vec::new();
    for thread in threads {
        match groups.iter_mut().find(|(a, _)| *a == thread.0) {
            Some((_, group)) => group.push(thread),
            None => groups.push((thread.0.clone(), vec![thread])),
        }
    }
    groups
}

/// An action with its label, named by id or by the name agents are shown,
/// as the account's id.
fn resolve(
    commands: &Commands,
    account: &EmailAddress,
    action: &ThreadAction,
) -> Result<ThreadAction, String> {
    Ok(match action {
        ThreadAction::AddLabel(l) => ThreadAction::AddLabel(label_id(commands, account, l)?),
        ThreadAction::RemoveLabel(l) => ThreadAction::RemoveLabel(label_id(commands, account, l)?),
        other => other.clone(),
    })
}

fn label_id(commands: &Commands, account: &EmailAddress, label: &str) -> Result<String, String> {
    let choices = commands.label_choices(account).map_err(say)?;
    if choices.iter().any(|c| c.label == label) {
        return Ok(label.to_owned());
    }
    match choices.iter().find(|c| c.name.eq_ignore_ascii_case(label)) {
        Some(c) => Ok(c.label.clone()),
        None => Err(format!(
            "{account} has no label {label:?}. list_labels shows the ones it has; create_label makes one."
        )),
    }
}
