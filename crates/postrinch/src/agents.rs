//! Agents connect through `postrinch-mcp`: the app listens on loopback and
//! answers them through the command layer (see `postrinch-agents`).

use std::sync::Arc;

use postrinch_agents::{Agents, serve, wire};
use postrinch_commands::Commands;
use tokio::sync::Notify;

use crate::state::State;

/// Listen for agents. A change an agent makes reloads what the window shows
/// and wakes sync's sender, as the window's own changes do.
pub fn start(commands: Commands, state: State, send: Option<Arc<Notify>>) {
    let Some(path) = Agents::default_path() else {
        tracing::warn!("agents are off: this system has no config directory");
        return;
    };
    let on_change: serve::OnChange = Arc::new(move || {
        if let Some(send) = &send {
            send.notify_one();
        }
        state.revision.update_send(|n| *n += 1);
    });
    let addr = wire::addr();
    if let Err(e) = serve::start(&addr, commands, path, on_change) {
        // Another Postrinch is answering there, or something else holds it.
        tracing::warn!("agents cannot connect: listening at {addr} failed: {e}");
    }
}
