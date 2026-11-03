//! Setting an app up to launch `postrinch-mcp`: one entry in the app's own
//! MCP settings, and nothing else in its file changed.
//!
//! Before a file is changed it is copied beside itself
//! (`<file>.postrinch-backup`); the new one is written whole and renamed into
//! place, keeping the old one's permissions. A file that does not parse,
//! or whose servers are not where they should be, is left alone: the person
//! gets the entry to paste instead.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::wire::KEY_ENV;

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("this system has no home folder")]
    NoHome,
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{0} does not read as the app's settings, so it was left alone")]
    Unreadable(PathBuf),
}

/// An app Postrinch can set up by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum App {
    ClaudeDesktop,
    ClaudeCode,
    Codex,
    Cursor,
}

impl App {
    pub const ALL: [App; 4] = [App::ClaudeDesktop, App::ClaudeCode, App::Codex, App::Cursor];

    pub fn name(self) -> &'static str {
        match self {
            App::ClaudeDesktop => "Claude Desktop",
            App::ClaudeCode => "Claude Code",
            App::Codex => "Codex",
            App::Cursor => "Cursor",
        }
    }

    /// The file holding the app's MCP servers.
    pub fn settings_path(self) -> Result<PathBuf, SetupError> {
        let home = || dirs::home_dir().ok_or(SetupError::NoHome);
        Ok(match self {
            App::ClaudeDesktop => dirs::config_dir()
                .ok_or(SetupError::NoHome)?
                .join("Claude")
                .join("claude_desktop_config.json"),
            App::ClaudeCode => home()?.join(".claude.json"),
            App::Cursor => home()?.join(".cursor").join("mcp.json"),
            App::Codex => match std::env::var_os("CODEX_HOME") {
                Some(dir) => PathBuf::from(dir).join("config.toml"),
                None => home()?.join(".codex").join("config.toml"),
            },
        })
    }

    /// Whether the app looks installed: its settings file or its folder is
    /// there.
    pub fn found(self) -> bool {
        self.settings_path().is_ok_and(|p| {
            p.exists()
                || match self {
                    // ~/.claude.json sits in the home folder itself.
                    App::ClaudeCode => dirs::home_dir().is_some_and(|h| h.join(".claude").is_dir()),
                    _ => p.parent().is_some_and(Path::is_dir),
                }
        })
    }

    fn toml(self) -> bool {
        self == App::Codex
    }

    /// Add (or replace) the entry `entry` launching `bridge` with `key`.
    /// Returns the file changed.
    pub fn add(self, entry: &str, bridge: &Path, key: &str) -> Result<PathBuf, SetupError> {
        let path = self.settings_path()?;
        self.add_at(&path, entry, bridge, key)?;
        Ok(path)
    }

    fn add_at(self, path: &Path, entry: &str, bridge: &Path, key: &str) -> Result<(), SetupError> {
        let path = path.to_owned();
        let old = read(&path)?;
        let new = if self.toml() {
            toml_with(&path, old.as_deref(), entry, bridge, key)?
        } else {
            let mut server = json!({ "command": bridge, "env": { KEY_ENV: key } });
            if self == App::ClaudeCode {
                server["type"] = json!("stdio");
            }
            json_with(&path, old.as_deref(), |servers| {
                servers.insert(entry.to_owned(), server);
            })?
        };
        write(&path, old.is_some(), &new)
    }

    /// Take the entry out, if it is there. A file that is gone or does not
    /// read is left as it is.
    pub fn remove(self, entry: &str) -> Result<bool, SetupError> {
        self.remove_at(&self.settings_path()?, entry)
    }

    fn remove_at(self, path: &Path, entry: &str) -> Result<bool, SetupError> {
        let path = path.to_owned();
        let Some(old) = read(&path)? else {
            return Ok(false);
        };
        let new = if self.toml() {
            let mut doc: toml_edit::DocumentMut = old
                .parse()
                .map_err(|_| SetupError::Unreadable(path.clone()))?;
            let Some(servers) = doc
                .get_mut("mcp_servers")
                .and_then(|t| t.as_table_like_mut())
            else {
                return Ok(false);
            };
            if servers.remove(entry).is_none() {
                return Ok(false);
            }
            doc.to_string()
        } else {
            let mut removed = false;
            let new = json_with(&path, Some(&old), |servers| {
                removed = servers.shift_remove(entry).is_some();
            })?;
            if !removed {
                return Ok(false);
            }
            new
        };
        write(&path, true, &new)?;
        Ok(true)
    }
}

/// The entry's name: `postrinch-` and the agent's name in lower case.
pub fn entry_name(agent: &str) -> String {
    let slug: String = agent
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug: Vec<&str> = slug.split('-').filter(|s| !s.is_empty()).collect();
    if slug.is_empty() {
        "postrinch".to_owned()
    } else {
        format!("postrinch-{}", slug.join("-"))
    }
}

/// `postrinch-mcp`, beside the program running.
pub fn bridge_path() -> PathBuf {
    let name = if cfg!(windows) {
        "postrinch-mcp.exe"
    } else {
        "postrinch-mcp"
    };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

/// The entry to paste, for apps set up by hand.
pub fn json_snippet(entry: &str, bridge: &Path, key: &str) -> String {
    let value = json!({ "mcpServers": { entry: { "command": bridge, "env": { KEY_ENV: key } } } });
    serde_json::to_string_pretty(&value).expect("JSON serializes")
}

pub fn toml_snippet(entry: &str, bridge: &Path, key: &str) -> String {
    toml_with(Path::new(""), None, entry, bridge, key).expect("a new file always works")
}

pub fn command_snippet(bridge: &Path, key: &str) -> String {
    format!(
        "Command:      {}\nEnvironment:  {KEY_ENV}={key}",
        bridge.display()
    )
}

fn read(path: &Path) -> Result<Option<String>, SetupError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(SetupError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

/// The JSON file with its `mcpServers` changed by `change`.
fn json_with(
    path: &Path,
    old: Option<&str>,
    change: impl FnOnce(&mut Map<String, Value>),
) -> Result<String, SetupError> {
    let unreadable = || SetupError::Unreadable(path.to_owned());
    let mut doc: Value = match old {
        Some(text) if !text.trim().is_empty() => {
            serde_json::from_str(text).map_err(|_| unreadable())?
        }
        _ => json!({}),
    };
    let root = doc.as_object_mut().ok_or_else(unreadable)?;
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(unreadable)?;
    change(servers);
    let mut text = serde_json::to_string_pretty(&doc).expect("JSON serializes");
    text.push('\n');
    Ok(text)
}

/// The TOML file with `[mcp_servers.<entry>]` set, comments kept.
fn toml_with(
    path: &Path,
    old: Option<&str>,
    entry: &str,
    bridge: &Path,
    key: &str,
) -> Result<String, SetupError> {
    use toml_edit::{DocumentMut, Item, Table, value};
    let mut doc: DocumentMut = old
        .unwrap_or_default()
        .parse()
        .map_err(|_| SetupError::Unreadable(path.to_owned()))?;
    let servers = doc
        .entry("mcp_servers")
        .or_insert_with(|| {
            let mut t = Table::new();
            t.set_implicit(true);
            Item::Table(t)
        })
        .as_table_mut()
        .ok_or_else(|| SetupError::Unreadable(path.to_owned()))?;
    let mut server = Table::new();
    server["command"] = value(bridge.display().to_string());
    let mut env = Table::new();
    env[KEY_ENV] = value(key);
    server["env"] = Item::Table(env);
    servers[entry] = Item::Table(server);
    Ok(doc.to_string())
}

/// Write `text` to `path`: the old file backed up first, the new one
/// renamed into place with the old one's permissions.
fn write(path: &Path, existed: bool, text: &str) -> Result<(), SetupError> {
    let io = |source| SetupError::Io {
        path: path.to_owned(),
        source,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let tmp = sibling(path, "postrinch-tmp");
    std::fs::write(&tmp, text).map_err(io)?;
    if existed {
        std::fs::copy(path, sibling(path, "postrinch-backup")).map_err(io)?;
        let permissions = std::fs::metadata(path).map_err(io)?.permissions();
        std::fs::set_permissions(&tmp, permissions).map_err(io)?;
    }
    std::fs::rename(&tmp, path).map_err(io)
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(suffix);
    path.with_file_name(name)
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
