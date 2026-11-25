//! An IMAP account in Settings › Accounts (design/mockups/
//! add-imap-account.html, "Server settings" and "Needs to sign in"): its
//! servers and sign-in, Server settings… or Sign in again…, the folders
//! it gives each role, and whether its server files sent mail itself.

use postrinch_commands::AccountInfo;
use postrinch_model::{EmailAddress, ImapSettings, SignInMethod};
use postrinch_sync::imap::{AccountFolder, OAuthProvider};
use rinch::prelude::*;

use crate::add_account::ServerForm;
use crate::imap_settings_run::{Role, chosen_folder, role_choices, role_value};
use crate::state::State;
use crate::ui::settings::{here_line, state_line, sync_line};

#[component]
pub fn imap_account_detail(state: State, info: AccountInfo, settings: ImapSettings) -> NodeHandle {
    let address = info.account.address.clone();
    let (line, trouble) = state_line(&info.account.state);
    let needs_sign_in = !info.account.is_active();
    let here = here_line(&info);
    let incoming = ServerForm::from_server(&settings.imap).line();
    let outgoing = ServerForm::from_server(&settings.smtp).line();
    let oauth = matches!(settings.sign_in, SignInMethod::OAuth2 { .. });
    let (syncing, remove) = (address.clone(), address.clone());
    // A refused sign-in is mended first; the servers otherwise.
    let first = if needs_sign_in {
        let again = address.clone();
        rsx! {
            Button { variant: "default", size: "sm", onclick: move || state.sign_in(Some(again.clone())), "Sign in again…" }
        }
    } else {
        let (servers, kept) = (address.clone(), settings.clone());
        rsx! {
            Button {
                variant: "default",
                size: "sm",
                onclick: move || state.adding.change_servers_of(servers.clone(), &kept),
                "Server settings…"
            }
        }
    };
    let removal = if oauth {
        "Removing an account deletes its mail from this computer and signs Postrinch out of it. Nothing is deleted from the server."
    } else {
        "Removing an account deletes its mail from this computer and forgets its password. Nothing is deleted from the server."
    };
    if state
        .imap
        .folders
        .get()
        .is_none_or(|(account, _)| account != address)
    {
        state.imap.load_folders(state, address.clone());
    }
    rsx! {
        div { class: "account-detail",
            h2 { {address.to_string()} }
            span { class: if trouble { "account-state trouble" } else { "account-state" },
                {format!("IMAP · {line}")}
            }
            div { class: "facts",
                span { class: "fact-name", "Sync" }
                span { {move || sync_line(state, &syncing)} }
                span { class: "fact-name", "On this computer" }
                span { {here} }
                span { class: "fact-name", "Incoming" }
                span { {incoming} }
                span { class: "fact-name", "Outgoing" }
                span { {outgoing} }
                span { class: "fact-name", "Signs in with" }
                span { {signs_in_with(&settings.sign_in)} }
            }
            div { class: "account-actions",
                {first}
                Button { variant: "default", size: "sm", onclick: move || state.sync_now(), "Sync now" }
                span { class: "spacer" }
                Button { variant: "outline", color: "red", size: "sm", onclick: move || state.removing.set(Some(remove.clone())), "Remove from Postrinch" }
            }
            {folders_section(__scope, state, address.clone(), settings.clone())}
            p { class: "settings-hint", {removal} }
        }
    }
}

/// How the account signs in, as its facts say it.
fn signs_in_with(method: &SignInMethod) -> String {
    match method {
        SignInMethod::Password => "A password, kept in the system keyring".to_owned(),
        SignInMethod::SeparateSmtpPassword => {
            "A password for each server, kept in the system keyring".to_owned()
        }
        SignInMethod::OAuth2 { provider } => {
            let who = OAuthProvider::from_id(provider).map_or(provider.as_str(), |p| match p {
                OAuthProvider::Google => "Google",
                OAuthProvider::Microsoft => "Microsoft",
                _ => provider.as_str(),
            });
            format!("{who}, in the browser; a token kept in the system keyring")
        }
    }
}

#[component]
fn folders_section(state: State, account: EmailAddress, settings: ImapSettings) -> NodeHandle {
    let loaded = state.imap.folders;
    let mine = account.clone();
    let files = settings.server_files_sent;
    let chosen = settings.server_files_sent_chosen;
    let (toggle, back) = (account.clone(), account.clone());
    let hint = if chosen {
        if files {
            "You said the server keeps a copy of what is sent, so Postrinch does not."
        } else {
            "You said the server keeps no copy of what is sent, so Postrinch puts one in Sent."
        }
    } else if files {
        "Postrinch takes it that the server does, and puts its own copy in Sent once it sees the server did not."
    } else {
        "Postrinch saw that the server keeps no copy of what is sent, so it puts one in Sent."
    };
    rsx! {
        section { class: "imap-folders",
            h3 { class: "imap-folders-head", "Folders" }
            for folders in loaded.get().filter(|(a, _)| *a == mine).map(|(_, f)| f) {
                div { key: format!("{folders:?}"), class: "facts",
                    for role in Role::ALL {
                        div { key: role.name(), class: "imap-role",
                            {role_row(__scope, state, account.clone(), role, settings.clone(), folders.clone())}
                        }
                    }
                }
            }
            Checkbox {
                label: "The server files sent mail in Sent itself",
                checked_fn: move || files,
                onchange: move || state.imap.set_files_sent(state, toggle.clone(), Some(!files)),
            }
            p { class: "settings-hint", {hint} }
            if chosen {
                span {
                    class: "add-link",
                    onclick: move || state.imap.set_files_sent(state, back.clone(), None),
                    "Let Postrinch notice again"
                }
            }
        }
    }
}

/// One role's name and its choice of folder.
fn role_row(
    __scope: &mut RenderScope,
    state: State,
    account: EmailAddress,
    role: Role,
    settings: ImapSettings,
    folders: Vec<AccountFolder>,
) -> NodeHandle {
    let data = role_choices(role, &folders, &settings.roles);
    let value = role_value(role.chosen(&settings.roles).as_deref());
    rsx! {
        div { class: "imap-role-row",
            span { class: "fact-name", {role.name()} }
            Select {
                size: "sm",
                data: data,
                value_fn: move || value.clone(),
                onchange: move |v: String| {
                    state.imap.set_role(state, account.clone(), role, chosen_folder(&v));
                },
            }
        }
    }
}

pub const CSS: &str = r#"
.imap-folders { display: flex; flex-direction: column; gap: 10px; padding-top: 12px; border-top: 1px solid var(--line-soft); }
.imap-folders-head { margin: 0; font-size: 13px; font-weight: 600; }
.imap-role { display: contents; }
.imap-role-row { display: contents; }
.imap-role-row .fact-name { align-self: center; }
"#;
