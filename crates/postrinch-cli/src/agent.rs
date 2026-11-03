//! `postrinch-cli agent`: make, list and remove the agents that may connect
//! through `postrinch-mcp`, as Settings › Agents does. Works while the
//! window is open: it touches only the agents file, never the store.

use postrinch_agents::setup::{bridge_path, entry_name, json_snippet};
use postrinch_agents::{Agents, Reach, wire};
use postrinch_model::{EmailAddress, Timestamp};

use crate::AgentCommand;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub(crate) fn run(command: AgentCommand) -> Result<()> {
    let path = Agents::default_path().ok_or("this system has no config directory")?;
    let mut agents = Agents::load(&path)?;
    match command {
        AgentCommand::Add { name, account } => {
            let reach = if account.is_empty() {
                Reach::Every
            } else {
                let accounts = account
                    .iter()
                    .map(|a| EmailAddress::parse(a).map_err(|_| format!("not an address: {a}")))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Reach::Only(accounts)
            };
            let (agent, key) = agents.create(&name, reach)?;
            let bridge = bridge_path();
            let entry = entry_name(&agent.name);
            println!(
                "Agent {:?} made. Its key is shown this once:\n\n  {key}\n",
                agent.name
            );
            println!("Add this to the app's MCP servers (Claude Desktop, Cursor, …):\n");
            println!("{}\n", json_snippet(&entry, &bridge, &key));
            println!("Or for Claude Code:\n");
            println!(
                "  claude mcp add {entry} --env {}={key} -- {}",
                wire::KEY_ENV,
                bridge.display()
            );
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
                println!(
                    "{}  ({reach}; made {}, {used})",
                    agent.name,
                    when(agent.created)
                );
            }
        }
        AgentCommand::Remove { name } => match agents.remove(&name)? {
            Some(agent) => {
                println!("Removed {:?}: its key no longer works.", agent.name);
                match &agent.setup {
                    Some(setup) => match setup.app.remove(&setup.entry) {
                        Ok(_) => println!("Its entry is out of {}'s settings.", setup.app.name()),
                        Err(e) => {
                            println!("Its entry in {}'s settings stays: {e}", setup.app.name())
                        }
                    },
                    None => println!("Remove its entry from the app's MCP servers too."),
                }
            }
            None => return Err(format!("there is no agent {name:?}").into()),
        },
    }
    Ok(())
}

fn when(t: Timestamp) -> String {
    jiff::Timestamp::from_millisecond(t.as_millis())
        .map(|t| t.strftime("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_default()
}
