//! One app's settings file: its MCP servers read, changed and written back,
//! with nothing else in it changed.
//!
//! Before a file is changed it is copied beside itself
//! (`<file>.postrinch-backup`); the new one is written whole and renamed into
//! place, keeping the old one's permissions. A file that does not parse,
//! or whose servers are not where they should be, is left alone: the person
//! gets the entry to paste instead.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::setup::SetupError;
use crate::wire::KEY_ENV;

/// Which app's file a place is, which says how it is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `~/.claude.json`: read by the terminal's `claude` and the desktop
    /// app's Code tab. Servers sit at the top (the person's own) and under
    /// `projects.<folder>` (one project's).
    ClaudeCode,
    /// `claude_desktop_config.json`: read by the desktop app, both tabs.
    ClaudeDesktop,
    Cursor,
    Codex,
}

/// A file an app reads its MCP servers from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub path: PathBuf,
    pub kind: Kind,
    /// Whether that part of the app is on this computer.
    pub installed: bool,
}

/// A server entry in a settings file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub name: String,
    /// Under `projects.<folder>` in `~/.claude.json`: that project's.
    pub project: Option<String>,
    pub command: Option<String>,
    pub key: Option<String>,
}

/// A settings file as read, to change and write back.
pub(crate) struct SettingsFile {
    place: Place,
    existed: bool,
    changed: bool,
    doc: Doc,
}

enum Doc {
    Json(Value),
    Toml(DocumentMut),
}

impl SettingsFile {
    /// Read the file; one not there yet is empty.
    pub fn read(place: &Place) -> Result<Self, SetupError> {
        let text = match std::fs::read_to_string(&place.path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(SetupError::Io {
                    path: place.path.clone(),
                    source,
                });
            }
        };
        let unreadable = || SetupError::Unreadable(place.path.clone());
        let doc = if place.kind == Kind::Codex {
            let doc: DocumentMut = text
                .as_deref()
                .unwrap_or_default()
                .parse()
                .map_err(|_| unreadable())?;
            if doc.get("mcp_servers").is_some_and(|s| !s.is_table()) {
                return Err(unreadable());
            }
            Doc::Toml(doc)
        } else {
            let doc: Value = match text.as_deref() {
                Some(t) if !t.trim().is_empty() => {
                    serde_json::from_str(t).map_err(|_| unreadable())?
                }
                _ => json!({}),
            };
            let servers_ok = doc
                .as_object()
                .is_some_and(|root| root.get("mcpServers").is_none_or(Value::is_object));
            if !servers_ok {
                return Err(unreadable());
            }
            Doc::Json(doc)
        };
        Ok(Self {
            place: place.clone(),
            existed: text.is_some(),
            changed: false,
            doc,
        })
    }

    pub fn path(&self) -> &Path {
        &self.place.path
    }

    /// Every server in the file, the project ones in `~/.claude.json` too.
    pub fn servers(&self) -> Vec<Server> {
        let mut found = Vec::new();
        match &self.doc {
            Doc::Json(doc) => {
                for (project, servers) in self.json_scopes(doc) {
                    for (name, server) in servers {
                        let text = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_owned);
                        found.push(Server {
                            name: name.clone(),
                            project: project.clone(),
                            command: text(server.get("command")),
                            key: text(server.get("env").and_then(|e| e.get(KEY_ENV))),
                        });
                    }
                }
            }
            Doc::Toml(doc) => {
                let Some(servers) = doc.get("mcp_servers").and_then(Item::as_table_like) else {
                    return found;
                };
                for (name, server) in servers.iter() {
                    let text = |i: Option<&Item>| i.and_then(Item::as_str).map(str::to_owned);
                    let env = server.as_table_like().and_then(|s| s.get("env"));
                    found.push(Server {
                        name: name.to_owned(),
                        project: None,
                        command: text(server.as_table_like().and_then(|s| s.get("command"))),
                        key: text(
                            env.and_then(Item::as_table_like)
                                .and_then(|e| e.get(KEY_ENV)),
                        ),
                    });
                }
            }
        }
        found
    }

    /// Take out every server `drop` picks. Returns those taken out.
    pub fn drop_servers(&mut self, drop: impl Fn(&Server) -> bool) -> Vec<Server> {
        let gone: Vec<Server> = self.servers().into_iter().filter(|s| drop(s)).collect();
        if gone.is_empty() {
            return gone;
        }
        match &mut self.doc {
            Doc::Json(doc) => {
                for server in &gone {
                    let scope = match &server.project {
                        None => doc.get_mut("mcpServers"),
                        Some(project) => doc
                            .get_mut("projects")
                            .and_then(|p| p.get_mut(project))
                            .and_then(|p| p.get_mut("mcpServers")),
                    };
                    if let Some(servers) = scope.and_then(Value::as_object_mut) {
                        servers.shift_remove(&server.name);
                    }
                }
            }
            Doc::Toml(doc) => {
                if let Some(servers) = doc.get_mut("mcp_servers").and_then(Item::as_table_like_mut)
                {
                    for server in &gone {
                        servers.remove(&server.name);
                    }
                }
            }
        }
        self.changed = true;
        gone
    }

    /// Add (or replace) the person's own entry `entry` launching `bridge`
    /// with `key`.
    pub fn insert(&mut self, entry: &str, bridge: &Path, key: &str) {
        match &mut self.doc {
            Doc::Json(doc) => {
                let mut server = json!({ "command": bridge, "env": { KEY_ENV: key } });
                if self.place.kind == Kind::ClaudeCode {
                    server["type"] = json!("stdio");
                }
                let root = doc.as_object_mut().expect("checked when read");
                let servers = root
                    .entry("mcpServers")
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .expect("checked when read");
                servers.insert(entry.to_owned(), server);
            }
            Doc::Toml(doc) => toml_insert(doc, entry, bridge, key),
        }
        self.changed = true;
    }

    /// Write the file back, if anything changed. Returns whether it did.
    pub fn write(self) -> Result<bool, SetupError> {
        if !self.changed {
            return Ok(false);
        }
        let text = match &self.doc {
            Doc::Json(doc) => {
                let mut text = serde_json::to_string_pretty(doc).expect("JSON serializes");
                text.push('\n');
                text
            }
            Doc::Toml(doc) => doc.to_string(),
        };
        write(&self.place.path, self.existed, &text)?;
        Ok(true)
    }

    /// The JSON file's server maps: the person's own, then (in
    /// `~/.claude.json`) each project's.
    fn json_scopes<'a>(&self, doc: &'a Value) -> Vec<(Option<String>, &'a Map<String, Value>)> {
        let mut scopes = Vec::new();
        if let Some(servers) = doc.get("mcpServers").and_then(Value::as_object) {
            scopes.push((None, servers));
        }
        if self.place.kind == Kind::ClaudeCode
            && let Some(projects) = doc.get("projects").and_then(Value::as_object)
        {
            for (folder, project) in projects {
                if let Some(servers) = project.get("mcpServers").and_then(Value::as_object) {
                    scopes.push((Some(folder.clone()), servers));
                }
            }
        }
        scopes
    }
}

/// `[mcp_servers.<entry>]` set, comments and the rest kept.
fn toml_insert(doc: &mut DocumentMut, entry: &str, bridge: &Path, key: &str) {
    let servers = doc
        .entry("mcp_servers")
        .or_insert_with(|| {
            let mut t = Table::new();
            t.set_implicit(true);
            Item::Table(t)
        })
        .as_table_mut()
        .expect("checked when read");
    let mut server = Table::new();
    server["command"] = value(bridge.display().to_string());
    let mut env = Table::new();
    env[KEY_ENV] = value(key);
    server["env"] = Item::Table(env);
    servers[entry] = Item::Table(server);
}

pub(crate) fn toml_snippet(entry: &str, bridge: &Path, key: &str) -> String {
    let mut doc = DocumentMut::new();
    toml_insert(&mut doc, entry, bridge, key);
    doc.to_string()
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
