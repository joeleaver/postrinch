//! The list's header, after the "one pencil, one panel" mockup: what is
//! shown, named, and one line saying in words what it holds. It only
//! describes; changing a view is the panel's, from the pencil on its row.

use postrinch_commands::{Archived, Filters, Last};
use postrinch_model::ListOrder;
use rinch::prelude::*;

use crate::state::State;
use crate::view_edit::Editing;

#[component]
pub fn view_head(state: State) -> NodeHandle {
    let searching = move || state.search.get().is_some();
    let title = Memo::new(move || {
        if searching() {
            return "Search".to_owned();
        }
        let new = state
            .view_edit
            .with(|e| e.as_ref().is_some_and(|e| e.kind == Editing::New));
        if new {
            return "New view".to_owned();
        }
        state.saved_view().map(|e| e.view.name).unwrap_or_default()
    });
    let line = Memo::new(move || {
        if searching() {
            let accounts = state.search_accounts.get();
            let query = state.search.get().unwrap_or_default();
            return describe(state, &query, &accounts, ListOrder::NewestFirst);
        }
        state
            .shown_view()
            .map(|v| describe(state, &v.query, &v.accounts, v.order))
            .unwrap_or_default()
    });
    rsx! {
        div { class: "view-head",
            h1 { class: "view-title", {move || title.get()} }
            p { class: "view-line", {move || line.get()} }
        }
    }
}

/// What a view holds, in words: "Label Clients · in the inbox · last
/// month", and whatever of its search the words don't cover, as typed.
fn describe(
    state: State,
    query: &str,
    accounts: &[postrinch_model::EmailAddress],
    order: ListOrder,
) -> String {
    let f = Filters::read(query);
    let mut parts: Vec<String> = Vec::new();
    match f.labels.as_slice() {
        [] => {}
        [one] => parts.push(format!("Label {one}")),
        many => {
            let join = if f.all_labels { " and " } else { " or " };
            parts.push(format!("Labels {}", many.join(join)));
        }
    }
    if !f.without.is_empty() {
        parts.push(format!("not {}", f.without.join(", ")));
    }
    match f.archived {
        Archived::Include => {}
        Archived::LeaveOut => parts.push("in the inbox".into()),
        Archived::Only => parts.push("archived only".into()),
    }
    let rest = Filters::default().apply(query);
    if !rest.is_empty() {
        parts.push(rest);
    }
    for (any, not, word) in [(&f.from, &f.not_from, "from"), (&f.to, &f.not_to, "to")] {
        if !any.is_empty() {
            parts.push(format!("{word} {}", any.join(" or ")));
        }
        if !not.is_empty() {
            parts.push(format!("not {word} {}", not.join(" or ")));
        }
    }
    if let Some(within) = &f.within {
        parts.push(match within.as_str() {
            "7d" => "last week".to_owned(),
            "1m" => "last month".to_owned(),
            "1y" => "last year".to_owned(),
            other => format!("newer than {other}"),
        });
    }
    for (on, what) in [
        (f.unread, "unread"),
        (f.starred, "starred"),
        (f.attachments, "with an attachment"),
        (f.last == Some(Last::Me), "I replied last"),
        (f.last == Some(Last::Them), "they replied last"),
    ] {
        if on {
            parts.push(what.to_owned());
        }
    }
    let known = crate::ui::labels::sidebar_accounts(state).len();
    match accounts {
        [] => {}
        [one] => parts.push(one.to_string()),
        many if many.len() < known => parts.push(format!("{} accounts", many.len())),
        _ => {}
    }
    match order {
        ListOrder::NewestFirst => {}
        ListOrder::OldestFirst => parts.push("oldest first".into()),
        ListOrder::UnreadFirst => parts.push("unread first".into()),
    }
    if parts.is_empty() {
        return "All mail".to_owned();
    }
    let mut line = parts.join(" · ");
    if let Some(first) = line.get(..1) {
        line = first.to_uppercase() + &line[1..];
    }
    line
}

/// The header's styles.
pub const CSS: &str = r#"
.view-head { flex-shrink: 0; display: flex; flex-direction: column; gap: 3px; padding: 12px 20px 10px; border-bottom: 1px solid var(--line-soft); min-width: 0; }
.view-title { margin: 0; font-size: 18px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.view-line { margin: 0; font-size: 12px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
"#;
