//! The add-account dialog (design/mockups/add-imap-account.html): which
//! kind, the address, discovery's steps as they run, what was found with
//! the password or the browser sign-in, Google's mail offered as a Google
//! account, the wait for the browser and the check. The servers panel is
//! in `add_account_servers`; the state and slow work in
//! `add_account_run`.

use rinch::prelude::*;

use crate::add_account::{AddForm, Kind, Step, Way};
use crate::add_account_run::{AddUi, LOOK_STEPS, adding_address, look_line};
use crate::state::State;
use crate::ui::add_account_servers::servers_panel;
use postrinch_discover::{Discovery, Step as LookStep, StepState};
use postrinch_sync::imap::OAuthProvider;

#[component]
pub fn add_account_dialog(state: State) -> NodeHandle {
    let ui = state.adding;
    rsx! {
        Modal {
            opened_fn: move || ui.step.get().is_some(),
            onclose: move || ui.close(),
            title: "",
            size: "md",
            div { class: "add-dialog",
                match ui.step.get() {
                    Some(Step::Choose) => {choose(__scope, state)},
                    Some(Step::Address) => {address(__scope, state)},
                    Some(Step::LookingUp) => {looking(__scope, state)},
                    Some(Step::Found) => {found(__scope, state)},
                    Some(Step::Workspace) => {workspace(__scope, state)},
                    Some(Step::Servers) => {servers_panel(__scope, state)},
                    Some(Step::Browser) => {browser(__scope, state)},
                    Some(Step::Checking) => {checking(__scope, state)},
                    None => "",
                }
            }
        }
    }
}

/// The dialog's heading: what is being added, and the address.
pub fn head(__scope: &mut RenderScope, title: String, sub: String) -> NodeHandle {
    rsx! {
        div { class: "add-head",
            h2 { class: "dialog-title", {title} }
            p { class: "dialog-hint", {sub} }
        }
    }
}

/// What went wrong, on the step the person is back on.
pub fn error_line(__scope: &mut RenderScope, ui: AddUi) -> NodeHandle {
    rsx! {
        div {
            for error in ui.error.get() {
                p { key: error.clone(), class: "add-error", {error.clone()} }
            }
        }
    }
}

/// The dialog's title for the kind being added.
pub fn kind_title(form: &AddForm) -> String {
    match form.kind {
        Kind::Other => "Other mail account".to_owned(),
        Kind::Microsoft => "Microsoft account".to_owned(),
    }
}

/// A text field of the form, by what it reads and writes.
pub fn text_field(
    __scope: &mut RenderScope,
    ui: AddUi,
    label: &'static str,
    get: fn(&AddForm) -> String,
    set: fn(&mut AddForm, String),
) -> NodeHandle {
    rsx! {
        div { class: "add-field",
            span { class: "add-label", {label} }
            TextInput {
                spellcheck: false,
                size: "sm",
                value_fn: move || get(&ui.form.get()),
                oninput: move |v: String| ui.edit(|f| set(f, v)),
            }
        }
    }
}

/// A password field of the form: masked, and never shown back.
pub fn password_field(
    __scope: &mut RenderScope,
    ui: AddUi,
    label: String,
    get: fn(&AddForm) -> String,
    set: fn(&mut AddForm, String),
) -> NodeHandle {
    rsx! {
        div { class: "add-field",
            span { class: "add-label", {label} }
            PasswordInput {
                size: "sm",
                autocomplete: "off",
                value_fn: move || get(&ui.form.get()),
                oninput: move |v: String| ui.edit(|f| set(f, v)),
            }
        }
    }
}

#[component]
fn choose(state: State) -> NodeHandle {
    let ui = state.adding;
    rsx! {
        div { class: "dialog",
            {head(__scope, "Add an account".into(), "Which kind of account is it?".into())}
            div { class: "add-choice", onclick: move || ui.add_as_google(state),
                span { class: "add-choice-icon", "G" }
                div {
                    span { class: "add-choice-name", "Google" }
                    span { class: "add-choice-about", "Gmail and Google Workspace. Signs in in your browser; mail, labels and calendars." }
                }
            }
            div { class: "add-choice", onclick: move || ui.choose(Kind::Microsoft),
                span { class: "add-choice-icon", "M" }
                div {
                    span { class: "add-choice-name", "Microsoft" }
                    span { class: "add-choice-about", "Outlook.com, Hotmail and Microsoft 365. Signs in in your browser. Mail only." }
                }
            }
            div { class: "add-choice", onclick: move || ui.choose(Kind::Other),
                span { class: "add-choice-icon", "@" }
                div {
                    span { class: "add-choice-name", "Other mail account" }
                    span { class: "add-choice-about", "Fastmail, iCloud, Yahoo, your work's server: any IMAP and SMTP account. Mail only." }
                }
            }
            div { class: "dialog-actions",
                Button { variant: "default", size: "sm", onclick: move || ui.close(), "Cancel" }
            }
        }
    }
}

#[component]
fn address(state: State) -> NodeHandle {
    let ui = state.adding;
    let form = ui.form.get();
    let other = form.kind == Kind::Other;
    rsx! {
        div { class: "dialog",
            {head(__scope, kind_title(&form), "Postrinch looks up the servers from the address.".into())}
            {text_field(__scope, ui, "Email address", |f| f.address.clone(), |f, v| f.address = v)}
            {text_field(__scope, ui, "Your name", |f| f.name.clone(), |f, v| f.name = v)}
            {error_line(__scope, ui)}
            div { class: "dialog-actions",
                if other {
                    span { class: "add-link", onclick: move || by_hand(ui, Step::Address), "Set up servers by hand" }
                }
                span { class: "add-spacer" }
                Button { variant: "default", size: "sm", onclick: move || ui.close(), "Cancel" }
                Button { variant: "filled", size: "sm", onclick: move || ui.look_up(), "Continue" }
            }
        }
    }
}

/// "Set up servers by hand": the usual names guessed from the address.
fn by_hand(ui: AddUi, back: Step) {
    ui.edit(|f| {
        if f.parsed_address().is_ok() && f.imap.host.is_empty() {
            f.take_discovery(&Discovery::nothing(), |_| false);
        }
    });
    ui.servers(back);
}

#[component]
fn looking(state: State) -> NodeHandle {
    let ui = state.adding;
    let form = ui.form.get();
    let address = adding_address(&form);
    let domain = address.rsplit_once('@').map_or("", |(_, d)| d).to_owned();
    rsx! {
        div { class: "dialog",
            {head(__scope, kind_title(&form), address.clone())}
            div { class: "add-steps",
                for step in LOOK_STEPS {
                    div { key: format!("{step:?}"),
                        {look_step(__scope, ui, domain.clone(), step)}
                    }
                }
            }
            {error_line(__scope, ui)}
            div { class: "dialog-actions",
                span { class: "add-link", onclick: move || by_hand(ui, Step::Address), "Set up servers by hand" }
                span { class: "add-spacer" }
                Button { variant: "default", size: "sm", onclick: move || ui.go(Step::Address), "Back" }
            }
        }
    }
}

/// One of discovery's steps: waiting its turn, running, or what it found.
fn look_step(__scope: &mut RenderScope, ui: AddUi, domain: String, step: LookStep) -> NodeHandle {
    rsx! {
        div {
            class: {move || if seen(ui, step).is_some() { "add-step" } else { "add-step add-faint" }},
            {move || step_line(seen(ui, step), &domain, step)}
        }
    }
}

/// What discovery has said of `step` so far.
fn seen(ui: AddUi, step: LookStep) -> Option<StepState> {
    ui.looking
        .get()
        .into_iter()
        .find(|(s, _)| *s == step)
        .map(|(_, state)| state)
}

fn step_line(seen: Option<StepState>, domain: &str, step: LookStep) -> String {
    match seen {
        Some(state) => format!("{} {}", mark(state), look_line(domain, step, state)),
        None => {
            let waiting = look_line(domain, step, StepState::Running);
            format!("· {}", waiting.trim_end_matches('…'))
        }
    }
}

fn mark(state: StepState) -> &'static str {
    match state {
        StepState::Running => "…",
        StepState::Found => "✓",
        StepState::NothingFound => "–",
    }
}

/// The found box: who, the servers, and where they came from.
pub fn found_box(__scope: &mut RenderScope, ui: AddUi, form: &AddForm) -> NodeHandle {
    let domain = adding_address(form)
        .rsplit_once('@')
        .map_or(String::new(), |(_, d)| d.to_owned());
    let who = form.found.provider.clone().unwrap_or(domain);
    let source = form.found.source.unwrap_or("");
    let (imap, smtp) = (
        format!("Incoming  {}", form.imap.line()),
        format!("Outgoing  {}", form.smtp.line()),
    );
    rsx! {
        div { class: "add-found",
            div { class: "add-row",
                span { class: "add-who", {who} }
                span { class: "add-spacer" }
                span { class: "add-link", onclick: move || ui.servers(Step::Found), "Change" }
            }
            div { class: "add-server", {imap} }
            div { class: "add-server", {smtp} }
            div { class: "settings-hint", {source} }
        }
    }
}

/// "Sign in with Microsoft".
fn provider_name(provider: OAuthProvider) -> &'static str {
    match provider {
        OAuthProvider::Google => "Google",
        OAuthProvider::Microsoft => "Microsoft",
        _ => "the provider",
    }
}

#[component]
fn found(state: State) -> NodeHandle {
    let ui = state.adding;
    let form = ui.form.get();
    let sub = adding_address(&form);
    // Only a change of way redraws the part below the found box, not
    // each key typed into the password.
    let way = Memo::new(move || ui.form.get().way);
    rsx! {
        div { class: "dialog",
            {head(__scope, kind_title(&form), sub)}
            {found_box(__scope, ui, &form)}
            match way.get() {
                Way::Browser(provider) => {browser_way(__scope, state, provider)},
                Way::Password => {password_way(__scope, state)},
            }
        }
    }
}

#[component]
fn browser_way(state: State, provider: OAuthProvider) -> NodeHandle {
    let ui = state.adding;
    let name = provider_name(provider);
    let takes_passwords = ui.form.get().found.takes_passwords;
    rsx! {
        div { class: "dialog",
            p { class: "settings-hint",
                {format!("{name} signs you in in your browser. Postrinch keeps only a token, in the system keyring, and never sees your password.")}
            }
            {error_line(__scope, ui)}
            div { class: "dialog-actions",
                if takes_passwords {
                    span { class: "add-link", onclick: move || ui.edit(|f| f.way = Way::Password), "Use an app password instead" }
                }
                span { class: "add-spacer" }
                Button { variant: "default", size: "sm", onclick: move || ui.go(Step::Address), "Back" }
                Button { variant: "filled", size: "sm", onclick: move || ui.add(state), {format!("Sign in with {name}")} }
            }
        }
    }
}

#[component]
fn password_way(state: State) -> NodeHandle {
    let ui = state.adding;
    let form = ui.form.get();
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
            {error_line(__scope, ui)}
            div { class: "dialog-actions",
                span { class: "add-spacer" }
                Button { variant: "default", size: "sm", onclick: move || ui.go(Step::Address), "Back" }
                Button { variant: "filled", size: "sm", onclick: move || ui.add(state), "Add account" }
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

#[component]
fn workspace(state: State) -> NodeHandle {
    let ui = state.adding;
    let form = ui.form.get();
    let address = adding_address(&form);
    let domain = address.rsplit_once('@').map_or("", |(_, d)| d).to_owned();
    rsx! {
        div { class: "dialog",
            {head(__scope, kind_title(&form), address.clone())}
            div { class: "add-warn",
                {format!("{domain}'s mail is hosted by Google. Added as a Google account it keeps Gmail's labels, categories and calendars.")}
            }
            {error_line(__scope, ui)}
            div { class: "dialog-actions",
                Button {
                    variant: "default",
                    size: "sm",
                    onclick: move || {
                        ui.edit(AddForm::use_imap_anyway);
                        ui.go(Step::Found);
                    },
                    "Use IMAP anyway"
                }
                Button { variant: "filled", size: "sm", onclick: move || ui.add_as_google(state), "Add as a Google account" }
            }
        }
    }
}

#[component]
fn browser(state: State) -> NodeHandle {
    let ui = state.adding;
    let form = ui.form.get();
    rsx! {
        div { class: "dialog",
            {head(__scope, kind_title(&form), adding_address(&form))}
            div { class: "add-steps",
                div { class: "add-step", "… Finish signing in in your browser." }
            }
            div { class: "dialog-actions",
                span {
                    class: "add-link",
                    onclick: move || {
                        if let Some(page) = ui.browser_page.get()
                            && let Err(e) = open::that_detached(&page)
                        {
                            ui.error.set(Some(format!("The browser did not open: {e}")));
                        }
                    },
                    "Open the page again"
                }
                span { class: "add-spacer" }
                Button { variant: "default", size: "sm", onclick: move || ui.cancel_browser(), "Cancel" }
            }
        }
    }
}

#[component]
fn checking(state: State) -> NodeHandle {
    let ui = state.adding;
    let form = ui.form.get();
    let line = format!(
        "… Signing in to {} and {}",
        form.imap.host.trim(),
        form.smtp.host.trim()
    );
    let primary = if ui.changing.get().is_some() {
        "Check and save"
    } else {
        "Add account"
    };
    rsx! {
        div { class: "dialog",
            {head(__scope, kind_title(&form), adding_address(&form))}
            div { class: "add-steps",
                div { class: "add-step", {line} }
                div { class: "add-step add-faint", "No mail is sent: each server is only signed in to." }
            }
            div { class: "dialog-actions",
                Button { variant: "default", size: "sm", disabled: true, "Back" }
                Button { variant: "filled", size: "sm", disabled: true, {primary} }
            }
        }
    }
}

pub const CSS: &str = r#"
.add-dialog .dialog { gap: 10px; }
.add-head { display: flex; flex-direction: column; gap: 2px; }
.add-field { display: flex; flex-direction: column; gap: 4px; }
.add-label { font-size: 12px; font-weight: 600; color: var(--muted); }
.add-choice { display: flex; gap: 12px; align-items: flex-start; padding: 12px; border: 1px solid var(--line); border-radius: 8px; cursor: pointer; }
.add-choice:hover { background: var(--hover); }
.add-choice-icon { width: 22px; text-align: center; font-weight: 700; color: var(--accent); }
.add-choice-name { display: block; font-weight: 600; }
.add-choice-about { display: block; color: var(--muted); font-size: 12.5px; }
.add-found { border: 1px solid var(--line); border-radius: 8px; padding: 10px 12px; font-size: 13px; display: flex; flex-direction: column; gap: 2px; }
.add-row { display: flex; gap: 8px; align-items: center; }
.add-who { font-weight: 600; }
.add-server { color: var(--ink-2); font-family: ui-monospace, monospace; font-size: 12px; white-space: pre-wrap; overflow-wrap: anywhere; }
.add-warn { background: var(--warn-bg); border: 1px solid var(--warn-line); border-radius: 8px; padding: 9px 11px; font-size: 13px; }
.add-error { margin: 0; color: var(--danger); font-size: 13px; overflow-wrap: anywhere; }
.add-link { color: var(--accent); cursor: pointer; font-size: 13px; }
.add-spacer { flex: 1; }
.add-steps { display: flex; flex-direction: column; gap: 4px; font-size: 13px; }
.add-faint { color: var(--faint); }
.add-dialog .dialog-actions { align-items: center; flex-wrap: wrap; }
"#;
