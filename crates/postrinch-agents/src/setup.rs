//! The apps Postrinch can set up by itself, and where each keeps its MCP
//! servers. Connecting and disconnecting agents is [`crate::connect`];
//! reading and changing one settings file is [`crate::setup_file`].
//!
//! Claude is one choice that covers the Chat tab, the desktop app's Code
//! tab and the terminal's `claude`. Chat reads only the desktop app's file
//! and the terminal only `~/.claude.json`, while the Code tab reads both.
//! So the same entry (one name, one command, one key) goes into each file
//! whose Claude is here, and the Code tab, finding it twice under one name,
//! loads it once.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

pub use crate::setup_file::{Kind, Place};
use crate::wire::KEY_ENV;

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("this system has no home folder")]
    NoHome,
    #[error("{0} is not on this computer")]
    NotHere(&'static str),
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
    /// Claude: the desktop app's Chat and Code tabs and the terminal's
    /// `claude`. Agents set up before these were one choice read as it.
    #[serde(alias = "claude_desktop", alias = "claude_code")]
    Claude,
    Codex,
    Cursor,
}

impl App {
    pub const ALL: [App; 3] = [App::Claude, App::Codex, App::Cursor];

    pub fn name(self) -> &'static str {
        match self {
            App::Claude => "Claude",
            App::Codex => "Codex",
            App::Cursor => "Cursor",
        }
    }

    /// Every file the app may read its MCP servers from, whether or not
    /// that part of it is on this computer.
    pub fn places(self) -> Result<Vec<Place>, SetupError> {
        let home = dirs::home_dir().ok_or(SetupError::NoHome)?;
        let place = |path: PathBuf, kind: Kind, installed: bool| Place {
            installed: installed || path.exists(),
            path,
            kind,
        };
        let beside = |path: &Path| path.parent().is_some_and(Path::is_dir);
        Ok(match self {
            App::Claude => {
                let desktop = dirs::config_dir()
                    .ok_or(SetupError::NoHome)?
                    .join("Claude")
                    .join("claude_desktop_config.json");
                let desktop_here = beside(&desktop);
                vec![
                    // ~/.claude.json sits in the home folder itself, so the
                    // folder beside it says nothing: ~/.claude/ does.
                    place(
                        home.join(".claude.json"),
                        Kind::ClaudeCode,
                        home.join(".claude").is_dir(),
                    ),
                    place(desktop, Kind::ClaudeDesktop, desktop_here),
                ]
            }
            App::Cursor => {
                let path = home.join(".cursor").join("mcp.json");
                let here = beside(&path);
                vec![place(path, Kind::Cursor, here)]
            }
            App::Codex => {
                let path = match std::env::var_os("CODEX_HOME") {
                    Some(dir) => PathBuf::from(dir).join("config.toml"),
                    None => home.join(".codex").join("config.toml"),
                };
                let here = beside(&path);
                vec![place(path, Kind::Codex, here)]
            }
        })
    }

    /// The files connecting writes: those of the parts that are here.
    pub fn installed_places(self) -> Result<Vec<Place>, SetupError> {
        Ok(self.places()?.into_iter().filter(|p| p.installed).collect())
    }

    /// Whether the app looks installed: some settings file of its, or the
    /// folder it goes in, is there.
    pub fn found(self) -> bool {
        self.installed_places().is_ok_and(|p| !p.is_empty())
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

const BRIDGE: &str = "postrinch-mcp";

/// `postrinch-mcp`, beside the program running.
pub fn bridge_path() -> PathBuf {
    let name = if cfg!(windows) {
        "postrinch-mcp.exe"
    } else {
        BRIDGE
    };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Whether a server's command launches Postrinch's bridge, wherever it is.
pub fn is_bridge(command: &str) -> bool {
    // Either separator: a file may have been written on the other system.
    let file = command.rsplit(['/', '\\']).next().unwrap_or(command);
    file == BRIDGE || file.eq_ignore_ascii_case("postrinch-mcp.exe")
}

/// The entry to paste, for apps set up by hand.
pub fn json_snippet(entry: &str, bridge: &Path, key: &str) -> String {
    let value = json!({ "mcpServers": { entry: { "command": bridge, "env": { KEY_ENV: key } } } });
    serde_json::to_string_pretty(&value).expect("JSON serializes")
}

pub fn toml_snippet(entry: &str, bridge: &Path, key: &str) -> String {
    crate::setup_file::toml_snippet(entry, bridge, key)
}

pub fn command_snippet(bridge: &Path, key: &str) -> String {
    format!(
        "Command:      {}\nEnvironment:  {KEY_ENV}={key}",
        bridge.display()
    )
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
