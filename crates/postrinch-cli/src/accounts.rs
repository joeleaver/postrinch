//! Listing accounts, signing them in (again), and removing them, each
//! with its own provider.

use postrinch_model::{AccountState, EmailAddress, Provider, SignInMethod};
use postrinch_sync::google::{remove_google_account, sign_in_google};
use postrinch_sync::imap::{Added, ImapSecret, SignInAgain};

use crate::add_imap::{Passwords, Terminal};
use crate::commands::Result;
use crate::context::Context;

pub fn accounts(cx: &Context) -> Result<()> {
    let accounts = cx.store.accounts()?;
    let social = cx.store.social_accounts()?;
    if accounts.is_empty() && social.is_empty() {
        println!(
            "No accounts. Add one with `postrinch-cli add-google`, `add-imap` or `add-ofscp`."
        );
    }
    for account in accounts {
        let state = match &account.state {
            AccountState::Active => "signed in".to_owned(),
            AccountState::NeedsSignIn(reason) => format!("must sign in again: {reason}"),
        };
        let name = account.display_name.as_deref().unwrap_or("");
        println!("{}\t{name}\t{state}", account.address);
    }
    crate::ofscp::list(&social);
    Ok(())
}

/// Signs a Google account in (a new one with no address), or any
/// account in again: a Google or OAuth IMAP account in the browser, a
/// password account with its password, asked for, checked against both
/// servers before it is kept.
pub async fn sign_in(cx: &Context, address: Option<EmailAddress>) -> Result<()> {
    let stored = match &address {
        Some(address) => cx.store.account(address)?,
        None => None,
    };
    let Some(stored) = stored.filter(|a| a.provider == Provider::Imap) else {
        let auth = cx.auth()?;
        let account = sign_in_google(&auth, &cx.store, address.as_ref(), |url| {
            eprintln!("Opening your browser to sign in. If it does not open, visit:\n{url}\n");
            open::that_detached(url)
        })
        .await?;
        println!("Signed in as {}.", account.address);
        return Ok(());
    };
    let oauth = cx.imap_oauth()?;
    let imap = cx.imap(&oauth)?;
    let settings = stored
        .imap
        .as_ref()
        .ok_or_else(|| format!("{} has no IMAP settings", stored.address))?;
    let address = stored.address.clone();
    let mut terminal = Terminal { from_stdin: false };
    let how = match settings.sign_in {
        SignInMethod::OAuth2 { .. } => SignInAgain::Browser(Box::new(|url: &str| {
            eprintln!("Opening your browser to sign in. If it does not open, visit:\n{url}\n");
            open::that_detached(url)
        })),
        SignInMethod::Password => {
            SignInAgain::Password(ImapSecret::password(terminal.password("IMAP and SMTP")?))
        }
        SignInMethod::SeparateSmtpPassword => SignInAgain::Password(ImapSecret::Password {
            password: terminal.password("IMAP")?,
            smtp_password: Some(terminal.password("SMTP")?),
        }),
    };
    match imap.sign_in_again(&oauth, &address, how).await? {
        Added::Account(_) => {
            println!("Signed in as {address}.");
            Ok(())
        }
        Added::CheckFailed(check) => {
            Err(format!("{address} did not sign in; nothing was kept.\n{check}").into())
        }
    }
}

/// Signs an account out and removes it and its mail from this machine.
pub async fn remove_account(cx: &Context, address: &EmailAddress) -> Result<()> {
    let account = cx
        .store
        .account(address)?
        .ok_or_else(|| format!("no account {address}"))?;
    let removed = match account.provider {
        Provider::Google => remove_google_account(&*cx.auth()?, &cx.store, address).await?,
        Provider::Imap => {
            let oauth = cx.imap_oauth()?;
            cx.imap(&oauth)?.remove_account(&oauth, address).await?
        }
    };
    println!("Removed {address} and {removed} stored messages' raw copies.");
    Ok(())
}
