//! Connecting an app to a new agent, and taking an agent's entries out
//! again: what Settings › Agents and `postrinch-cli agent` both do.
//!
//! Connecting Claude replaces whatever Postrinch entries its files hold
//! already (any server launching `postrinch-mcp`, under any name), so each
//! file ends with exactly one. The agents those entries served, and every
//! other agent set up for Claude, are replaced by the new one in the same
//! save: their keys stop working, and rules that asked them ask it.
//! Codex and Cursor each get one more entry, as many agents as the person
//! makes.
//!
//! An agent's entries are found by its key, not by their names, so removing
//! one takes out every copy of it wherever it was put.

use std::path::{Path, PathBuf};

use crate::registry::{Agent, Agents, Reach, RegistryError, Setup};
use crate::setup::{App, Kind, Place, SetupError, entry_name, is_bridge};
use crate::setup_file::{Server, SettingsFile};

/// A new agent, and what became of its app's settings.
#[derive(Debug)]
pub struct Connected {
    pub agent: Agent,
    /// Its key, shown this once: it is not kept.
    pub key: String,
    /// Its entry's name in the app's settings, or to paste.
    pub entry: String,
    pub outcome: Outcome,
}

#[derive(Debug)]
pub enum Outcome {
    /// No app was named: the person pastes the entry.
    ByHand,
    /// The entry went into these files, and these agents were replaced.
    Added {
        files: Vec<PathBuf>,
        replaced: Vec<Agent>,
    },
    /// The app could not be set up by itself: the person pastes the entry.
    /// `replaced` is empty unless the trouble came after the new agent was
    /// made.
    Trouble {
        error: SetupError,
        replaced: Vec<Agent>,
    },
}

/// Make the agent `name` and set `app` up to launch it.
pub fn connect(
    agents: &mut Agents,
    app: Option<App>,
    name: &str,
    reach: Reach,
    bridge: &Path,
) -> Result<Connected, RegistryError> {
    let places = app.map(App::installed_places);
    connect_in(agents, app.zip(places), name, reach, bridge)
}

/// [`connect`], with the app's files given.
pub(crate) fn connect_in(
    agents: &mut Agents,
    app: Option<(App, Result<Vec<Place>, SetupError>)>,
    name: &str,
    reach: Reach,
    bridge: &Path,
) -> Result<Connected, RegistryError> {
    let Some((app, places)) = app else {
        let (agent, key) = agents.create(name, reach)?;
        let entry = entry_name(&agent.name);
        return Ok(Connected {
            agent,
            key,
            entry,
            outcome: Outcome::ByHand,
        });
    };
    let (mut files, retire) = match places.and_then(|p| prepare(agents, app, &p)) {
        Ok(prepared) => prepared,
        Err(error) => {
            // Nothing was replaced, so the name may still be taken.
            let (agent, key) = agents.create(&agents.free_name(name), reach)?;
            let entry = entry_name(&agent.name);
            return Ok(Connected {
                agent,
                key,
                entry,
                outcome: Outcome::Trouble {
                    error,
                    replaced: Vec::new(),
                },
            });
        }
    };
    let (agent, key, replaced) = agents.replace(name, reach, &retire)?;
    let entry = entry_name(&agent.name);
    let mut written = Vec::new();
    let mut trouble = None;
    for mut file in files.drain(..) {
        file.insert(&entry, bridge, &key);
        let path = file.path().to_owned();
        match file.write() {
            Ok(_) => written.push(path),
            Err(e) => trouble = trouble.or(Some(e)),
        }
    }
    if !written.is_empty() {
        let setup = Setup {
            app,
            entry: entry.clone(),
        };
        // The file says so too: removing finds the entries by their key.
        if let Err(e) = agents.set_setup(&agent.id, Some(setup)) {
            tracing::warn!("{e}");
        }
    }
    let agent = agents.get(&agent.id).cloned().unwrap_or(agent);
    let outcome = match trouble {
        Some(error) => Outcome::Trouble { error, replaced },
        None => Outcome::Added {
            files: written,
            replaced,
        },
    };
    Ok(Connected {
        agent,
        key,
        entry,
        outcome,
    })
}

/// The app's files read, Postrinch's old entries taken out of Claude's,
/// and the agents to replace.
fn prepare(
    agents: &Agents,
    app: App,
    places: &[Place],
) -> Result<(Vec<SettingsFile>, Vec<String>), SetupError> {
    if places.is_empty() {
        return Err(SetupError::NotHere(app.name()));
    }
    let mut files = places
        .iter()
        .map(SettingsFile::read)
        .collect::<Result<Vec<_>, _>>()?;
    if app != App::Claude {
        return Ok((files, Vec::new()));
    }
    let dropped: Vec<Server> = files
        .iter_mut()
        .flat_map(|f| f.drop_servers(|s| s.command.as_deref().is_some_and(is_bridge)))
        .collect();
    Ok((files, replaced_ids(agents, &dropped)))
}

/// The agents connecting Claude replaces: every one set up for Claude, and
/// those set up by hand whose key was in an entry taken out.
fn replaced_ids(agents: &Agents, dropped: &[Server]) -> Vec<String> {
    agents
        .list()
        .iter()
        .filter(|a| match &a.setup {
            Some(setup) => setup.app == App::Claude,
            None => dropped
                .iter()
                .filter_map(|s| s.key.as_deref())
                .any(|k| a.holds_key(k)),
        })
        .map(|a| a.id.clone())
        .collect()
}

/// The agents connecting `app` now would replace, for the person to see
/// first. A file that does not read replaces nothing.
pub fn would_replace(agents: &Agents, app: App) -> Vec<Agent> {
    let Ok(places) = app.installed_places() else {
        return Vec::new();
    };
    would_replace_in(agents, app, &places)
}

pub(crate) fn would_replace_in(agents: &Agents, app: App, places: &[Place]) -> Vec<Agent> {
    let ids = prepare(agents, app, places).map_or_else(|_| Vec::new(), |(_, ids)| ids);
    agents
        .list()
        .iter()
        .filter(|a| ids.contains(&a.id))
        .cloned()
        .collect()
}

/// What taking an agent's entries out did.
#[derive(Debug, Default)]
pub struct Disconnected {
    /// The files its entries came out of.
    pub files: Vec<PathBuf>,
    /// Files that held them but could not be changed.
    pub trouble: Vec<SetupError>,
}

/// Take every entry holding `agent`'s key out of every app's settings: the
/// files Postrinch wrote it to, and any it was pasted into.
pub fn disconnect(agent: &Agent) -> Disconnected {
    let mut places = Vec::new();
    let mut done = Disconnected::default();
    for app in App::ALL {
        match app.places() {
            Ok(p) => places.extend(p),
            Err(e) => done.trouble.push(e),
        }
    }
    let more = disconnect_in(agent, &places);
    done.files.extend(more.files);
    done.trouble.extend(more.trouble);
    done
}

pub(crate) fn disconnect_in(agent: &Agent, places: &[Place]) -> Disconnected {
    let mut done = Disconnected::default();
    for place in places.iter().filter(|p| p.path.exists()) {
        let result = SettingsFile::read(place).and_then(|mut file| {
            file.drop_servers(|s| s.key.as_deref().is_some_and(|k| agent.holds_key(k)));
            file.write()
        });
        match result {
            Ok(true) => done.files.push(place.path.clone()),
            Ok(false) => {}
            // A file that does not read is left alone. It matters only if
            // it may hold the key, which begins with the agent's id.
            Err(SetupError::Unreadable(path)) => {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                if text.contains(&agent.id) {
                    done.trouble.push(SetupError::Unreadable(path));
                }
            }
            Err(e) => done.trouble.push(e),
        }
    }
    done
}

/// An agent a rule can ask: connected to Claude, with its entry in
/// `~/.claude.json`, where the terminal's `claude` can start it unattended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Askable {
    pub id: String,
    pub name: String,
    /// Its entry's name there, which names its tools (`mcp__<entry>__…`).
    pub entry: String,
}

/// The agents a rule can ask.
pub fn askable(agents: &Agents) -> Vec<Askable> {
    let place = App::Claude
        .places()
        .ok()
        .and_then(|places| places.into_iter().find(|p| p.kind == Kind::ClaudeCode));
    place.map_or_else(Vec::new, |p| askable_in(agents, &p))
}

pub(crate) fn askable_in(agents: &Agents, place: &Place) -> Vec<Askable> {
    let Ok(file) = SettingsFile::read(place) else {
        return Vec::new();
    };
    let servers: Vec<Server> = file
        .servers()
        .into_iter()
        .filter(|s| s.project.is_none())
        .collect();
    agents
        .list()
        .iter()
        .filter(|a| a.setup.as_ref().is_some_and(|s| s.app == App::Claude))
        .filter_map(|a| {
            let server = servers
                .iter()
                .find(|s| s.key.as_deref().is_some_and(|k| a.holds_key(k)))?;
            Some(Askable {
                id: a.id.clone(),
                name: a.name.clone(),
                entry: server.name.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "connect_tests.rs"]
mod tests;
