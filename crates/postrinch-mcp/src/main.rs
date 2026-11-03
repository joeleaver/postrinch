//! `postrinch-mcp`: an MCP server over stdio through which an agent reads
//! and changes the person's mail. It holds no mail itself: each tool call
//! goes to the running Postrinch app as the agent whose key is in
//! `POSTRINCH_AGENT_KEY` (see `postrinch-agents`).
//!
//! stdout carries the protocol and nothing else: logs go to stderr.

// A GUI client that launches this on Windows shows no console window.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod tools;

use std::sync::Arc;

use postrinch_agents::client::Client;
use postrinch_agents::wire;
use rmcp::ServiceExt;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let key = std::env::var(wire::KEY_ENV).ok();
    let client = Arc::new(Client::new(wire::addr(), key));
    match tools::Postrinch::new(client)
        .serve(rmcp::transport::stdio())
        .await
    {
        Ok(service) => {
            if let Err(e) = service.waiting().await {
                tracing::error!("the MCP session ended with an error: {e}");
            }
        }
        Err(e) => tracing::error!("the MCP session could not start: {e}"),
    }
}
