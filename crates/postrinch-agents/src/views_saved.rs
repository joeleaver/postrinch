//! The views calls' answers: the sidebar's views, what one shows, and
//! making, changing and deleting them as the view panel does. An agent
//! limited to some accounts sees only views of every account or of its
//! own, and changes only those of its own (`Commands::may_change_view`).

use postrinch_commands::{Commands, ViewEntry, followed};
use postrinch_model::{SavedView, ViewCount};
use serde_json::{Value, json};

use crate::view_shape::{self, ViewShape};
use crate::views::{LabelNames, rows, say};
use crate::wire::Call;

pub(crate) fn run(commands: &Commands, call: Call) -> Result<Value, String> {
    match call {
        Call::Views => {
            let entries = commands.views().map_err(say)?;
            Ok(Value::Array(read_all(commands, &entries)?))
        }
        Call::ListView {
            view,
            offset,
            limit,
        } => {
            let entry = find(commands, &view)?;
            let v = entry.view;
            let page = commands
                .list_view(&v, v.order, offset, limit)
                .map_err(say)?;
            let threads = rows(commands, &mut LabelNames::default(), &page.threads);
            Ok(json!({
                "view": v.key,
                "name": v.name,
                "total": page.total,
                "offset": page.offset,
                "threads": threads,
            }))
        }
        Call::SaveView { view, change } => save(commands, view.as_deref(), &change),
        Call::DeleteView { view } => {
            let entry = find(commands, &view)?;
            commands.remove_view(&entry.view.key).map_err(say)?;
            Ok(match entry.original {
                Some(_) => json!({
                    "put_back": entry.view.key,
                    "note": "A built-in view is not deleted: it is put back as it first was.",
                }),
                None => json!({
                    "deleted": entry.view.key,
                    "was": read_one(commands, &entry, None),
                    "note": "This cannot be undone, but save_view with what it was makes it again.",
                }),
            })
        }
        _ => unreachable!("only views calls come here"),
    }
}

/// Make a view, or change the one `named`, and answer with it as saved.
fn save(commands: &Commands, named: Option<&str>, change: &ViewShape) -> Result<Value, String> {
    let mut view = match named {
        Some(named) => find(commands, named)?.view,
        None => {
            let name = change.name.as_deref().unwrap_or_default();
            let mut view = commands.new_view(name, "in:inbox").map_err(say)?;
            view.query = String::new();
            // An agent of some accounts makes views of those.
            if change.accounts.is_none() && !commands.may_change_view(&view) {
                view.accounts = commands
                    .accounts()
                    .map_err(say)?
                    .into_iter()
                    .map(|a| a.account.address)
                    .collect();
            }
            view
        }
    };
    let fixed = change.search.is_some() || change.accounts.is_some();
    match followed(&view.key) {
        Some("inbox") if fixed => {
            return Err(
                "An account's inbox shows Inbox's mail in that account: change Inbox's search instead, and every account's inbox follows.".into(),
            );
        }
        Some(_) if fixed => {
            return Err(
                "An account's category is Gmail's, as it is: its search and accounts cannot change. Make a view of your own instead.".into(),
            );
        }
        _ => {}
    }
    change.apply(&mut view)?;
    commands.save_view(&view).map_err(say)?;
    let entry = find(commands, &view.key)?;
    Ok(read_all(commands, std::slice::from_ref(&entry))?.remove(0))
}

/// The view a call names: by key, or by its name when one view has it.
fn find(commands: &Commands, named: &str) -> Result<ViewEntry, String> {
    let entries = commands.views().map_err(say)?;
    if let Some(entry) = entries.iter().find(|e| e.view.key == named) {
        return Ok(entry.clone());
    }
    let mut by_name = entries
        .into_iter()
        .filter(|e| e.view.name.eq_ignore_ascii_case(named.trim()));
    match (by_name.next(), by_name.next()) {
        (Some(entry), None) => Ok(entry),
        (Some(_), Some(_)) => Err(format!(
            "Several views are called {named:?}; name one by its `view` key, as list_views gives it."
        )),
        (None, _) => Err(format!(
            "There is no view {named:?}. list_views shows the views there are."
        )),
    }
}

/// Views as agents read them, each with its count.
fn read_all(commands: &Commands, entries: &[ViewEntry]) -> Result<Vec<Value>, String> {
    // Every view counted, whatever its sidebar shows.
    let counted: Vec<SavedView> = entries
        .iter()
        .map(|e| SavedView {
            count: ViewCount::All,
            ..e.view.clone()
        })
        .collect();
    let tallies = commands.view_counts(&counted).map_err(say)?;
    Ok(entries
        .iter()
        .map(|e| read_one(commands, e, tallies.iter().find(|t| t.key == e.view.key)))
        .collect())
}

fn read_one(
    commands: &Commands,
    entry: &ViewEntry,
    tally: Option<&postrinch_commands::ViewTally>,
) -> Value {
    view_shape::read(entry, tally, commands.may_change_view(&entry.view))
}
