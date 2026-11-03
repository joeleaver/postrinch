//! The app's side: listen on loopback and answer agents' calls.
//!
//! One thread accepts connections; each connection gets a thread of its
//! own, as the command layer is blocking. A connection must open with the
//! key of an agent in the agents file. Every call reads the file again, so
//! a removed agent's next call is refused and a narrowed one sees less.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use postrinch_commands::Commands;
use serde::Serialize;

use crate::registry::{Agent, Agents, Reach};
use crate::views;
use crate::wire::{Greeting, Hello, MAX_LINE, Outcome, Request, Response};

/// Called after an agent changed mail: the window shows it and sync sends it.
pub type OnChange = Arc<dyn Fn() + Send + Sync>;

/// Listen at `addr` (see [`crate::wire::addr`]) in the background.
pub fn start(
    addr: &str,
    commands: Commands,
    agents_path: PathBuf,
    on_change: OnChange,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    tracing::info!("agents can connect at {addr}");
    start_on(listener, commands, agents_path, on_change)
}

/// Answer agents on a listener already bound.
pub fn start_on(
    listener: TcpListener,
    commands: Commands,
    agents_path: PathBuf,
    on_change: OnChange,
) -> std::io::Result<()> {
    thread::Builder::new()
        .name("agents".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let stream = match stream {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("an agent's connection failed: {e}");
                        continue;
                    }
                };
                let link = Link {
                    commands: commands.clone(),
                    agents_path: agents_path.clone(),
                    on_change: on_change.clone(),
                };
                let spawned = thread::Builder::new()
                    .name("agent".into())
                    .spawn(move || link.serve(stream));
                if let Err(e) = spawned {
                    tracing::warn!("an agent's connection could not start: {e}");
                }
            }
        })?;
    Ok(())
}

struct Link {
    commands: Commands,
    agents_path: PathBuf,
    on_change: OnChange,
}

impl Link {
    fn serve(&self, stream: TcpStream) {
        if let Err(e) = self.converse(stream) {
            tracing::debug!("an agent's connection closed: {e}");
        }
    }

    fn converse(&self, stream: TcpStream) -> std::io::Result<()> {
        let mut out = stream.try_clone()?;
        let mut lines = BufReader::new(stream.take(MAX_LINE));
        let mut line = String::new();

        lines.read_line(&mut line)?;
        let key = match serde_json::from_str::<Hello>(&line) {
            Ok(hello) => hello.key,
            Err(_) => String::new(),
        };
        let agent = match self.agent(&key) {
            Ok(agent) => agent,
            Err(reason) => return send(&mut out, &Greeting::Refused { reason }),
        };
        tracing::info!("agent {:?} connected", agent.name);
        send(
            &mut out,
            &Greeting::Welcome {
                agent: agent.name.clone(),
            },
        )?;

        loop {
            line.clear();
            // Each line may be up to MAX_LINE long.
            lines.get_mut().set_limit(MAX_LINE);
            if lines.read_line(&mut line)? == 0 {
                return Ok(());
            }
            let request: Request = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!("an agent sent a request that does not read: {e}");
                    return Ok(());
                }
            };
            let outcome = match self.agent(&key) {
                Ok(agent) => self.run(&agent, request.call),
                Err(reason) => Outcome::Error(reason),
            };
            send(
                &mut out,
                &Response {
                    id: request.id,
                    outcome,
                },
            )?;
        }
    }

    /// The agent a key belongs to, read from the file now.
    fn agent(&self, key: &str) -> Result<Agent, String> {
        let mut agents = Agents::load(&self.agents_path).map_err(|e| {
            tracing::error!("{e}");
            "Postrinch could not read its agents file.".to_owned()
        })?;
        let Some(agent) = agents.find_by_key(key).cloned() else {
            return Err(
                "This agent was removed in Postrinch, or its key is wrong: set it up again.".into(),
            );
        };
        if let Err(e) = agents.touch(&agent.id) {
            tracing::warn!("{e}");
        }
        Ok(agent)
    }

    fn run(&self, agent: &Agent, call: crate::wire::Call) -> Outcome {
        let commands = match &agent.reach {
            Reach::Every => self.commands.clone(),
            Reach::Only(accounts) => self.commands.limited_to(accounts),
        };
        let changes = call.changes();
        match views::run(&commands, call) {
            Ok(value) => {
                if changes {
                    (self.on_change)();
                }
                Outcome::Ok(value)
            }
            Err(sentence) => Outcome::Error(sentence),
        }
    }
}

fn send(out: &mut TcpStream, value: &impl Serialize) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()
}
