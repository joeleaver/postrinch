//! Agents: the app listens on loopback for `postrinch-mcp` (see
//! `postrinch-agents`), and Settings › Agents makes, changes and removes
//! them. The agents file is small and local, so it is read and written
//! here, on the UI thread.

use std::sync::Arc;

use postrinch_agents::connect::{self, Outcome};
use postrinch_agents::setup::{bridge_path, command_snippet, json_snippet, toml_snippet};
use postrinch_agents::{Agent, Agents, App, Changed, Gmail, Reach, serve, wire};
use postrinch_commands::Commands;
use postrinch_model::EmailAddress;

use crate::actions::Notice;
use crate::state::State;
use crate::sync::SyncHandle;

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
/// and wakes sync's sender, as the window's own changes do; a change to
/// rules also wakes sync, to give Gmail the rules, and the rules runner;
/// a change to views only reloads the window.
/// With `sync`, agents can also make, rename and delete labels in Gmail.
pub fn start(commands: Commands, state: State, sync: Option<SyncHandle>) {
    let Some(path) = Agents::default_path() else {
        state.agents_off.set(Some(
            "Agents cannot connect: this system has no settings folder.".into(),
        ));
        return;
    };
    let gmail = sync
        .clone()
        .map(|sync| Arc::new(crate::gmail_labels::GmailLabels(sync)) as Arc<dyn Gmail>);
    let wakes = sync.map(|s| (s.send, s.wake));
    let on_change: serve::OnChange = Arc::new(move |changed: &Changed| {
        // Views stay here: nothing for sync to send.
        if let Some((send, wake)) = &wakes
            && !matches!(
                changed,
                Changed::Views | Changed::Answers | Changed::Calendar
            )
        {
            send.notify_one();
            if matches!(changed, Changed::Rules(_)) {
                wake.notify_one();
            }
        }
        if let Changed::Rules(account) = changed {
            crate::rules::due_now(account.clone());
        }
        state.revision.update_send(|n| *n += 1);
    });
    let addr = wire::addr();
    let hooks = serve::Hooks { on_change, gmail };
    if let Err(e) = serve::start(&addr, commands, path, hooks) {
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

    /// The name the Connect dialog starts with: the app's, unless an agent
    /// connecting it would not replace has that name already.
    pub fn agent_name_for(self, app: Option<App>) -> String {
        let base = app.map_or("My agent", App::name);
        open().map_or_else(
            |_| base.to_owned(),
            |agents| {
                let replaced: Vec<String> = app
                    .map(|app| connect::would_replace(&agents, app))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|a| a.id)
                    .collect();
                agents.free_name_ignoring(base, &replaced)
            },
        )
    }

    /// The names of the agents connecting `app` would replace.
    pub fn agents_replaced_by(self, app: App) -> Vec<String> {
        open().map_or_else(
            |_| Vec::new(),
            |agents| {
                connect::would_replace(&agents, app)
                    .into_iter()
                    .map(|a| a.name)
                    .collect()
            },
        )
    }

    /// Make an agent; connect its app, or show its key to paste.
    pub fn make_agent(self, app: Option<App>, name: &str, reach: Reach) {
        let mut agents = match open() {
            Ok(a) => a,
            Err(e) => return self.agent_trouble(e),
        };
        let reach_text = reach_line(&reach);
        let bridge = bridge_path();
        let made = match connect::connect(&mut agents, app, name, reach, &bridge) {
            Ok(made) => made,
            Err(e) => return self.agent_trouble(e.to_string()),
        };
        let trouble = match made.outcome {
            Outcome::ByHand => None,
            Outcome::Added { replaced, .. } => {
                let app = app.expect("only an app is added to");
                self.agent_dialog.set(None);
                self.notice.set(Some(Notice {
                    text: added_text(app, &replaced),
                    trouble: false,
                }));
                self.load_agents();
                return;
            }
            Outcome::Trouble { error, replaced } => Some(format!(
                "{} could not be set up by itself: {error}. Paste this instead.{}",
                app.map_or("The app", App::name),
                replaced_text(&replaced),
            )),
        };
        let entry = made.entry;
        self.agent_dialog.set(Some(AgentDialog::Ready(Ready {
            name: made.agent.name.clone(),
            reach: reach_text,
            json: json_snippet(&entry, &bridge, &made.key),
            toml: toml_snippet(&entry, &bridge, &made.key),
            command: command_snippet(&bridge, &made.key),
            key: made.key,
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

    /// Remove an agent, and its entries from every app's settings.
    pub fn remove_agent(self, agent: &Agent) {
        let mut agents = match open() {
            Ok(a) => a,
            Err(e) => return self.agent_trouble(e),
        };
        if let Err(e) = agents.remove(&agent.id) {
            return self.agent_trouble(e.to_string());
        }
        let done = connect::disconnect(agent);
        let mut text = match (&agent.setup, done.files.is_empty()) {
            (_, false) => format!(
                "Removed {}, and its entry from {}.",
                agent.name,
                files_line(&done.files)
            ),
            (Some(setup), true) => format!(
                "Removed {}. {}\u{2019}s settings no longer held its entry.",
                agent.name,
                setup.app.name()
            ),
            (None, true) => format!(
                "Removed {}. Take its entry out of the app\u{2019}s settings too.",
                agent.name
            ),
        };
        for e in &done.trouble {
            text.push_str(&format!(" It may still be in {e}."));
        }
        self.notice.set(Some(Notice {
            text,
            trouble: !done.trouble.is_empty(),
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

/// What the notice says once `app` is set up.
fn added_text(app: App, replaced: &[Agent]) -> String {
    let what = match app {
        App::Claude => "Added to Claude, for its Chat and Code tabs and the terminal",
        App::Codex => "Added to Codex",
        App::Cursor => "Added to Cursor",
    };
    format!(
        "{what}. Restart {} to connect it.{}",
        app.name(),
        replaced_text(replaced)
    )
}

/// " It replaces A and B: …", or nothing when nothing was replaced.
pub fn replaced_text(replaced: &[Agent]) -> String {
    let names: Vec<String> = replaced.iter().map(|a| a.name.clone()).collect();
    match names.len() {
        0 => String::new(),
        1 => format!(
            " It replaces {}: that key no longer works, and rules that asked it ask this one.",
            names[0]
        ),
        _ => format!(
            " It replaces {}: their keys no longer work, and rules that asked them ask this one.",
            and_list(&names)
        ),
    }
}

/// "A", "A and B", "A, B and C".
pub fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The files, with the home folder as `~`.
pub fn files_line(files: &[std::path::PathBuf]) -> String {
    and_list(&files.iter().map(|f| tilde(f)).collect::<Vec<_>>())
}

/// A path, with the home folder as `~`.
pub fn tilde(path: &std::path::Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(ToOwned::to_owned)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

fn open() -> Result<Agents, String> {
    let path = Agents::default_path().ok_or("This system has no settings folder.")?;
    Agents::load(&path).map_err(|e| e.to_string())
}
