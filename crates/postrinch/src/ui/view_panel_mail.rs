//! The view panel's "which mail": where (accounts, archived mail, when),
//! who (from, to), what (labels, Gmail's categories, unread and the rest),
//! and the search text
//! all of these write into (`Filters`), so the text stays the one place a
//! view's search lives and each field follows the others.

use postrinch_commands::{Archived, Filters, Last};
use postrinch_model::{EmailAddress, SavedView};
use rinch::prelude::*;

use crate::state::State;
use crate::ui::account_color;
use crate::ui::category_picker::category_picker;
use crate::ui::label_picker::label_picker;
use crate::ui::people_picker::{Side, people_picker};
use crate::ui::view_panel::{choice, read};

/// The panel's search, read as filters.
pub(crate) fn filters(state: State) -> Filters {
    read(state, |e| Filters::read(&e.draft.query))
}

/// Change the panel's search through its filters.
pub(crate) fn change(state: State, edit: impl FnOnce(&mut Filters)) {
    state.change_edit(|v| {
        let mut f = Filters::read(&v.query);
        edit(&mut f);
        v.query = f.apply(&v.query);
    });
}

/// `newer_than:` as the When chips have it: one of theirs, or "other".
fn within(view: &SavedView) -> &'static str {
    match Filters::read(&view.query).within.as_deref() {
        None => "",
        Some("7d") => "7d",
        Some("1m") => "1m",
        Some("1y") => "1y",
        Some(_) => "other",
    }
}

#[component]
pub fn which_mail(state: State) -> NodeHandle {
    let archived = choice(
        __scope,
        state,
        "Archived mail",
        vec![
            (Archived::LeaveOut, "Leave out"),
            (Archived::Include, "Include"),
            (Archived::Only, "Only archived"),
        ],
        |v| Filters::read(&v.query).archived,
        |v, a| {
            let mut f = Filters::read(&v.query);
            f.archived = a;
            v.query = f.apply(&v.query);
        },
    );
    let when = choice(
        __scope,
        state,
        "When",
        vec![
            ("", "Any time"),
            ("7d", "Week"),
            ("1m", "Month"),
            ("1y", "Year"),
        ],
        within,
        |v, w| {
            let mut f = Filters::read(&v.query);
            f.within = (!w.is_empty()).then(|| w.to_owned());
            v.query = f.apply(&v.query);
        },
    );
    let flags: Vec<NodeHandle> = [
        (
            "Unread",
            (|f: &mut Filters| &mut f.unread) as fn(&mut Filters) -> &mut bool,
        ),
        ("Starred", |f| &mut f.starred),
        ("Has an attachment", |f| &mut f.attachments),
    ]
    .into_iter()
    .map(|(name, flag)| {
        let on = move || {
            let mut f = filters(state);
            *flag(&mut f)
        };
        rsx! {
            button {
                class: {move || if on() { "view-chip on" } else { "view-chip" }},
                aria-pressed: {move || if on() { "true" } else { "false" }},
                onclick: move || change(state, move |f| { let v = flag(f); *v = !*v; }),
                {name}
            }
        }
    })
    .collect();
    let lasts: Vec<NodeHandle> = [
        (Last::Me, "I replied last"),
        (Last::Them, "They replied last"),
    ]
    .into_iter()
    .map(|(which, name)| {
        let on = move || filters(state).last == Some(which);
        rsx! {
            button {
                class: {move || if on() { "view-chip on" } else { "view-chip" }},
                aria-pressed: {move || if on() { "true" } else { "false" }},
                onclick: move || change(state, move |f| {
                    f.last = if f.last == Some(which) { None } else { Some(which) };
                }),
                {name}
            }
        }
    })
    .collect();
    rsx! {
        div { class: "panel-mail",
        {accounts(__scope, state)}
        {archived}
        {when}
        {people_picker(__scope, state, Side::From)}
        {people_picker(__scope, state, Side::To)}
        {label_picker(__scope, state)}
        {category_picker(__scope, state)}
        div { class: "panel-field",
            span { class: "panel-name", "Only" }
            div { class: "view-chips", {flags} {lasts} }
        }
        div { class: "panel-field panel-query",
            span { class: "panel-name", "Search text" }
            TextInput {
                spellcheck: false,
                size: "sm",
                value_fn: move || read(state, |e| e.draft.query.clone()),
                oninput: move |v: String| state.change_edit(|view| view.query = v),
            }
        }
        }
    }
}

/// The accounts it shows: all of them, or any of them.
#[component]
fn accounts(state: State) -> NodeHandle {
    let known: Vec<EmailAddress> = crate::ui::labels::sidebar_accounts(state);
    let several = known.len() > 1;
    let shown = move || read(state, |e| e.draft.accounts.clone());
    let chips: Vec<NodeHandle> = known
        .iter()
        .enumerate()
        .map(|(at, account)| {
            let (on, toggled) = (account.clone(), account.clone());
            let color = account_color(at);
            rsx! {
                button {
                    class: {move || if shown().contains(&on) { "view-chip on" } else { "view-chip" }},
                    onclick: move || {
                        let toggled = toggled.clone();
                        state.change_edit(move |v| {
                            if v.accounts.contains(&toggled) {
                                v.accounts.retain(|a| *a != toggled);
                            } else {
                                v.accounts.push(toggled);
                            }
                        })
                    },
                    span { class: "dot", style: {format!("background: {color}")} }
                    {account.to_string()}
                }
            }
        })
        .collect();
    rsx! {
        div { class: "panel-field", style: {if several { "" } else { "display: none" }},
            span { class: "panel-name", "Accounts" }
            div { class: "view-chips",
                button {
                    class: {move || if shown().is_empty() { "view-chip on" } else { "view-chip" }},
                    onclick: move || state.change_edit(|v| v.accounts.clear()),
                    "All accounts"
                }
                {chips}
            }
        }
    }
}
