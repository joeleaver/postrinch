//! `postrinch`: add Google accounts, sync them, and search the mail, from a
//! terminal. The window will do all of this; until it exists this is how a
//! person or a script drives Postrinch.

mod commands;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

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
    /// Search all accounts' mail.
    Search {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show a message's headers and text.
    Show { address: String, message_id: String },
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match commands::run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("postrinch: {e}");
            ExitCode::FAILURE
        }
    }
}
