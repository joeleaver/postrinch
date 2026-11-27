//! OFSCP accounts from the command line, for development (docs/ofscp.md,
//! "Crates": `add-ofscp`, `ofscp sync`): add one, list them, sign one in
//! again, sync, send what is queued, fetch an avatar, and sign out and
//! remove.

use std::path::PathBuf;

use clap::Subcommand;
use postrinch_social::{Added, LiveEvent, Revocation, Sent, Social, Synced};
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
    /// Sync one account, or every account not waiting to sign in: its groups, channels,
    /// conversations and each one's newest page.
    Sync { actor: Option<String> },
    /// Send what an account has queued: its posts' files uploaded first,
    /// each upload's progress shown as it goes.
    Send { actor: String },
    /// Fetch a user's avatar, as an account sees it, into a file. Served
    /// from the media cache while it is fresh.
    Avatar {
        actor: String,
        /// The user whose avatar it is (handle@domain).
        user: String,
        /// Where to write it.
        out: PathBuf,
    },
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
        OfscpCommand::Send { actor } => send(social, &actor).await,
        OfscpCommand::Avatar { actor, user, out } => avatar(social, &actor, &user, &out).await,
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
        // An account whose clock was off is tried again: a sync home
        // answers makes it active.
        None => social
            .store()
            .social_accounts()?
            .into_iter()
            .filter(|a| a.state != SocialAccountState::NeedsSignIn)
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

/// One pass over `actor`'s outbox, telling each upload's progress on
/// standard error as each tenth of it goes.
async fn send(social: &Social, actor: &str) -> Result<()> {
    let mut events = social.live_events();
    let progress = tokio::spawn(async move {
        let mut shown: Option<(String, u64)> = None;
        while let Ok(event) = events.recv().await {
            let LiveEvent::Uploading(upload) = event else {
                continue;
            };
            let tenth = (upload.sent * 10).checked_div(upload.total).unwrap_or(10);
            let at = (format!("{}/{}", upload.client_id, upload.file), tenth);
            if shown.as_ref() != Some(&at) {
                eprintln!(
                    "uploading {} ({} of {}): {}%",
                    upload.name,
                    upload.file + 1,
                    upload.files,
                    tenth * 10
                );
                shown = Some(at);
            }
        }
    });
    let sent = social.send_outbox(actor).await;
    progress.abort();
    let sent = sent?;
    println!("{actor}: {}", sent_summary(&sent));
    Ok(())
}

fn sent_summary(sent: &Sent) -> String {
    format!(
        "{} sent, {} to try again, {} refused, {} waiting for files",
        sent.sent, sent.failed, sent.refused, sent.waiting
    )
}

/// Write `user`'s avatar, as `actor`'s account sees it, to `out`.
async fn avatar(social: &Social, actor: &str, user: &str, out: &std::path::Path) -> Result<()> {
    let Some(avatar) = social.fetch_avatar(actor, user).await? else {
        println!("{user} has no avatar.");
        return Ok(());
    };
    std::fs::write(out, &avatar.bytes)?;
    println!(
        "{} bytes{} to {}{}",
        avatar.bytes.len(),
        avatar
            .media
            .mime
            .as_deref()
            .map(|m| format!(" ({m})"))
            .unwrap_or_default(),
        out.display(),
        if avatar.from_cache {
            ", from the cache"
        } else {
            ""
        }
    );
    Ok(())
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
