use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use postrinch_model::{AccountState, EmailAddress, Provider, ProviderMessageId};
use postrinch_store::Store;
use postrinch_sync::google::{ClientConfig, GmailApi, GmailSync, GoogleAuth, Keychain, Progress};

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
        Command::Sync { address } => sync(&cx, address.as_deref().map(parse).transpose()?).await,
        Command::Search { query, limit } => search(&cx, &query, limit),
        Command::Show {
            address,
            message_id,
        } => show(&cx, &parse(&address)?, message_id),
    }
}

fn parse(address: &str) -> Result<EmailAddress> {
    Ok(EmailAddress::parse(address)?)
}

fn accounts(cx: &Context) -> Result<()> {
    let accounts = cx.store.accounts()?;
    if accounts.is_empty() {
        println!("No accounts. Add one with `postrinch add-google`.");
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
    let account = auth
        .sign_in(address.as_ref(), |url| {
            eprintln!("Opening your browser to sign in. If it does not open, visit:\n{url}\n");
            open::that_detached(url)
        })
        .await?;
    cx.store.put_account(&account)?;
    println!("Signed in as {}.", account.address);
    Ok(())
}

async fn remove_account(cx: &Context, address: &EmailAddress) -> Result<()> {
    let account = cx
        .store
        .account(address)?
        .ok_or_else(|| format!("no account {address}"))?;
    if account.provider == Provider::Google {
        cx.auth()?.sign_out(address).await?;
    }
    cx.store.remove_account(address)?;
    let removed = cx.store.collect_garbage(std::time::Duration::ZERO)?;
    println!("Removed {address} and {removed} stored messages' raw copies.");
    Ok(())
}

async fn sync(cx: &Context, only: Option<EmailAddress>) -> Result<()> {
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
            None => "no Google accounts; add one with `postrinch add-google`".into(),
        });
    }
    let auth = cx.auth()?;
    let sync = GmailSync::new(Arc::new(GmailApi::new(auth)), cx.store.clone());
    let mut failed = false;
    for account in accounts {
        if !account.is_active() && only.is_none() {
            eprintln!("{}: must sign in again; skipped", account.address);
            continue;
        }
        let label = account.address.to_string();
        let progress = Arc::new(move |p: Progress| {
            match p {
                Progress::Listing { found } => eprint!("\r{label}: listing… {found} messages"),
                Progress::Applying { done, total } => {
                    eprint!("\r{label}: {done}/{total}          ")
                }
            }
            let _ = std::io::stderr().flush();
        });
        match sync.sync(&account.address, progress).await {
            Ok(r) => eprintln!(
                "\r{}: {} sync done: {} fetched, {} updated, {} removed",
                account.address,
                if r.full { "full" } else { "incremental" },
                r.fetched,
                r.relabelled,
                r.removed
            ),
            Err(e) => {
                eprintln!("\r{}: {e}", account.address);
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

fn search(cx: &Context, query: &str, limit: usize) -> Result<()> {
    for hit in cx.store.search(query, limit)? {
        let m = &hit.message;
        let from = m
            .headers
            .from
            .first()
            .map(|f| f.display_name())
            .unwrap_or_default();
        println!(
            "{:.2}\t{}\t{}\t{}\t{}",
            hit.score,
            m.account,
            m.id.0,
            from,
            m.subject()
        );
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
