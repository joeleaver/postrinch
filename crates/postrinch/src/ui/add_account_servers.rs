//! The servers panel (design/mockups/add-imap-account.html, "By hand"):
//! each server's name, port, security and user name, filled in from
//! discovery or its guesses. It opens from the address ("Set up servers
//! by hand"), from Change, from a check that could not reach a server,
//! and from "Server settings…" on an IMAP account, where saving checks
//! both servers again before anything changes.

use postrinch_model::Security;
use rinch::prelude::*;

use crate::add_account::{AddForm, ServerForm};
use crate::add_account_run::{AddUi, adding_address};
use crate::state::State;
use crate::ui::add_account::{error_line, head, kind_title, password_field};

#[component]
pub fn servers_panel(state: State) -> NodeHandle {
    let ui = state.adding;
    let form = ui.form.get();
    let changing = ui.changing.get().is_some();
    let address = adding_address(&form);
    let (title, sub) = if changing {
        (
            "Server settings".to_owned(),
            format!("{address} · both servers are checked before anything changes"),
        )
    } else if form.found.source.is_none() {
        (
            kind_title(&form),
            format!("{address} · nothing was found, so these are guesses"),
        )
    } else {
        (kind_title(&form), address)
    };
    // A password is asked for only when adding with one, to a provider
    // that takes them; the servers of an account here keep the sign-in it
    // has.
    let asks_password = !changing && form.asks_password();
    let separate = Memo::new(move || ui.form.get().separate_smtp);
    rsx! {
        div { class: "dialog",
            {head(__scope, title, sub)}
            div { class: "add-servers",
                h4 { class: "add-servers-head", "Incoming (IMAP)" }
                {server_fields(__scope, ui, Side::Imap)}
                h4 { class: "add-servers-head", "Outgoing (SMTP)" }
                {server_fields(__scope, ui, Side::Smtp)}
            }
            if asks_password {
                div { class: "dialog",
                    {password_field(__scope, ui, "Password".into(), |f| f.password.clone(), |f, v| f.password = v)}
                    Checkbox {
                        label: "The outgoing server has its own password",
                        checked_fn: move || separate.get(),
                        onchange: move || ui.edit(|f| f.separate_smtp = !f.separate_smtp),
                    }
                    if separate.get() {
                        {password_field(__scope, ui, "Outgoing password".into(), |f| f.smtp_password.clone(), |f, v| f.smtp_password = v)}
                    }
                }
            }
            {error_line(__scope, ui)}
            div { class: "dialog-actions",
                span { class: "add-spacer" }
                Button { variant: "default", size: "sm", onclick: move || back(ui), "Back" }
                Button {
                    variant: "filled",
                    size: "sm",
                    onclick: move || {
                        if ui.changing.get().is_some() {
                            ui.save_servers(state);
                        } else {
                            ui.add(state);
                        }
                    },
                    {if changing { "Check and save" } else { "Check and add" }}
                }
            }
        }
    }
}

/// Back: where the panel came from; for an account's servers, the
/// account, unchanged.
fn back(ui: AddUi) {
    if ui.changing.get().is_some() {
        ui.close();
    } else {
        ui.go(ui.servers_back.get());
    }
}

/// Which server of the form.
#[derive(Clone, Copy)]
enum Side {
    Imap,
    Smtp,
}

impl Side {
    fn of(self, form: &AddForm) -> &ServerForm {
        match self {
            Side::Imap => &form.imap,
            Side::Smtp => &form.smtp,
        }
    }

    fn of_mut(self, form: &mut AddForm) -> &mut ServerForm {
        match self {
            Side::Imap => &mut form.imap,
            Side::Smtp => &mut form.smtp,
        }
    }
}

fn server_fields(__scope: &mut RenderScope, ui: AddUi, side: Side) -> NodeHandle {
    let security = vec![
        SelectOption::new("tls", "TLS"),
        SelectOption::new("starttls", "STARTTLS"),
    ];
    let get = move |read: fn(&ServerForm) -> String| move || read(side.of(&ui.form.get()));
    rsx! {
        div { class: "add-server-grid",
            div { class: "add-field add-host",
                span { class: "add-label", "Server" }
                TextInput {
                    spellcheck: false,
                    size: "sm",
                    value_fn: get(|s| s.host.clone()),
                    oninput: move |v: String| ui.edit(|f| side.of_mut(f).host = v),
                }
            }
            div { class: "add-field add-port",
                span { class: "add-label", "Port" }
                TextInput {
                    spellcheck: false,
                    size: "sm",
                    inputmode: "numeric",
                    value_fn: get(|s| s.port.clone()),
                    oninput: move |v: String| ui.edit(|f| side.of_mut(f).port = v),
                }
            }
            div { class: "add-field add-security",
                span { class: "add-label", "Security" }
                Select {
                    size: "sm",
                    data: security,
                    value_fn: get(|s| security_value(s.security).to_owned()),
                    onchange: move |v: String| ui.edit(|f| side.of_mut(f).security = security_of(&v)),
                }
            }
            div { class: "add-field add-user",
                span { class: "add-label", "User name" }
                TextInput {
                    spellcheck: false,
                    size: "sm",
                    value_fn: get(|s| s.username.clone()),
                    oninput: move |v: String| ui.edit(|f| side.of_mut(f).username = v),
                }
            }
        }
    }
}

fn security_value(security: Security) -> &'static str {
    match security {
        Security::Tls => "tls",
        Security::StartTls => "starttls",
    }
}

fn security_of(value: &str) -> Security {
    match value {
        "starttls" => Security::StartTls,
        _ => Security::Tls,
    }
}

pub const CSS: &str = r#"
.add-servers { display: flex; flex-direction: column; gap: 6px; }
.add-servers-head { margin: 6px 0 0; font-size: 12.5px; font-weight: 600; }
.add-server-grid { display: grid; grid-template-columns: minmax(0, 1fr) 72px 120px; gap: 6px 8px; align-items: end; }
.add-user { grid-column: 1 / -1; }
/* An input keeps its intrinsic width unless told to fit its cell. */
.add-server-grid .rinch-text-input__input { width: 100%; min-width: 0; }
/* rinch's small Select is shorter than its small TextInput (2rem against
   2.25rem); on one row they must line up. */
.add-security .rinch-select__input { height: 2.25rem; }
@media (max-width: 720px) {
    .add-server-grid { grid-template-columns: minmax(0, 1fr) 64px; }
    .add-security { grid-column: 1 / -1; }
}
"#;
