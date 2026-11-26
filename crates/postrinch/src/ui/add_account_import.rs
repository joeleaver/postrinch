//! Importing from Thunderbird in the add-account dialog
//! (design/mockups/add-imap-account.html, T1 to T5): the accounts found,
//! by profile, to pick from; each picked account's turn, with the sign-in
//! it would get when added by hand, Skip this account and Stop importing;
//! and what came of each. The check, the servers panel and the browser's
//! wait are the dialog's own steps.

use rinch::prelude::*;

use crate::add_account::{Step, Way};
use crate::add_account_run::{AddUi, adding_address};
use crate::import_thunderbird::{Ended, Group, Route, Row};
use crate::state::State;
use crate::ui::add_account::{error_line, found_box, head};
use crate::ui::add_account_sign_in::{browser_hint, password_body, provider_name};

#[component]
pub fn import_list(state: State) -> NodeHandle {
    let ui = state.adding;
    let listing = ui.import.get().map(|i| i.listing).unwrap_or_default();
    // Profiles are named only when there is more than one.
    let named = listing.groups.len() > 1;
    let groups: Vec<NodeHandle> = listing
        .groups
        .iter()
        .map(|g| group(__scope, ui, g, named))
        .collect();
    let trouble: Vec<NodeHandle> = listing
        .trouble
        .iter()
        .map(|line| rsx! { p { class: "add-error", {line.clone()} } })
        .collect();
    let picked = Memo::new(move || ui.import.get().map_or(0, |i| i.picked.len()));
    rsx! {
        div { class: "dialog",
            {head(__scope, "Import from Thunderbird".into(), "Accounts found in Thunderbird on this computer.".into())}
            {trouble}
            div { class: "add-imp-list", {groups} }
            p { class: "settings-hint", "Passwords stay in Thunderbird; you'll sign in to each account once." }
            {error_line(__scope, ui)}
            div { class: "dialog-actions",
                span { class: "add-spacer" }
                Button { variant: "default", size: "sm", onclick: move || ui.go(Step::Choose), "Cancel" }
                Button {
                    variant: "filled",
                    size: "sm",
                    disabled: {move || picked.get() == 0},
                    onclick: move || ui.start_import(),
                    {move || import_label(picked.get())}
                }
            }
        }
    }
}

/// "Import 3 accounts".
pub fn import_label(picked: usize) -> String {
    match picked {
        1 => "Import 1 account".to_owned(),
        n => format!("Import {n} accounts"),
    }
}

fn group(__scope: &mut RenderScope, ui: AddUi, group: &Group, named: bool) -> NodeHandle {
    let label = named.then(|| rsx! { div { class: "add-imp-profile", {group.label.clone()} } });
    let problem = group
        .problem
        .clone()
        .map(|p| rsx! { p { class: "add-imp-problem", {p} } });
    let rows: Vec<NodeHandle> = group.rows.iter().map(|r| row(__scope, ui, r)).collect();
    rsx! {
        div { class: "add-imp-group",
            {label}
            {problem}
            {rows}
        }
    }
}

fn row(__scope: &mut RenderScope, ui: AddUi, row: &Row) -> NodeHandle {
    let id = row.id;
    let can = row.route.is_some();
    let name = if row.name.is_empty() {
        String::new()
    } else {
        format!(" · {}", row.name)
    };
    rsx! {
        div { class: if can { "add-imp" } else { "add-imp add-imp-off" },
            Checkbox {
                size: "sm",
                disabled: !can,
                checked_fn: move || ui.import.get().is_some_and(|i| i.picked.contains(&id)),
                onchange: move || ui.toggle_import(id),
            }
            div { class: "add-imp-body",
                div {
                    span { class: "add-imp-who", {row.who.clone()} }
                    span { class: "add-imp-name", {name} }
                }
                div { class: "add-imp-srv", {row.server.clone()} }
            }
            span { class: "add-imp-how", {row.how.clone()} }
        }
    }
}

/// The current account's turn. A skip or a hand-off to Google's sign-in
/// moves to the next account without leaving this step, so the turn is
/// keyed by its row and built afresh for each account.
#[component]
pub fn import_sign_in(state: State) -> NodeHandle {
    let ui = state.adding;
    rsx! {
        div { class: "add-imp-turn",
            for id in ui.import.get().and_then(|i| i.current().map(|r| r.id)) {
                div { key: id, class: "add-imp-turn", {turn(__scope, state)} }
            }
        }
    }
}

fn turn(__scope: &mut RenderScope, state: State) -> NodeHandle {
    let ui = state.adding;
    let form = ui.form.get();
    let import = ui.import.get().unwrap_or_default();
    let google = matches!(
        import.current().and_then(|r| r.route.as_ref()),
        Some(Route::Google(_))
    );
    let sub = format!("{} · {}", adding_address(&form), import.position());
    let (body, primary) = if google {
        (google_box(__scope), "Sign in with Google".to_owned())
    } else {
        let found = found_box(__scope, ui, &form, Step::ImportSignIn);
        let no_smtp = form.smtp.host.trim().is_empty().then(|| {
            rsx! { div { class: "add-warn", "Thunderbird has no outgoing server for this account: Change sets one." } }
        });
        let (way, primary) = match form.way {
            Way::Browser(provider) => {
                let name = provider_name(provider);
                (browser_hint(__scope, name), format!("Sign in with {name}"))
            }
            Way::Password => (password_body(__scope, ui, &form), "Add account".to_owned()),
        };
        (
            rsx! { div { class: "dialog", {found} {no_smtp} {way} } },
            primary,
        )
    };
    rsx! {
        div { class: "dialog",
            {head(__scope, "Import from Thunderbird".into(), sub)}
            {body}
            {error_line(__scope, ui)}
            div { class: "dialog-actions",
                span { class: "add-link", onclick: move || ui.stop_import(), "Stop importing" }
                span { class: "add-spacer" }
                Button { variant: "default", size: "sm", onclick: move || ui.skip_import(), "Skip this account" }
                Button {
                    variant: "filled",
                    size: "sm",
                    onclick: move || {
                        if google {
                            ui.import_google(state);
                        } else {
                            ui.add(state);
                        }
                    },
                    {primary}
                }
            }
        }
    }
}

/// A Gmail account comes in as a Google account.
fn google_box(__scope: &mut RenderScope) -> NodeHandle {
    rsx! {
        div { class: "dialog",
            div { class: "add-found",
                span { class: "add-who", "Google" }
                div { class: "settings-hint", "Added as a Google account: mail, labels and calendars." }
                div { class: "settings-hint", "From Thunderbird's settings" }
            }
            {browser_hint(__scope, "Google")}
        }
    }
}

#[component]
pub fn import_done(state: State) -> NodeHandle {
    let ui = state.adding;
    let import = ui.import.get().unwrap_or_default();
    let title = if import.stopped() {
        "Import stopped"
    } else {
        "Imported from Thunderbird"
    };
    let total = import.queue.len();
    let sub = format!(
        "{} of {total} account{} added.",
        import.added(),
        if total == 1 { "" } else { "s" }
    );
    let lines: Vec<NodeHandle> = import
        .ended
        .iter()
        .map(|(who, ended)| {
            let (mark, class) = match ended {
                Ended::Added => ("✓", "add-step"),
                Ended::ToGoogle => ("…", "add-step"),
                Ended::Skipped | Ended::NotAdded => ("–", "add-step add-faint"),
            };
            let line = format!("{mark} {who} · {}", ended.text());
            rsx! { div { class: class, {line} } }
        })
        .collect();
    let left = import
        .ended
        .iter()
        .any(|(_, e)| matches!(e, Ended::Skipped | Ended::NotAdded));
    rsx! {
        div { class: "dialog",
            {head(__scope, title.into(), sub)}
            div { class: "add-steps", {lines} }
            if left {
                p { class: "settings-hint", "Accounts not added can be imported later: Account › Add account… › Import from Thunderbird." }
            }
            div { class: "dialog-actions",
                span { class: "add-spacer" }
                Button { variant: "filled", size: "sm", onclick: move || ui.close(), "Done" }
            }
        }
    }
}

pub const CSS: &str = r#"
.add-imp-turn { display: contents; }
.add-imp-list { display: flex; flex-direction: column; gap: 6px; max-height: 360px; overflow-y: auto; padding-right: 10px; }
.add-imp-group { display: flex; flex-direction: column; gap: 6px; }
.add-imp-profile { margin-top: 4px; font-size: 12px; font-weight: 600; color: var(--muted); }
.add-imp-problem { margin: 0; font-size: 12.5px; color: var(--trouble); }
.add-imp { display: flex; gap: 8px; align-items: center; padding: 8px 10px; border: 1px solid var(--line); border-radius: 8px; font-size: 13px; }
.add-imp-body { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.add-imp-who { font-weight: 600; }
.add-imp-name { color: var(--muted); }
.add-imp-srv { color: var(--ink-2); font-family: ui-monospace, monospace; font-size: 12px; overflow-wrap: anywhere; }
.add-imp-how { color: var(--ink-2); font-size: 12.5px; text-align: right; max-width: 170px; }
.add-imp-off { background: var(--panel); }
.add-imp-off .add-imp-who, .add-imp-off .add-imp-name, .add-imp-off .add-imp-srv, .add-imp-off .add-imp-how { color: var(--faint); }
"#;
