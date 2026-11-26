//! The add-account dialog's found step (design/mockups/add-imap-account.html,
//! steps 4 and 4a): what was found, then the way in: the browser sign-in,
//! the password (or app password, with the provider's page for making
//! one), or, for a provider that signs in only in the browser without
//! that sign-in here, why there is none. The import from Thunderbird
//! shows each account's way in with the same parts.

use rinch::prelude::*;

use crate::add_account::{AddForm, Step, Way};
use crate::add_account_run::{AddUi, adding_address};
use crate::state::State;
use crate::ui::add_account::{error_line, found_box, head, kind_title, password_field};
use postrinch_sync::imap::OAuthProvider;

/// "Sign in with Microsoft".
pub fn provider_name(provider: OAuthProvider) -> &'static str {
    match provider {
        OAuthProvider::Google => "Google",
        OAuthProvider::Microsoft => "Microsoft",
        _ => "the provider",
    }
}

#[component]
pub fn found(state: State) -> NodeHandle {
    let ui = state.adding;
    let form = ui.form.get();
    let sub = adding_address(&form);
    // Only a change of way redraws the part below the found box, not
    // each key typed into the password. A provider that signs in only in
    // the browser, without that sign-in here, is asked for no password.
    let way = Memo::new(move || {
        let form = ui.form.get();
        (form.way(), form.asks_password())
    });
    rsx! {
        div { class: "dialog",
            {head(__scope, kind_title(&form), sub)}
            {found_box(__scope, ui, &form, Step::Found)}
            match way.get() {
                (Way::Browser(provider), _) => {browser_way(__scope, state, provider)},
                (Way::Password, true) => {password_way(__scope, state)},
                (Way::Password, false) => {no_way(__scope, state)},
            }
        }
    }
}

/// No way in: why, and Back.
#[component]
fn no_way(state: State) -> NodeHandle {
    let ui = state.adding;
    let why = ui.form.get().no_way_in().unwrap_or_default();
    rsx! {
        div { class: "dialog",
            p { class: "add-error", {why} }
            div { class: "dialog-actions",
                span { class: "add-spacer" }
                Button { variant: "default", size: "sm", onclick: move || ui.go(Step::Address), "Back" }
            }
        }
    }
}

#[component]
fn browser_way(state: State, provider: OAuthProvider) -> NodeHandle {
    let ui = state.adding;
    let name = provider_name(provider);
    // The browser is the only way in: Google and Microsoft, the providers
    // Postrinch signs in to there, take no passwords from it.
    rsx! {
        div { class: "dialog",
            {browser_hint(__scope, name)}
            {error_line(__scope, ui)}
            div { class: "dialog-actions",
                span { class: "add-spacer" }
                Button { variant: "default", size: "sm", onclick: move || ui.go(Step::Address), "Back" }
                Button { variant: "filled", size: "sm", onclick: move || ui.add(state), {format!("Sign in with {name}")} }
            }
        }
    }
}

/// What the browser sign-in does with the person's password.
pub fn browser_hint(__scope: &mut RenderScope, name: &str) -> NodeHandle {
    rsx! {
        p { class: "settings-hint",
            {format!("{name} signs you in in your browser. Postrinch keeps only a token, in the system keyring, and never sees your password.")}
        }
    }
}

#[component]
fn password_way(state: State) -> NodeHandle {
    let ui = state.adding;
    rsx! {
        div { class: "dialog",
            {password_body(__scope, ui, &ui.form.get())}
            {error_line(__scope, ui)}
            div { class: "dialog-actions",
                span { class: "add-spacer" }
                Button { variant: "default", size: "sm", onclick: move || ui.go(Step::Address), "Back" }
                Button { variant: "filled", size: "sm", onclick: move || ui.add(state), "Add account" }
            }
        }
    }
}

/// The password, or app password with the provider's page for making
/// one, and the trust asked for servers nobody vouches for.
pub fn password_body(__scope: &mut RenderScope, ui: AddUi, form: &AddForm) -> NodeHandle {
    let app_page = form.found.app_password_url.clone();
    let who = form
        .found
        .provider
        .clone()
        .unwrap_or_else(|| "This provider".to_owned());
    let label = if app_page.is_some() {
        "App password"
    } else {
        "Password"
    };
    let confirm = form.found.confirm_servers;
    rsx! {
        div { class: "dialog",
            for url in app_page.clone() {
                div { key: url.clone(), class: "add-warn",
                    {format!("{who} needs an app password for mail apps, not your usual one. ")}
                    {app_password_link(__scope, ui, url.clone(), who.clone())}
                }
            }
            {password_field(__scope, ui, label.to_owned(), |f| f.password.clone(), |f, v| f.password = v)}
            p { class: "settings-hint", "Kept in the system keyring, never in Postrinch's files." }
            if confirm {
                Checkbox {
                    label: "These servers are outside the address's domain, and I trust them",
                    checked_fn: move || ui.form.get().trusted,
                    onchange: move || ui.edit(|f| f.trusted = !f.trusted),
                }
            }
        }
    }
}

/// "Make one at Fastmail ↗": the provider's page, in the browser.
pub fn app_password_link(
    __scope: &mut RenderScope,
    ui: AddUi,
    url: String,
    who: String,
) -> NodeHandle {
    rsx! {
        span {
            class: "add-link",
            onclick: move || {
                if let Err(e) = open::that_detached(&url) {
                    ui.error.set(Some(format!("The browser did not open: {e}")));
                }
            },
            {format!("Make one at {who} ↗")}
        }
    }
}
