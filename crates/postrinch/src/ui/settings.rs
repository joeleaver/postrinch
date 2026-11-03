//! Settings: for now its Accounts page, after the Settings › Accounts
//! mockup. Other sections arrive with what they configure.

use postrinch_commands::AccountInfo;
use postrinch_model::{AccountState, EmailAddress};
use rinch::prelude::*;

use crate::state::{State, View};
use crate::sync::thousands;
use crate::ui::account_color;

#[component]
pub fn settings(state: State) -> NodeHandle {
    rsx! {
        div { class: "settings",
            nav { class: "settings-nav",
                div { class: "settings-title", "Settings" }
                div { class: "folder chosen", "Accounts" }
                div { class: "settings-back", onclick: move || state.view.set(View::Mail), "← Back to mail" }
            }
            section { class: "settings-accounts",
                div { class: "settings-accounts-head",
                    h1 { "Accounts" }
                    Button { variant: "filled", size: "xs", onclick: move || state.sign_in(None), "Add Google account" }
                }
                for (i, info) in state.accounts.get().into_iter().enumerate() {
                    div { key: format!("{}|{:?}", info.account.address.key(), info.account.state),
                        {account_row(__scope, state, info.clone(), i)}
                    }
                }
                if state.accounts.get().is_empty() {
                    p { class: "settings-hint", "No accounts yet. Add a Google account to start." }
                }
            }
            main { class: "settings-main",
                for notice in state.notice.get() {
                    div {
                        key: notice.text.clone(),
                        class: if notice.trouble { "notice trouble" } else { "notice" },
                        {notice.text.clone()}
                    }
                }
                for info in shown(state) {
                    div { key: format!("{info:?}"),
                        {account_detail(__scope, state, info.clone())}
                    }
                }
            }
            {confirm_removal(__scope, state)}
        }
    }
}

/// The account the detail pane shows: the chosen one, or the first.
fn shown(state: State) -> Option<AccountInfo> {
    let accounts = state.accounts.get();
    let chosen = state.account_shown.get();
    accounts
        .iter()
        .find(|a| Some(&a.account.address) == chosen.as_ref())
        .or_else(|| accounts.first())
        .cloned()
}

fn state_line(state: &AccountState) -> (String, bool) {
    match state {
        AccountState::Active => ("Signed in".to_owned(), false),
        AccountState::NeedsSignIn(reason) => (format!("Needs to sign in: {reason}"), true),
    }
}

#[component]
fn account_row(state: State, info: AccountInfo, index: usize) -> NodeHandle {
    let address = info.account.address.clone();
    let chosen = address.clone();
    let (line, trouble) = state_line(&info.account.state);
    let dot = format!("background: {}", account_color(index));
    rsx! {
        div {
            class: {move || {
                let here = shown(state).is_some_and(|s| s.account.address == chosen);
                if here { "account-row chosen" } else { "account-row" }
            }},
            onclick: move || state.account_shown.set(Some(address.clone())),
            span { class: "dot", style: {dot.clone()} }
            div { class: "account-row-text",
                span { class: "account-address", {info.account.address.to_string()} }
                span { class: if trouble { "account-state trouble" } else { "account-state" }, {line} }
            }
        }
    }
}

#[component]
fn account_detail(state: State, info: AccountInfo) -> NodeHandle {
    let address = info.account.address.clone();
    let (line, trouble) = state_line(&info.account.state);
    let syncing = address.clone();
    let here = format!(
        "{} messages · {} downloaded whole",
        thousands(info.messages),
        thousands(info.downloaded)
    );
    let (again, remove) = (address.clone(), address.clone());
    rsx! {
        div { class: "account-detail",
            h2 { {address.to_string()} }
            span { class: if trouble { "account-state trouble" } else { "account-state" },
                {format!("Google · {line}")}
            }
            div { class: "facts",
                span { class: "fact-name", "Sync" }
                span { {move || sync_line(state, &syncing)} }
                span { class: "fact-name", "On this computer" }
                span { {here} }
            }
            div { class: "account-actions",
                Button { variant: "default", size: "sm", onclick: move || state.sign_in(Some(again.clone())), "Sign in again" }
                Button { variant: "default", size: "sm", onclick: move || state.sync_now(), "Sync now" }
                span { class: "spacer" }
                Button { variant: "outline", color: "red", size: "sm", onclick: move || state.removing.set(Some(remove.clone())), "Remove from Postrinch" }
            }
            p { class: "settings-hint",
                "Removing an account deletes its mail from this computer and signs Postrinch out of it. Nothing is deleted from Gmail."
            }
        }
    }
}

/// What an account's sync is doing, as the sidebar says it.
fn sync_line(state: State, account: &EmailAddress) -> String {
    state
        .sync
        .get()
        .into_iter()
        .find(|s| &s.account == account)
        .map(|s| s.line)
        .unwrap_or_else(|| "Waiting for the next check".to_owned())
}

/// Asks before an account is removed.
#[component]
fn confirm_removal(state: State) -> NodeHandle {
    let confirm = move || {
        if let Some(account) = state.removing.get() {
            state.remove_account(account);
        }
        state.removing.set(None);
    };
    rsx! {
        Modal {
            opened_fn: move || state.removing.get().is_some(),
            onclose: move || state.removing.set(None),
            title: "",
            size: "sm",
            div { class: "dialog",
                h2 { class: "dialog-title", "Remove this account?" }
                p { class: "dialog-hint", {move || removal_text(state.removing.get().as_ref())} }
                div { class: "dialog-actions",
                    Button { variant: "default", size: "sm", onclick: move || state.removing.set(None), "Keep it" }
                    Button { variant: "filled", color: "red", size: "sm", onclick: confirm, "Remove" }
                }
            }
        }
    }
}

fn removal_text(account: Option<&EmailAddress>) -> String {
    match account {
        Some(a) => format!(
            "{a} and all its mail will be deleted from this computer, and Postrinch will be signed out of it. Nothing is deleted from Gmail; you can add it again later."
        ),
        None => String::new(),
    }
}
