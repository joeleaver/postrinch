//! `postrinch`: add Google and IMAP accounts, sync them, and search the
//! mail, from a terminal. The window will do all of this; until it exists
//! this is how a person or a script drives Postrinch.

mod accounts;
mod add_imap;
mod agent;
mod commands;
mod context;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing_subscriber::prelude::*;

#[derive(Parser)]
#[command(
    name = "postrinch-cli",
    version,
    about = "Postrinch from the command line"
)]
struct Cli {
    /// Where the store lives. Defaults to `<data dir>/postrinch`.
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// The Google OAuth client file. Defaults to
    /// `<config dir>/postrinch/google-oauth-client.json`.
    #[arg(long, global = true)]
    client: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List the accounts and whether each is signed in.
    Accounts,
    /// Add an IMAP account, or change one's servers or password: the
    /// servers from the options or discovery, the password from standard
    /// input or a prompt (never an argument), or --oauth to sign in in the
    /// browser. Both servers are checked before anything is kept.
    AddImap(add_imap::AddImapArgs),
    /// Add a Google account: sign in in the browser.
    AddGoogle,
    /// Sign an account in again.
    SignIn { address: String },
    /// Sign an account out and remove it and its mail from this machine.
    RemoveAccount { address: String },
    /// Sync one account, or every signed-in account, then download the
    /// bodies of messages known only by their headers, newest first.
    Sync {
        address: Option<String>,
        /// Stop after the sync; leave the bodies for a later run.
        #[arg(long)]
        no_backfill: bool,
    },
    /// Throw away an account's local mail and sync it again from scratch.
    /// The account stays signed in; raw messages already downloaded are
    /// reused.
    Resync { address: String },
    /// Rebuild an account's downloaded messages from the originals stored
    /// here, after a change to how Postrinch reads mail. Needs no network.
    Reparse { address: String },
    /// Compact the store's files once after an engine upgrade, as the
    /// window does when it opens (a no-op when already done).
    Compact,
    /// Search all accounts' mail.
    Search {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// List the views, each with what the sidebar counts beside it and
    /// how long listing it took.
    Views,
    /// Show a message's headers and text.
    Show { address: String, message_id: String },
    /// Make, list and remove the agents that may connect through
    /// postrinch-mcp. Works while the window is open.
    #[command(subcommand)]
    Agent(AgentCommand),
}

#[derive(Subcommand)]
enum AgentCommand {
    /// Make an agent. With --app, set that app up to launch it, as
    /// Settings › Agents does; without, print its key, once, with the
    /// entry to give the app.
    Add {
        /// What to call it: the app it is for, say.
        name: String,
        /// An account it may see; repeat for more. Without any, it sees
        /// every account, those added later included.
        #[arg(long)]
        account: Vec<String>,
        /// The app to set up. Claude covers its Chat and Code tabs and the
        /// terminal's `claude`, and replaces the agents Claude had.
        #[arg(long, value_enum)]
        app: Option<AgentApp>,
    },
    /// List the agents.
    List,
    /// Remove an agent by name: its key stops working at once.
    Remove { name: String },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum AgentApp {
    Claude,
    Codex,
    Cursor,
}

#[tokio::main]
async fn main() -> ExitCode {
    // RUST_LOG or warnings, never the mail protocols' raw lines
    // (postrinch-log says why).
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(postrinch_log::filter("warn")),
        )
        .init();

    let cli = Cli::parse();
    let result = match cli.command {
        Command::Agent(command) => agent::run(command),
        _ => commands::run(cli).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("postrinch-cli: {e}");
            ExitCode::FAILURE
        }
    }
}
