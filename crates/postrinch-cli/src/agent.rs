//! `postrinch-cli agent`: make, list and remove the agents that may connect
//! through `postrinch-mcp`, as Settings › Agents does. Works while the
//! window is open: it touches only the agents file and the apps' settings,
//! never the store.

use std::path::PathBuf;

use postrinch_agents::connect::{self, Changes, Outcome};
use postrinch_agents::setup::{bridge_path, json_snippet};
use postrinch_agents::{Agent, Agents, App, Reach};
use postrinch_model::{EmailAddress, Timestamp};

use crate::{AgentApp, AgentCommand};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub(crate) fn run(command: AgentCommand) -> Result<()> {
    let path = Agents::default_path().ok_or("this system has no config directory")?;
    let mut agents = Agents::load(&path)?;
    match command {
        AgentCommand::Add { name, account, app } => {
            let reach = if account.is_empty() {
                Reach::Every
            } else {
                let accounts = account
                    .iter()
                    .map(|a| EmailAddress::parse(a).map_err(|_| format!("not an address: {a}")))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Reach::Only(accounts)
            };
            let app = app.map(|a| match a {
                AgentApp::Claude => App::Claude,
                AgentApp::Codex => App::Codex,
                AgentApp::Cursor => App::Cursor,
            });
            let bridge = bridge_path();
            let made = connect::connect(&mut agents, app, &name, reach, &bridge)?;
            let snippet = json_snippet(&made.entry, &bridge, &made.key);
            match made.outcome {
                Outcome::Added { files, changes } => {
                    let app = app.expect("only an app is added to");
                    println!("Agent {:?} made, and added to:", made.agent.name);
                    for file in files {
                        println!("  {}", file.display());
                    }
                    say_changes(&changes);
                    println!("Restart {} to connect it.", app.name());
                }
                Outcome::Trouble { error, changes } => {
                    let app = app.map_or("The app", App::name);
                    println!(
                        "Agent {:?} made, but {app} could not be set up by itself: {error}",
                        made.agent.name
                    );
                    say_changes(&changes);
                    if !changes.replaced.is_empty() {
                        println!(
                            "Put the entry below in place of Postrinch's old entry in that file."
                        );
                    }
                    print_key(&made.key, &snippet);
                }
                Outcome::ByHand => {
                    println!("Agent {:?} made.", made.agent.name);
                    print_key(&made.key, &snippet);
                    println!(
                        "For Claude, Codex or Cursor, `postrinch-cli agent add <name> --app claude` \
                         (or codex, cursor) sets the app up instead."
                    );
                }
            }
        }
        AgentCommand::List => {
            if agents.list().is_empty() {
                println!("No agents. `postrinch-cli agent add <name>` makes one.");
            }
            for agent in agents.list() {
                let reach = match &agent.reach {
                    Reach::Every => "every account".to_owned(),
                    Reach::Only(accounts) => accounts
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                };
                let used = agent
                    .last_used
                    .map_or("never used".into(), |t| format!("last used {}", when(t)));
                let app = agent
                    .setup
                    .as_ref()
                    .map_or("set up by hand".to_owned(), |s| {
                        format!("in {}", s.app.name())
                    });
                println!(
                    "{}  ({app}; {reach}; made {}, {used})",
                    agent.name,
                    when(agent.created)
                );
            }
        }
        AgentCommand::Remove { name } => match agents.remove(&name)? {
            Some(agent) => {
                println!("Removed {:?}: its key no longer works.", agent.name);
                let done = connect::disconnect(&agent);
                say_files(&agent, &done.files);
                for e in done.trouble {
                    println!("It may still be in {e}");
                }
            }
            None => return Err(format!("there is no agent {name:?}").into()),
        },
    }
    Ok(())
}

fn print_key(key: &str, snippet: &str) {
    println!("Its key is shown this once:\n\n  {key}\n");
    println!("Add this to the app's MCP servers:\n");
    println!("{snippet}\n");
}

fn say_changes(changes: &Changes) {
    let names = |agents: &[Agent]| {
        agents
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !changes.replaced.is_empty() {
        println!(
            "It replaces {}: their keys no longer work, and rules that asked them ask this one \
             (once Postrinch next runs rules).",
            names(&changes.replaced)
        );
    }
    if !changes.left.is_empty() {
        println!(
            "Claude's settings no longer hold an entry for {}; {} where set up.",
            names(&changes.left),
            if changes.left.len() == 1 {
                "it still works"
            } else {
                "they still work"
            }
        );
    }
}

fn say_files(agent: &Agent, files: &[PathBuf]) {
    if !files.is_empty() {
        println!("Its entry came out of:");
        for file in files {
            println!("  {}", file.display());
        }
    } else if let Some(setup) = &agent.setup {
        println!("{}'s settings no longer held its entry.", setup.app.name());
    } else {
        println!("Remove its entry from the app's MCP servers too.");
    }
}

fn when(t: Timestamp) -> String {
    jiff::Timestamp::from_millisecond(t.as_millis())
        .map(|t| t.strftime("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_default()
}
