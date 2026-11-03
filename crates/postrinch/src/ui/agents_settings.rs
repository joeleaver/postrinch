//! Settings › Agents, after its mockup: whether agents can connect, the
//! apps to connect, and the agents connected.

use postrinch_agents::{Agent, App};
use rinch::prelude::*;

use crate::agents::{AgentDialog, reach_line};
use crate::dates;
use crate::state::State;

#[component]
pub fn agents_page(state: State) -> NodeHandle {
    let mut cards: Vec<NodeHandle> = Vec::new();
    for app in App::ALL.into_iter().map(Some).chain([None]) {
        cards.push(app_card(__scope, state, app));
    }
    rsx! {
        main { class: "settings-main",
            div { class: "agents-head",
                h1 { class: "settings-heading", "Agents" }
                p { class: "agents-intro",
                    "Apps that read and change your mail through Postrinch, over MCP. Each has its own key and sees only the accounts you give it."
                }
                div { class: "agents-status",
                    span { class: {move || if state.agents_off.get().is_some() { "status-dot off" } else { "status-dot" }} }
                    span { {move || state.agents_off.get().unwrap_or_else(|| "Agents can connect while Postrinch is open.".into())} }
                }
            }
            for notice in state.notice.get() {
                div { key: notice.text.clone(), class: "agents-notices",
                    div {
                        class: if notice.trouble { "notice trouble" } else { "notice" },
                        {notice.text.clone()}
                    }
                }
            }
            h2 { class: "settings-subheading", "Connect an app" }
            div { class: "agent-apps", {cards} }
            p { class: "settings-hint",
                "Connecting adds one Postrinch entry to the app\u{2019}s own MCP settings and keeps a backup of the file; nothing else in it changes. Restart the app afterwards."
            }
            h2 { class: "settings-subheading", "Connected" }
            div { class: "agent-list",
                for agent in state.agent_list.get() {
                    div { key: agent.id.clone(),
                        {agent_row(__scope, state, agent.clone())}
                    }
                }
                if state.agent_list.get().is_empty() {
                    p { class: "agent-empty", "No agents yet. Connect an app above." }
                }
            }
            p { class: "settings-hint",
                "Removing an agent stops its key at once, even mid-conversation. One connected from here also comes out of the app\u{2019}s settings."
            }
            div { class: "agents-lock",
                span { class: "agents-lock-title", "Always yours to approve" }
                span {
                    "An agent can read, search, mark, star, archive, trash and label: each can be undone. Sending and deleting open here for you to confirm, and no setting lets an agent skip that."
                }
            }
        }
    }
}

/// One app to connect; `None` for any other app, set up by hand.
#[component]
fn app_card(state: State, app: Option<App>) -> NodeHandle {
    let name = app.map_or("Another app", App::name);
    let found = app.is_some_and(App::found);
    let connected = move || {
        app.is_some_and(|app| {
            state
                .agent_list
                .get()
                .iter()
                .any(|a| a.setup.as_ref().is_some_and(|s| s.app == app))
        })
    };
    let open = move || {
        state.notice.set(None);
        // An app that is not here gets the entry to paste.
        let app = app.filter(|a| a.found());
        state
            .agent_dialog
            .set(Some(AgentDialog::Connect { app, editing: None }));
    };
    rsx! {
        div { class: "agent-app",
            div { class: "agent-app-text",
                span { class: "agent-app-name", {name} }
                span { class: {move || if connected() { "agent-app-state ok" } else { "agent-app-state" }},
                    {move || match app {
                        None => "Any app that speaks MCP",
                        Some(_) if connected() => "Connected",
                        Some(_) if found => "Found on this computer",
                        Some(_) => "Not found on this computer",
                    }}
                }
            }
            if found && !connected() {
                Button { variant: "filled", size: "xs", onclick: open, "Connect" }
            }
            if found && connected() {
                Button { variant: "default", size: "xs", onclick: open, "Connect another" }
            }
            if !found {
                Button { variant: "default", size: "xs", onclick: open, "Set up by hand" }
            }
        }
    }
}

#[component]
fn agent_row(state: State, agent: Agent) -> NodeHandle {
    let how = match &agent.setup {
        Some(setup) => format!("In {}\u{2019}s settings", setup.app.name()),
        None => "Set up by hand".to_owned(),
    };
    let used = match agent.last_used {
        Some(t) => format!("last used {}", dates::short(t, &dates::now())),
        None => "never used".to_owned(),
    };
    let detail = format!("{how} \u{b7} {used}");
    let reach = reach_line(&agent.reach);
    let (change, remove) = (agent.clone(), agent.clone());
    rsx! {
        div { class: "agent-row",
            div { class: "agent-row-text",
                span { class: "agent-row-name", {agent.name.clone()} }
                span { class: "agent-row-detail", {detail} }
            }
            div { class: "agent-row-reach",
                span { class: "agent-row-label", "Sees" }
                span { class: "agent-row-sees", {reach} }
            }
            Button {
                variant: "default",
                size: "xs",
                onclick: move || {
                    state.notice.set(None);
                    state.agent_dialog.set(Some(AgentDialog::Connect { app: None, editing: Some(change.clone()) }));
                },
                "Change"
            }
            Button {
                variant: "outline",
                color: "red",
                size: "xs",
                onclick: move || state.agent_removing.set(Some(remove.clone())),
                "Remove"
            }
        }
    }
}

pub const CSS: &str = r#"
.agents-head { display: flex; flex-direction: column; gap: 8px; }
.agents-notices { max-width: 820px; display: flex; flex-direction: column; gap: 8px; }
.agents-intro { margin: 0; color: var(--ink-2); max-width: 640px; line-height: 1.5; }
.agents-status { display: flex; align-items: center; gap: 8px; font-size: 13px; color: var(--ink-2); }
.status-dot { width: 8px; height: 8px; border-radius: 50%; background: #2f8a55; flex-shrink: 0; }
.status-dot.off { background: #b45309; }
.agent-apps { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 12px; max-width: 820px; }
.agent-app {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 10px;
    padding: 14px;
    border: 1px solid var(--line);
    border-radius: 10px;
}
.agent-app-text { display: flex; flex-direction: column; gap: 2px; }
.agent-app-name { font-weight: 600; }
.agent-app-state { font-size: 12px; color: var(--muted); }
.agent-app-state.ok { color: #2f8a55; }
.agent-list { display: flex; flex-direction: column; max-width: 820px; border: 1px solid var(--line); border-radius: 10px; }
.agent-list > div + div { border-top: 1px solid var(--line-soft); }
.agent-empty { margin: 0; padding: 14px 16px; color: var(--muted); }
.agent-row { display: flex; align-items: center; gap: 16px; padding: 12px 16px; }
.agent-row-text { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.agent-row-name { font-weight: 600; }
.agent-row-detail { font-size: 13px; color: var(--muted); }
.agent-row-reach { width: 250px; display: flex; flex-direction: column; gap: 2px; }
.agent-row-label { font-size: 12px; color: var(--muted); }
.agent-row-sees { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.agents-lock {
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-width: 820px;
    padding: 14px 16px;
    border-radius: 10px;
    background: var(--panel);
    line-height: 1.5;
    color: var(--ink-2);
}
.agents-lock-title { font-weight: 600; color: var(--ink); }
.agent-dialog { display: flex; flex-direction: column; gap: 16px; }
.agent-dialog .choices { max-width: none; }
.agent-accounts { display: flex; flex-direction: column; gap: 8px; padding: 4px 0 0 2px; }
.agent-field { display: flex; flex-direction: column; gap: 6px; }
.agent-field-name { font-weight: 600; }
.agent-note { margin: 0; padding: 12px 14px; border-radius: 10px; background: var(--panel); font-size: 13px; line-height: 1.5; color: var(--ink-2); }
.agent-warn { margin: 0; padding: 12px 14px; border-radius: 10px; background: var(--warn-bg); font-size: 13px; line-height: 1.5; color: var(--trouble); }
.agent-key { display: flex; gap: 8px; align-items: center; }
.agent-key code {
    flex: 1;
    min-width: 0;
    padding: 9px 12px;
    border: 1px solid var(--field);
    border-radius: 8px;
    background: var(--panel);
    font-family: 'IBM Plex Mono', monospace;
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}
.agent-key-head { display: flex; align-items: baseline; justify-content: space-between; }
.agent-once { font-size: 13px; color: var(--trouble); }
.agent-tabs { display: flex; gap: 4px; border-bottom: 1px solid var(--line-soft); }
.agent-tab { padding: 8px 12px; margin-bottom: -1px; border-bottom: 2px solid transparent; color: var(--ink-2); cursor: pointer; }
.agent-tab.chosen { border-bottom-color: var(--accent); color: var(--accent-ink); font-weight: 600; }
.agent-snippet {
    margin: 0;
    padding: 12px 14px;
    border-radius: 8px;
    background: #16181d;
    color: #e8eaee;
    font-family: 'IBM Plex Mono', monospace;
    font-size: 12px;
    line-height: 1.6;
    white-space: pre;
    overflow-x: auto;
}
.agent-copy { display: flex; align-items: center; gap: 10px; font-size: 13px; color: var(--muted); }
"#;
