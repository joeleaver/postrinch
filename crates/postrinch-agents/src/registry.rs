//! The agents file: who may connect and which accounts each sees.
//!
//! `<config>/postrinch/agents.json`, on this computer only, written whole
//! (to a temporary file, then renamed) and readable by its owner alone.
//! A key is `postrinch_agent_<id>_<secret>`; the file keeps only its
//! SHA-256, so a key is shown once, when the agent is made, and never again.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use postrinch_model::{EmailAddress, Timestamp};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const KEY_PREFIX: &str = "postrinch_agent_";
/// A last use is written at most this often.
const TOUCH_EVERY_MS: i64 = 60_000;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("the agents file {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("the agents file {path} does not read: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("there is already an agent named {0:?}")]
    NameTaken(String),
    #[error("an agent needs a name")]
    NoName,
}

/// Which accounts an agent sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reach {
    /// Every account, those added later included.
    Every,
    Only(Vec<EmailAddress>),
}

/// An agent, as the file keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub reach: Reach,
    pub created: Timestamp,
    pub last_used: Option<Timestamp>,
    key_sha256: String,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    agents: Vec<Agent>,
}

/// The agents, as last read from their file.
pub struct Agents {
    path: PathBuf,
    agents: Vec<Agent>,
}

impl Agents {
    /// Where the agents file lives.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("postrinch").join("agents.json"))
    }

    /// Read the agents file; none yet is no agents.
    pub fn load(path: &Path) -> Result<Self, RegistryError> {
        let agents = match std::fs::read(path) {
            Ok(bytes) => {
                let file: File =
                    serde_json::from_slice(&bytes).map_err(|source| RegistryError::Parse {
                        path: path.to_owned(),
                        source,
                    })?;
                file.agents
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(source) => {
                return Err(RegistryError::Io {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        Ok(Self {
            path: path.to_owned(),
            agents,
        })
    }

    pub fn list(&self) -> &[Agent] {
        &self.agents
    }

    /// Make an agent. Returns it and its key, which is not kept: show it
    /// to the person now.
    pub fn create(&mut self, name: &str, reach: Reach) -> Result<(Agent, String), RegistryError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(RegistryError::NoName);
        }
        if self
            .agents
            .iter()
            .any(|a| a.name.eq_ignore_ascii_case(name))
        {
            return Err(RegistryError::NameTaken(name.to_owned()));
        }
        let id = hex(&random::<16>());
        let key = format!(
            "{KEY_PREFIX}{id}_{}",
            URL_SAFE_NO_PAD.encode(random::<32>())
        );
        let agent = Agent {
            id,
            name: name.to_owned(),
            reach,
            created: now(),
            last_used: None,
            key_sha256: hex(&Sha256::digest(key.as_bytes())),
        };
        self.agents.push(agent.clone());
        self.save()?;
        Ok((agent, key))
    }

    /// Remove an agent by id or name. Its key stops working at once.
    pub fn remove(&mut self, id_or_name: &str) -> Result<Option<Agent>, RegistryError> {
        let Some(at) = self
            .agents
            .iter()
            .position(|a| a.id == id_or_name || a.name.eq_ignore_ascii_case(id_or_name))
        else {
            return Ok(None);
        };
        let removed = self.agents.remove(at);
        self.save()?;
        Ok(Some(removed))
    }

    /// The agent a key belongs to.
    pub fn find_by_key(&self, key: &str) -> Option<&Agent> {
        let rest = key.strip_prefix(KEY_PREFIX)?;
        let (id, _) = rest.split_once('_')?;
        let agent = self.agents.iter().find(|a| a.id == id)?;
        let hash = hex(&Sha256::digest(key.as_bytes()));
        same(hash.as_bytes(), agent.key_sha256.as_bytes()).then_some(agent)
    }

    /// Note that an agent was used now, at most once a minute.
    pub fn touch(&mut self, id: &str) -> Result<(), RegistryError> {
        let now = now();
        let Some(agent) = self.agents.iter_mut().find(|a| a.id == id) else {
            return Ok(());
        };
        if agent
            .last_used
            .is_some_and(|t| now.as_millis() - t.as_millis() < TOUCH_EVERY_MS)
        {
            return Ok(());
        }
        agent.last_used = Some(now);
        self.save()
    }

    fn save(&self) -> Result<(), RegistryError> {
        let io = |source| RegistryError::Io {
            path: self.path.clone(),
            source,
        };
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let file = File {
            agents: self.agents.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&file).expect("agents serialize");
        let tmp = self.path.with_extension("json.tmp");
        let mut out = owner_only(&tmp).map_err(io)?;
        out.write_all(&bytes).map_err(io)?;
        out.sync_all().map_err(io)?;
        drop(out);
        std::fs::rename(&tmp, &self.path).map_err(io)
    }
}

/// A new file only its owner can read.
fn owner_only(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Compare without stopping at the first difference.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    rand::fill(&mut bytes[..]);
    bytes
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn now() -> Timestamp {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    Timestamp::from_millis(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_finds_its_agent_until_it_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agents.json");
        let mut agents = Agents::load(&path).unwrap();
        let work = EmailAddress::parse("work@example.org").unwrap();
        let (agent, key) = agents
            .create("Claude Desktop", Reach::Only(vec![work]))
            .unwrap();
        assert!(key.starts_with("postrinch_agent_"));
        assert!(matches!(
            agents.create("claude desktop", Reach::Every),
            Err(RegistryError::NameTaken(_))
        ));

        let again = Agents::load(&path).unwrap();
        assert_eq!(again.find_by_key(&key).map(|a| &a.id), Some(&agent.id));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains(&key[key.len() - 20..]),
            "the key is not kept"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        let mut wrong = key.clone();
        wrong.pop();
        wrong.push(if key.ends_with('A') { 'B' } else { 'A' });
        assert!(again.find_by_key(&wrong).is_none());
        assert!(again.find_by_key("postrinch_agent_nope_x").is_none());

        agents.remove("Claude Desktop").unwrap().unwrap();
        assert!(Agents::load(&path).unwrap().find_by_key(&key).is_none());
    }
}
