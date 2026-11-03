//! Settings › Agents' dialogs, after their mockups: connecting an app (or
//! changing an agent's accounts), an agent's key shown once with the entry
//! to paste, and removing an agent.

use postrinch_agents::{Agent, App, Reach};
use postrinch_model::EmailAddress;
use rinch::prelude::*;

use crate::agents::{AgentDialog, Ready, reach_of};
use crate::state::State;

/// Whichever agent dialog is open.
#[component]
pub fn agent_dialogs(state: State) -> NodeHandle {
    rsx! {
        div {
        Modal {
            opened_fn: move || state.agent_dialog.get().is_some(),
            onclose: move || state.agent_dialog.set(None),
            title: "",
            size: "md",
            // Keyed so each opening starts fresh (rinch#2105: a loop, not
            // `if let`).
            for dialog in state.agent_dialog.get() {
                div { key: format!("{dialog:?}"),
                    {dialog_body(__scope, state, dialog.clone())}
                }
            }
        }
        Modal {
            opened_fn: move || state.agent_removing.get().is_some(),
            onclose: move || state.agent_removing.set(None),
            title: "",
            size: "sm",
            for agent in state.agent_removing.get() {
                div { key: agent.id.clone(),
                    {confirm_remove(__scope, state, agent.clone())}
                }
            }
        }
        }
    }
}

#[component]
fn dialog_body(state: State, dialog: AgentDialog) -> NodeHandle {
    match dialog {
        AgentDialog::Connect { app, editing } => connect(__scope, state, app, editing),
        AgentDialog::Ready(ready) => ready_dialog(__scope, state, ready),
    }
}

#[component]
fn connect(state: State, app: Option<App>, editing: Option<Agent>) -> NodeHandle {
    let (every0, chosen0) = match editing.as_ref().map(|a| &a.reach) {
        Some(Reach::Only(accounts)) => (false, accounts.clone()),
        _ => (true, Vec::new()),
    };
    let every = Signal::new(every0);
    let chosen: Signal<Vec<EmailAddress>> = Signal::new(chosen0);
    let name = Signal::new(match &editing {
        Some(agent) => agent.name.clone(),
        None => state.agent_name_for(app),
    });
    let title = match (&editing, app) {
        (Some(agent), _) => format!("What {} can see", agent.name),
        (None, Some(app)) => format!("Connect {}", app.name()),
        (None, None) => "Set up an app by hand".to_owned(),
    };
    let about = match &editing {
        Some(_) => {
            "It sees the change on its next call; nothing changes in the app\u{2019}s settings."
                .to_owned()
        }
        None => format!(
            "{} will be able to read and search the mail you choose, and mark, star, archive, trash and label it.",
            app.map_or("The app", App::name)
        ),
    };
    // Where the entry goes, and what the app is called: the entry's own
    // name follows the Name field.
    let note = app.filter(|_| editing.is_none()).map(|app| {
        let path = app.settings_path().map(|p| tilde(&p)).unwrap_or_default();
        (path, app.name())
    });
    let accounts: Vec<NodeHandle> = state
        .accounts
        .get()
        .into_iter()
        .map(|info| {
            let address = info.account.address.clone();
            let has = address.clone();
            rsx! {
                Checkbox {
                    label: address.to_string(),
                    checked_fn: move || chosen.get().contains(&has),
                    onchange: move || {
                        every.set(false);
                        chosen.update(|c| {
                            if let Some(at) = c.iter().position(|a| a == &address) {
                                c.remove(at);
                            } else {
                                c.push(address.clone());
                            }
                        });
                    },
                }
            }
        })
        .collect();
    let editing_now = editing.clone();
    let go = move || {
        let reach = reach_of(every.get(), &chosen.get());
        if matches!(&reach, Reach::Only(a) if a.is_empty()) {
            return;
        }
        match &editing_now {
            Some(agent) => state.change_reach(agent, reach),
            None => state.make_agent(app, &name.get(), reach),
        }
    };
    let action = match (&editing, app) {
        (Some(_), _) => "Save",
        (None, Some(_)) => "Connect",
        (None, None) => "Make its key",
    };
    rsx! {
        div { class: "agent-dialog",
            div { class: "dialog",
                h2 { class: "dialog-title", {title} }
                p { class: "dialog-hint", {about} }
            }
            if editing.is_none() {
                div { class: "agent-field",
                    span { class: "agent-field-name", "Name" }
                    TextInput {
                        value_fn: move || name.get(),
                        oninput: move |v: String| name.set(v),
                    }
                    span { class: "dialog-hint", "How it shows in your list of agents." }
                }
            }
            div { class: "agent-field",
                span { class: "agent-field-name", "What it can see" }
                div { class: "choices",
                    div {
                        class: {move || if every.get() { "choice chosen" } else { "choice" }},
                        onclick: move || every.set(true),
                        span { class: "choice-name", "Every account" }
                        span { class: "choice-about", "And any you add later." }
                    }
                    div {
                        class: {move || if every.get() { "choice" } else { "choice chosen" }},
                        onclick: move || every.set(false),
                        span { class: "choice-name", "Only these accounts" }
                        span { class: "choice-about", "To this agent, the other accounts do not exist." }
                        div { class: "agent-accounts", {accounts} }
                    }
                }
            }
            for (path, app_name) in note.clone() {
                div { key: path.clone(),
                    {entry_note(__scope, name, path.clone(), app_name)}
                }
            }
            div { class: "dialog-actions",
                Button { variant: "default", size: "sm", onclick: move || state.agent_dialog.set(None), "Cancel" }
                Button {
                    variant: "filled",
                    size: "sm",
                    onclick: go,
                    {action}
                }
            }
        }
    }
}

/// Where Connect puts the entry; its name follows the Name field.
#[component]
fn entry_note(name: Signal<String>, path: String, app_name: &'static str) -> NodeHandle {
    // rsx closures capture only `Copy` values.
    let path = Signal::new(path.clone());
    rsx! {
        p { class: "agent-note",
            {move || format!(
                "Adds a {} entry to {}, after a backup beside it. Restart {app_name} to connect.",
                postrinch_agents::setup::entry_name(&name.get()),
                path.get(),
            )}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Json,
    Toml,
    Command,
}

#[component]
fn ready_dialog(state: State, ready: Ready) -> NodeHandle {
    let format = Signal::new(Format::Json);
    let pick = |r: &Ready, f: Format| match f {
        Format::Json => r.json.clone(),
        Format::Toml => r.toml.clone(),
        Format::Command => r.command.clone(),
    };
    let (shown, copied) = (ready.clone(), ready.clone());
    let key = ready.key.clone();
    let tabs = vec![
        format_tab(__scope, format, Format::Json, "JSON"),
        format_tab(__scope, format, Format::Toml, "Codex (TOML)"),
        format_tab(__scope, format, Format::Command, "Command"),
    ];
    rsx! {
        div { class: "agent-dialog",
            div { class: "dialog",
                h2 { class: "dialog-title", {format!("{} is ready to connect", ready.name)} }
                p { class: "dialog-hint",
                    {format!("It sees: {}. Give the app the server below; it connects while Postrinch is open.", ready.reach)}
                }
            }
            for trouble in ready.trouble.clone() {
                p { key: trouble.clone(), class: "agent-warn", {trouble.clone()} }
            }
            div { class: "agent-field",
                div { class: "agent-key-head",
                    span { class: "agent-field-name", "Its key" }
                    span { class: "agent-once", "Shown this once" }
                }
                div { class: "agent-key",
                    code { {ready.key.clone()} }
                    Button { variant: "default", size: "xs", onclick: move || copy(&key), "Copy key" }
                }
                span { class: "dialog-hint", "Postrinch keeps only a fingerprint of it. Lost it? Remove this agent and connect the app again." }
            }
            div { class: "agent-field",
                div { class: "agent-tabs", {tabs} }
                pre { class: "agent-snippet", {move || pick(&shown, format.get())} }
                div { class: "agent-copy",
                    Button { variant: "default", size: "xs", onclick: move || copy(&pick(&copied, format.get())), "Copy" }
                    span { "Paste it into the app\u{2019}s MCP servers, then restart the app." }
                }
            }
            p { class: "agent-warn",
                "Anyone with this key can read the mail it reaches while Postrinch is open. Keep it in the app\u{2019}s settings, not in a shared file or a command line."
            }
            div { class: "dialog-actions",
                Button { variant: "filled", size: "sm", onclick: move || state.agent_dialog.set(None), "Done" }
            }
        }
    }
}

#[component]
fn format_tab(format: Signal<Format>, which: Format, label: &'static str) -> NodeHandle {
    rsx! {
        div {
            class: {move || if format.get() == which { "agent-tab chosen" } else { "agent-tab" }},
            onclick: move || format.set(which),
            {label}
        }
    }
}

#[component]
fn confirm_remove(state: State, agent: Agent) -> NodeHandle {
    let text = match &agent.setup {
        Some(setup) => format!(
            "Its key stops working at once, and its entry comes out of {}\u{2019}s settings.",
            setup.app.name()
        ),
        None => "Its key stops working at once. Take its entry out of the app\u{2019}s settings yourself.".to_owned(),
    };
    let title = format!("Remove {}?", agent.name);
    rsx! {
        div { class: "dialog",
            h2 { class: "dialog-title", {title} }
            p { class: "dialog-hint", {text} }
            div { class: "dialog-actions",
                Button { variant: "default", size: "sm", onclick: move || state.agent_removing.set(None), "Keep it" }
                Button {
                    variant: "filled",
                    color: "red",
                    size: "sm",
                    onclick: move || {
                        state.agent_removing.set(None);
                        state.remove_agent(&agent);
                    },
                    "Remove"
                }
            }
        }
    }
}

/// A path, with the home folder as `~`.
fn tilde(path: &std::path::Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(ToOwned::to_owned)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

fn copy(text: &str) {
    if let Err(e) = rinch::clipboard::copy_text(text) {
        tracing::warn!("copying failed: {e}");
    }
}
