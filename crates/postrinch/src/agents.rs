//! Agents: the app listens on loopback for `postrinch-mcp` (see
//! `postrinch-agents`), and Settings › Agents makes, changes and removes
//! them. The agents file is small and local, so it is read and written
//! here, on the UI thread.

use std::sync::Arc;

use postrinch_agents::setup::{
    bridge_path, command_snippet, entry_name, json_snippet, toml_snippet,
};
use postrinch_agents::{Agent, Agents, App, Reach, Setup, serve, wire};
use postrinch_commands::Commands;
use postrinch_model::EmailAddress;
use tokio::sync::Notify;

use crate::actions::Notice;
use crate::state::State;

/// The dialog Settings › Agents has open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentDialog {
    /// Connect an app (`app` set), set one up by hand (`app` none), or
    /// change an agent's accounts (`editing` set).
    Connect {
        app: Option<App>,
        editing: Option<Agent>,
    },
    /// An agent made: its key, shown this once, and the entry to paste.
    Ready(Ready),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    pub name: String,
    pub reach: String,
    pub key: String,
    /// Why the app could not be set up by itself, when it was asked to be.
    pub trouble: Option<String>,
    pub json: String,
    pub toml: String,
    pub command: String,
}

/// Listen for agents. A change an agent makes reloads what the window shows
/// and wakes sync's sender, as the window's own changes do.
pub fn start(commands: Commands, state: State, send: Option<Arc<Notify>>) {
    let Some(path) = Agents::default_path() else {
        state.agents_off.set(Some(
            "Agents cannot connect: this system has no settings folder.".into(),
        ));
        return;
    };
    let on_change: serve::OnChange = Arc::new(move || {
        if let Some(send) = &send {
            send.notify_one();
        }
        state.revision.update_send(|n| *n += 1);
    });
    let addr = wire::addr();
    if let Err(e) = serve::start(&addr, commands, path, on_change) {
        // Another Postrinch is answering there, or something else holds it.
        tracing::warn!("agents cannot connect: listening at {addr} failed: {e}");
        let port = addr.rsplit(':').next().unwrap_or(&addr).to_owned();
        state.agents_off.set(Some(format!(
            "Agents cannot connect: another program is using Postrinch\u{2019}s port ({port}). Close it, or the other Postrinch, and reopen."
        )));
    }
}

/// How an agent's reach reads.
pub fn reach_line(reach: &Reach) -> String {
    match reach {
        Reach::Every => "Every account, and any you add".to_owned(),
        Reach::Only(accounts) if accounts.is_empty() => "No accounts".to_owned(),
        Reach::Only(accounts) => accounts
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", "),
    }
}

impl State {
    /// Read the agents file into Settings.
    pub fn load_agents(self) {
        match open() {
            Ok(agents) => self.agent_list.set(agents.list().to_vec()),
            Err(e) => self.agent_trouble(e),
        }
    }

    /// The name the Connect dialog starts with.
    pub fn agent_name_for(self, app: Option<App>) -> String {
        let base = app.map_or("My agent", App::name);
        open().map_or_else(|_| base.to_owned(), |a| a.free_name(base))
    }

    /// Make an agent; connect its app, or show its key to paste.
    pub fn make_agent(self, app: Option<App>, name: &str, reach: Reach) {
        let mut agents = match open() {
            Ok(a) => a,
            Err(e) => return self.agent_trouble(e),
        };
        let reach_text = reach_line(&reach);
        let (agent, key) = match agents.create(name, reach) {
            Ok(made) => made,
            Err(e) => return self.agent_trouble(e.to_string()),
        };
        let entry = entry_name(&agent.name);
        let bridge = bridge_path();
        let trouble = match app {
            None => None,
            Some(app) => match app.add(&entry, &bridge, &key) {
                Ok(_) => {
                    let setup = Setup {
                        app,
                        entry: entry.clone(),
                    };
                    if let Err(e) = agents.set_setup(&agent.id, Some(setup)) {
                        tracing::warn!("{e}");
                    }
                    self.agent_dialog.set(None);
                    self.notice.set(Some(Notice {
                        text: format!(
                            "Added to {}. Restart {} to connect it.",
                            app.name(),
                            app.name()
                        ),
                        trouble: false,
                    }));
                    self.load_agents();
                    return;
                }
                Err(e) => Some(format!(
                    "{} could not be set up by itself: {e}. Paste this instead.",
                    app.name()
                )),
            },
        };
        self.agent_dialog.set(Some(AgentDialog::Ready(Ready {
            name: agent.name.clone(),
            reach: reach_text,
            json: json_snippet(&entry, &bridge, &key),
            toml: toml_snippet(&entry, &bridge, &key),
            command: command_snippet(&bridge, &key),
            key,
            trouble,
        })));
        self.load_agents();
    }

    /// Change which accounts an agent sees.
    pub fn change_reach(self, agent: &Agent, reach: Reach) {
        let result =
            open().and_then(|mut a| a.set_reach(&agent.id, reach).map_err(|e| e.to_string()));
        if let Err(e) = result {
            return self.agent_trouble(e);
        }
        self.agent_dialog.set(None);
        self.load_agents();
    }

    /// Remove an agent, and the entry Postrinch put in its app's settings.
    pub fn remove_agent(self, agent: &Agent) {
        let mut agents = match open() {
            Ok(a) => a,
            Err(e) => return self.agent_trouble(e),
        };
        if let Err(e) = agents.remove(&agent.id) {
            return self.agent_trouble(e.to_string());
        }
        let text = match &agent.setup {
            Some(setup) => match setup.app.remove(&setup.entry) {
                Ok(_) => format!(
                    "Removed {}, and its entry from {}\u{2019}s settings.",
                    agent.name,
                    setup.app.name()
                ),
                Err(e) => format!(
                    "Removed {}. Its entry in {}\u{2019}s settings stays: {e}",
                    agent.name,
                    setup.app.name()
                ),
            },
            None => format!(
                "Removed {}. Take its entry out of the app\u{2019}s settings too.",
                agent.name
            ),
        };
        self.notice.set(Some(Notice {
            text,
            trouble: false,
        }));
        self.load_agents();
    }

    fn agent_trouble(self, text: String) {
        self.notice.set(Some(Notice {
            text,
            trouble: true,
        }));
    }
}

/// The reach a Connect dialog's choices make.
pub fn reach_of(every: bool, chosen: &[EmailAddress]) -> Reach {
    if every {
        Reach::Every
    } else {
        Reach::Only(chosen.to_vec())
    }
}

fn open() -> Result<Agents, String> {
    let path = Agents::default_path().ok_or("This system has no settings folder.")?;
    Agents::load(&path).map_err(|e| e.to_string())
}
