//! `postrinch-cli agent`: make, list and remove the agents that may connect
//! through `postrinch-mcp`. Until Settings › Agents exists, this is how
//! the person sets one up. Works while the window is open: it touches only
//! the agents file, never the store.

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
            println!(
                "Agent {:?} made. Its key is shown this once:\n\n  {key}\n",
                agent.name
            );
            println!("Add this to the app's MCP servers (Claude Desktop, Cursor, …):\n");
            let entry = serde_json::json!({
                format!("postrinch-{}", slug(&agent.name)): {
                    "command": bridge,
                    "env": { wire::KEY_ENV: key },
                }
            });
            println!("{}\n", serde_json::to_string_pretty(&entry)?);
            println!("Or for Claude Code:\n");
            println!(
                "  claude mcp add postrinch-{} --env {}={key} -- {bridge}",
                slug(&agent.name),
                wire::KEY_ENV,
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
            Some(agent) => println!(
                "Removed {:?}: its key no longer works. Remove its entry from the app's MCP servers too.",
                agent.name
            ),
            None => return Err(format!("there is no agent {name:?}").into()),
        },
    }
    Ok(())
}

/// `postrinch-mcp`, beside this program.
fn bridge_path() -> String {
    let name = if cfg!(windows) {
        "postrinch-mcp.exe"
    } else {
        "postrinch-mcp"
    };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .map_or_else(|| name.to_owned(), |p| p.display().to_string())
}

fn slug(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    slug.split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn when(t: Timestamp) -> String {
    jiff::Timestamp::from_millisecond(t.as_millis())
        .map(|t| t.strftime("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_default()
}
