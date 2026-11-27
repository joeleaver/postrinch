//! OFSCP accounts from the command line, for development (docs/ofscp.md,
//! "Crates": `add-ofscp`, `ofscp sync`): add one, list them, sign one in
//! again, sync, and sign out and remove.

use clap::Subcommand;
use postrinch_social::{Added, Revocation, Social, Synced};
use postrinch_store::{SocialAccount, SocialAccountState};

use crate::add_imap::Passwords;
use crate::commands::Result;

#[derive(Subcommand)]
pub enum OfscpCommand {
    /// List the OFSCP accounts and whether each can sync.
    Accounts,
    /// Sign an account in again with its password (read as `add-ofscp`
    /// reads it): a new device key, the old one revoked. Then sync it.
    SignIn {
        actor: String,
        /// Read the password from standard input, not a prompt.
        #[arg(long)]
        password_stdin: bool,
    },
    /// Sync one account, or every account that can: its groups, channels,
    /// conversations and each one's newest page.
    Sync { actor: Option<String> },
    /// Sign an account out of this computer (its device key revoked) and
    /// remove everything stored for it.
    Remove { actor: String },
}

/// Adds an account: shows what discovery found, then signs in with the
/// password from `passwords`, registers a device key and runs the first
/// sync. Nothing is kept if a step fails.
pub async fn add(social: &Social, actor: &str, passwords: &mut dyn Passwords) -> Result<()> {
    let found = social.look_up(actor).await?;
    let provider = &found.discovery.provider;
    eprintln!(
        "{} is on {} ({} {}, OFSCP {}).",
        found.actor,
        provider.domain,
        provider.software.name,
        provider.software.version,
        provider.protocol_version
    );
    let password = passwords.password(&found.actor.to_string())?;
    let Added { account, synced } = social.add(&found, &password).await?;
    println!(
        "Added {} as \"{}\". {}",
        account.actor,
        social.device_name(),
        summary(&synced)
    );
    Ok(())
}

pub async fn run(
    social: &Social,
    command: OfscpCommand,
    passwords: &mut dyn Passwords,
) -> Result<()> {
    match command {
        OfscpCommand::Accounts => accounts(social),
        OfscpCommand::SignIn { actor, .. } => {
            let password = passwords.password(&actor)?;
            let account = social.sign_in_again(&actor, &password).await?;
            println!("Signed in as {}.", account.actor);
            let synced = social.sync(&account.actor).await?;
            println!("{}: {}", account.actor, summary(&synced));
            Ok(())
        }
        OfscpCommand::Sync { actor } => sync(social, actor.as_deref()).await,
        OfscpCommand::Remove { actor } => {
            let removed = social.remove(&actor).await?;
            match removed.revocation {
                Revocation::Revoked | Revocation::AlreadyGone => {
                    println!("Signed out and removed {actor}.");
                }
                Revocation::Failed(why) => println!(
                    "Removed {actor} from this computer, but its device key could not be revoked \
                     ({why}); revoke \"{}\" from another device.",
                    social.device_name()
                ),
            }
            Ok(())
        }
    }
}

/// Lists the accounts, each with its state.
pub fn accounts(social: &Social) -> Result<()> {
    list(&social.store().social_accounts()?);
    Ok(())
}

/// Prints `accounts`, one a line: actor, name, state.
pub fn list(accounts: &[SocialAccount]) {
    for account in accounts {
        let name = account
            .profile
            .as_ref()
            .and_then(|p| p.get("displayName"))
            .and_then(|n| n.as_str())
            .unwrap_or("");
        println!("{}\t{name}\t{}", account.actor, state(account.state));
    }
}

async fn sync(social: &Social, only: Option<&str>) -> Result<()> {
    let accounts = match only {
        Some(actor) => vec![actor.to_owned()],
        None => social
            .store()
            .social_accounts_in(SocialAccountState::Active)?
            .into_iter()
            .map(|a| a.actor)
            .collect(),
    };
    if accounts.is_empty() {
        return Err(
            "no OFSCP accounts that can sync; add one with `postrinch-cli add-ofscp`".into(),
        );
    }
    let mut failed = false;
    for actor in accounts {
        match social.sync(&actor).await {
            Ok(synced) => println!("{actor}: {}", summary(&synced)),
            Err(e) => {
                eprintln!("{actor}: {e}");
                failed = true;
            }
        }
    }
    if failed {
        Err("some accounts did not sync".into())
    } else {
        Ok(())
    }
}

fn state(state: SocialAccountState) -> &'static str {
    match state {
        SocialAccountState::Active => "signed in",
        SocialAccountState::NeedsSignIn => "must sign in again",
        SocialAccountState::ClockSkewed => "this computer's clock is off",
    }
}

fn summary(synced: &Synced) -> String {
    let mut text = format!(
        "{} groups, {} places, {} messages",
        synced.groups, synced.places, synced.messages
    );
    if synced.skipped > 0 {
        text.push_str(&format!(
            " ({} no longer readable, left out)",
            synced.skipped
        ));
    }
    text
}

#[cfg(test)]
#[path = "ofscp_tests.rs"]
mod tests;
