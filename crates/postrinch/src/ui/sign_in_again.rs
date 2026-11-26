//! Sign in again, for an IMAP account that signs in with a password
//! (design/mockups/add-imap-account.html, "Sign in again"): the password
//! only, as the servers stay. Both servers check it before it is kept.

use rinch::prelude::*;

use crate::state::State;

#[component]
pub fn sign_in_again_dialog(state: State) -> NodeHandle {
    let ui = state.imap;
    // "Server settings": the servers panel for the account, in place of
    // this dialog.
    let servers = move || {
        let Some((account, _)) = ui.again.get() else {
            return;
        };
        let settings = state
            .accounts
            .get()
            .into_iter()
            .find(|a| a.account.address == account)
            .and_then(|a| a.account.imap);
        ui.close_sign_in_again();
        if let Some(settings) = settings {
            state.adding.change_servers_of(account, &settings);
        }
    };
    rsx! {
        Modal {
            opened_fn: move || ui.again.get().is_some(),
            onclose: move || ui.close_sign_in_again(),
            title: "",
            size: "sm",
            div { class: "dialog",
                div { class: "add-head",
                    h2 { class: "dialog-title", "Sign in again" }
                    p { class: "dialog-hint", {move || ui.again.get().map(|(account, reason)| format!("{account} · {reason}")).unwrap_or_default()} }
                }
                div { class: "add-field",
                    span { class: "add-label", "Password" }
                    PasswordInput {
                        size: "sm",
                        autocomplete: "off",
                        value_fn: move || ui.password.get(),
                        oninput: move |v: String| ui.password.set(v),
                        onsubmit: move || ui.sign_in_again(state),
                    }
                }
                if ui.again_separate.get() {
                    div { class: "add-field",
                        span { class: "add-label", "Outgoing password" }
                        PasswordInput {
                            size: "sm",
                            autocomplete: "off",
                            value_fn: move || ui.smtp_password.get(),
                            oninput: move |v: String| ui.smtp_password.set(v),
                        }
                    }
                }
                p { class: "settings-hint", "Kept in the system keyring, never in Postrinch's files." }
                for error in ui.again_error.get() {
                    p { key: error.clone(), class: "add-error", {error.clone()} }
                }
                div { class: "dialog-actions",
                    span { class: "add-link", onclick: servers, "Server settings" }
                    span { class: "add-spacer" }
                    Button { variant: "default", size: "sm", onclick: move || ui.close_sign_in_again(), "Cancel" }
                    Button {
                        variant: "filled",
                        size: "sm",
                        disabled: {move || ui.again_busy.get()},
                        onclick: move || ui.sign_in_again(state),
                        {move || if ui.again_busy.get() { "Checking…" } else { "Sign in" }}
                    }
                }
            }
        }
    }
}
