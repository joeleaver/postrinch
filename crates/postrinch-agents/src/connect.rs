//! Connecting an app to a new agent, and taking an agent's entries out
//! again: what Settings › Agents and `postrinch-cli agent` both do.
//!
//! Connecting Claude replaces whatever Postrinch entries its files hold
//! already among the person's own servers (any launching `postrinch-mcp`,
//! under any name), so each file ends with exactly one. A project's own
//! servers in `~/.claude.json` are left: one there was put there on purpose. The agents those entries served, and every
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
use crate::setup_file::{Server, SettingsFile, put_back};

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
    /// The entry went into these files.
    Added {
        files: Vec<PathBuf>,
        changes: Changes,
    },
    /// The app could not be set up by itself: the person pastes the entry.
    /// Nothing was replaced, unless one of the app's files was written and
    /// another could not be.
    Trouble { error: SetupError, changes: Changes },
}

/// What connecting did to other agents.
#[derive(Debug, Default)]
pub struct Changes {
    /// The agents the new one replaced: their keys no longer work.
    pub replaced: Vec<Agent>,
    /// Agents of another app whose entry came out of Claude's settings:
    /// they still work where they are set up.
    pub left: Vec<Agent>,
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
    let everywhere: Vec<Place> = App::ALL
        .into_iter()
        .filter_map(|a| a.places().ok())
        .flatten()
        .collect();
    connect_in(agents, app.zip(places), &everywhere, name, reach, bridge)
}

/// [`connect`], with the app's files and every app's files given.
pub(crate) fn connect_in(
    agents: &mut Agents,
    app: Option<(App, Result<Vec<Place>, SetupError>)>,
    everywhere: &[Place],
    name: &str,
    reach: Reach,
    bridge: &Path,
) -> Result<Connected, RegistryError> {
    let Some((app, places)) = app else {
        return by_hand(agents, name, reach, Outcome::ByHand);
    };
    let Prepared {
        files,
        retire,
        dropped,
    } = match places.and_then(|p| prepare(agents, app, &p)) {
        Ok(prepared) => prepared,
        Err(error) => {
            let outcome = Outcome::Trouble {
                error,
                changes: Changes::default(),
            };
            return by_hand(agents, &agents.free_name(name), reach, outcome);
        }
    };
    let pending = agents.mint(name, reach.clone(), &retire)?;
    let key = pending.key.clone();
    let entry = entry_name(&pending.agent.name);
    // The files first: until one holds the new key, no key is revoked.
    let mut written = Vec::new();
    let mut trouble = None;
    for mut file in files {
        file.insert(&entry, bridge, &key);
        let path = file.path().to_owned();
        let original = file.original().map(str::to_owned);
        match file.write() {
            Ok(_) => written.push((path, original)),
            Err(e) => trouble = trouble.or(Some(e)),
        }
    }
    if written.is_empty() {
        // Nothing changed: the agents Claude has keep working.
        let error = trouble.unwrap_or(SetupError::NotHere(app.name()));
        let outcome = Outcome::Trouble {
            error,
            changes: Changes::default(),
        };
        return by_hand(agents, &agents.free_name(name), reach, outcome);
    }
    let setup = Setup {
        app,
        entry: entry.clone(),
    };
    let (agent, replaced) = match agents.commit(pending, Some(setup)) {
        Ok(done) => done,
        Err(e) => {
            // The new key is not saved: the files go back to the old ones.
            for (path, original) in &written {
                if let Err(e) = put_back(path, original.as_deref()) {
                    tracing::warn!("putting {} back: {e}", path.display());
                }
            }
            return Err(e);
        }
    };
    let written: Vec<PathBuf> = written.into_iter().map(|(path, _)| path).collect();
    // A replaced agent's key may also have been pasted elsewhere.
    for old in &replaced {
        for e in disconnect_in(old, everywhere).trouble {
            tracing::warn!("{} replaced, but its entry may stay: {e}", old.name);
        }
    }
    let left = agents
        .list()
        .iter()
        .filter(|a| a.id != agent.id && dropped.iter().any(|k| a.holds_key(k)))
        .cloned()
        .collect();
    let changes = Changes { replaced, left };
    let outcome = match trouble {
        Some(error) => Outcome::Trouble { error, changes },
        None => Outcome::Added {
            files: written,
            changes,
        },
    };
    Ok(Connected {
        agent,
        key,
        entry,
        outcome,
    })
}

/// An agent made with no app set up: its entry to paste.
fn by_hand(
    agents: &mut Agents,
    name: &str,
    reach: Reach,
    outcome: Outcome,
) -> Result<Connected, RegistryError> {
    let (agent, key) = agents.create(name, reach)?;
    let entry = entry_name(&agent.name);
    Ok(Connected {
        agent,
        key,
        entry,
        outcome,
    })
}

/// An app's files ready to take the new entry.
struct Prepared {
    files: Vec<SettingsFile>,
    /// The agents to replace, by id.
    retire: Vec<String>,
    /// The keys of the entries taken out.
    dropped: Vec<String>,
}

/// The app's files read, Postrinch's entries taken out of Claude's (the
/// person's own: a project's stays, as its own choice), the agents to
/// replace, and the keys taken out.
fn prepare(agents: &Agents, app: App, places: &[Place]) -> Result<Prepared, SetupError> {
    if places.is_empty() {
        return Err(SetupError::NotHere(app.name()));
    }
    let mut files = places
        .iter()
        .map(SettingsFile::read)
        .collect::<Result<Vec<_>, _>>()?;
    if app != App::Claude {
        return Ok(Prepared {
            files,
            retire: Vec::new(),
            dropped: Vec::new(),
        });
    }
    let dropped: Vec<String> = files
        .iter_mut()
        .flat_map(|f| {
            f.drop_servers(|s| s.project.is_none() && s.command.as_deref().is_some_and(is_bridge))
        })
        .filter_map(|s| s.key)
        .collect();
    let retire = replaced_ids(agents, &dropped);
    Ok(Prepared {
        files,
        retire,
        dropped,
    })
}

/// The agents connecting Claude replaces: every one set up for Claude, and
/// those set up by hand whose key was in an entry taken out.
fn replaced_ids(agents: &Agents, dropped: &[String]) -> Vec<String> {
    agents
        .list()
        .iter()
        .filter(|a| match &a.setup {
            Some(setup) => setup.app == App::Claude,
            None => dropped.iter().any(|k| a.holds_key(k)),
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
    let ids = prepare(agents, app, places).map_or_else(|_| Vec::new(), |p| p.retire);
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

#[cfg(all(test, unix))]
#[path = "connect_failure_tests.rs"]
mod failure_tests;
