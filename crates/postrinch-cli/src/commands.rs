use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use postrinch_model::{AccountState, EmailAddress, Provider, ProviderMessageId};
use postrinch_store::Store;
use postrinch_sync::google::{
    ClientConfig, GmailApi, GmailSync, GoogleAuth, Keychain, Phase, Progress, ProgressFn,
    remove_google_account, sign_in_google,
};

use crate::{Cli, Command};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Context {
    store: Store,
    client: Option<PathBuf>,
}

impl Context {
    fn auth(&self) -> Result<Arc<GoogleAuth>> {
        let path = match &self.client {
            Some(path) => path.clone(),
            None => ClientConfig::default_path().ok_or("no config directory; pass --client")?,
        };
        let config = ClientConfig::load(&path)?;
        Ok(Arc::new(GoogleAuth::new(config, Arc::new(Keychain))))
    }
}

pub(crate) async fn run(cli: Cli) -> Result<()> {
    let data_dir = match cli.data_dir {
        Some(dir) => dir,
        None => dirs::data_dir()
            .ok_or("no data directory; pass --data-dir")?
            .join("postrinch"),
    };
    let store = Store::open(&data_dir)?;
    let cx = Context {
        store,
        client: cli.client,
    };
    match cli.command {
        Command::Accounts => accounts(&cx),
        Command::AddGoogle => sign_in(&cx, None).await,
        Command::SignIn { address } => sign_in(&cx, Some(parse(&address)?)).await,
        Command::RemoveAccount { address } => remove_account(&cx, &parse(&address)?).await,
        Command::Sync {
            address,
            no_backfill,
        } => {
            sync(
                &cx,
                address.as_deref().map(parse).transpose()?,
                !no_backfill,
            )
            .await
        }
        Command::Resync { address } => resync(&cx, parse(&address)?).await,
        Command::Reparse { address } => {
            let address = parse(&address)?;
            let rebuilt = postrinch_sync::reparse(&cx.store, &address)?;
            eprintln!("{address}: rebuilt {rebuilt} messages from their originals");
            Ok(())
        }
        Command::Search { query, limit } => search(&cx, &query, limit),
        Command::Show {
            address,
            message_id,
        } => show(&cx, &parse(&address)?, message_id),
        Command::Agent(_) => unreachable!("main runs agent commands without the store"),
    }
}

fn parse(address: &str) -> Result<EmailAddress> {
    Ok(EmailAddress::parse(address)?)
}

fn accounts(cx: &Context) -> Result<()> {
    let accounts = cx.store.accounts()?;
    if accounts.is_empty() {
        println!("No accounts. Add one with `postrinch-cli add-google`.");
    }
    for account in accounts {
        let state = match &account.state {
            AccountState::Active => "signed in".to_owned(),
            AccountState::NeedsSignIn(reason) => format!("must sign in again: {reason}"),
        };
        let name = account.display_name.as_deref().unwrap_or("");
        println!("{}\t{name}\t{state}", account.address);
    }
    Ok(())
}

async fn sign_in(cx: &Context, address: Option<EmailAddress>) -> Result<()> {
    let auth = cx.auth()?;
    let account = sign_in_google(&auth, &cx.store, address.as_ref(), |url| {
        eprintln!("Opening your browser to sign in. If it does not open, visit:\n{url}\n");
        open::that_detached(url)
    })
    .await?;
    println!("Signed in as {}.", account.address);
    Ok(())
}

async fn remove_account(cx: &Context, address: &EmailAddress) -> Result<()> {
    let account = cx
        .store
        .account(address)?
        .ok_or_else(|| format!("no account {address}"))?;
    let removed = match account.provider {
        Provider::Google => remove_google_account(&*cx.auth()?, &cx.store, address).await?,
        Provider::Imap => {
            cx.store.remove_account(address)?;
            cx.store.collect_garbage(std::time::Duration::ZERO)?
        }
    };
    println!("Removed {address} and {removed} stored messages' raw copies.");
    Ok(())
}

async fn resync(cx: &Context, address: EmailAddress) -> Result<()> {
    let removed = cx.store.clear_mail(&address)?;
    eprintln!("{address}: cleared {removed} messages; syncing again");
    sync(cx, Some(address), true).await?;
    // Raw messages the provider no longer has; the rest were reused.
    let unused = cx.store.collect_garbage(std::time::Duration::ZERO)?;
    if unused > 0 {
        eprintln!("removed {unused} raw messages no longer in the account");
    }
    Ok(())
}

async fn sync(cx: &Context, only: Option<EmailAddress>, backfill: bool) -> Result<()> {
    let accounts: Vec<_> = cx
        .store
        .accounts()?
        .into_iter()
        .filter(|a| a.provider == Provider::Google)
        .filter(|a| only.as_ref().is_none_or(|o| o.key() == a.address.key()))
        .collect();
    if accounts.is_empty() {
        return Err(match only {
            Some(address) => format!("no Google account {address}").into(),
            None => "no Google accounts; add one with `postrinch-cli add-google`".into(),
        });
    }
    let auth = cx.auth()?;
    let sync = GmailSync::new(Arc::new(GmailApi::new(auth)), cx.store.clone());
    let mut failed = false;
    for account in accounts {
        let address = &account.address;
        if !account.is_active() && only.is_none() {
            eprintln!("{address}: must sign in again; skipped");
            continue;
        }
        let result = match sync.sync(address, progress(address)).await {
            Ok(r) => {
                eprintln!(
                    "\r{address}: {} sync done: {} new (headers), {} downloaded, {} updated, {} removed",
                    if r.full { "full" } else { "incremental" },
                    r.headers,
                    r.fetched,
                    r.relabelled,
                    r.removed
                );
                if backfill {
                    sync.backfill(address, progress(address)).await.map(|r| {
                        if r.fetched > 0 {
                            eprintln!("\r{address}: downloaded {} messages", r.fetched);
                        }
                    })
                } else {
                    Ok(())
                }
            }
            Err(e) => Err(e),
        };
        if let Err(e) = result {
            eprintln!("\r{address}: {e}");
            failed = true;
        }
    }
    if failed {
        Err("some accounts did not sync".into())
    } else {
        Ok(())
    }
}

/// A progress line for one account, rewritten in place.
fn progress(address: &EmailAddress) -> ProgressFn {
    let label = address.to_string();
    Arc::new(move |p: Progress| {
        match p {
            Progress::Listing { found } => eprint!("\r{label}: listing… {found} messages"),
            Progress::Working { phase, done, total } => {
                let what = match phase {
                    Phase::Headers => "headers",
                    Phase::Changes => "changes",
                    Phase::Bodies => "downloading",
                };
                eprint!("\r{label}: {what} {done}/{total}          ")
            }
        }
        let _ = std::io::stderr().flush();
    })
}

fn search(cx: &Context, query: &str, limit: usize) -> Result<()> {
    let commands = postrinch_commands::Commands::new(cx.store.clone());
    let results = commands.search(query, limit)?;
    for row in &results.threads {
        let t = &row.summary;
        let when = jiff::Timestamp::from_millisecond(t.latest.as_millis())
            .map(|t| t.strftime("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let people: Vec<String> = t.participants.iter().map(|m| m.display_name()).collect();
        println!(
            "{when}\t{}\t{}\t{}\t{}",
            t.account,
            t.id.0,
            people.join(", "),
            t.subject
        );
    }
    if results.more {
        println!("(more than {limit})");
    }
    if let Some(remote) = &results.remote {
        for (account, n) in &remote.accounts {
            eprintln!(
                "{account}: {n} messages are not downloaded yet; their bodies were not searched"
            );
        }
    }
    Ok(())
}

fn show(cx: &Context, account: &EmailAddress, id: String) -> Result<()> {
    let m = cx
        .store
        .message(account, &ProviderMessageId(id.clone()))?
        .ok_or_else(|| format!("no message {id} in {account}"))?;
    let list = |boxes: &[postrinch_model::Mailbox]| {
        boxes
            .iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    println!("From:    {}", list(&m.headers.from));
    println!("To:      {}", list(&m.headers.to));
    if !m.headers.cc.is_empty() {
        println!("Cc:      {}", list(&m.headers.cc));
    }
    println!("Subject: {}", m.subject());
    println!("Labels:  {}", m.labels.join(", "));
    println!("Thread:  {}", m.thread.0);
    for a in m.attachments.iter().filter(|a| !a.inline) {
        println!(
            "Attached: {} ({}, {} bytes)",
            a.filename.as_deref().unwrap_or("unnamed"),
            a.mime_type,
            a.size
        );
    }
    println!("\n{}", m.body_text);
    Ok(())
}
