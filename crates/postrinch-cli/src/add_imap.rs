//! `postrinch-cli add-imap`: add an IMAP account (or change one's
//! servers or password). The servers come from the options, or from
//! discovery when none are given; the password from standard input or a
//! prompt that does not echo it, never from an argument, which other
//! users of the machine can see; or the account signs in in the browser
//! with `--oauth`. Both servers are checked before anything is kept.

use std::io::{BufRead, IsTerminal};

use postrinch_discover::{Candidate, Discoverer, Discovery};
use postrinch_model::{EmailAddress, FolderRoles, ImapSettings, Security, Server, SignInMethod};
use postrinch_sync::imap::{
    Added, ImapOAuth, ImapSecret, ImapSync, NewImapAccount, NewSignIn, OAuthProvider,
};

use crate::commands::Result;

#[derive(clap::Args)]
pub struct AddImapArgs {
    /// The account's address.
    address: String,
    /// Your name, as mail you send shows it.
    #[arg(long)]
    name: Option<String>,
    /// The IMAP server. Without it and --smtp-host, discovery finds both.
    #[arg(long, requires = "smtp_host")]
    imap_host: Option<String>,
    /// IMAP's port: 993 with TLS, 143 with STARTTLS, unless given.
    #[arg(long)]
    imap_port: Option<u16>,
    #[arg(long, value_enum, default_value_t = SecurityArg::Tls)]
    imap_security: SecurityArg,
    /// The SMTP server.
    #[arg(long, requires = "imap_host")]
    smtp_host: Option<String>,
    /// SMTP's port: 465 with TLS, 587 with STARTTLS, unless given.
    #[arg(long)]
    smtp_port: Option<u16>,
    #[arg(long, value_enum, default_value_t = SecurityArg::Tls)]
    smtp_security: SecurityArg,
    /// The user name for both servers; the address unless given.
    #[arg(long)]
    username: Option<String>,
    /// Read the password from standard input (its first line) rather than
    /// asking for it.
    #[arg(long, conflicts_with = "oauth")]
    password_stdin: bool,
    /// SMTP has a password of its own: asked for (or read) after IMAP's.
    #[arg(long, conflicts_with = "oauth")]
    separate_smtp_password: bool,
    /// Sign in in the browser with this provider instead of a password.
    #[arg(long, value_enum)]
    oauth: Option<OAuthArg>,
    /// Use servers discovery found outside the address's domain, which
    /// nobody vouches for; without it they are shown and not used.
    #[arg(long)]
    accept_servers: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SecurityArg {
    Tls,
    Starttls,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum OAuthArg {
    Google,
    Microsoft,
}

impl From<OAuthArg> for OAuthProvider {
    fn from(arg: OAuthArg) -> Self {
        match arg {
            OAuthArg::Google => OAuthProvider::Google,
            OAuthArg::Microsoft => OAuthProvider::Microsoft,
        }
    }
}

/// Where the password comes from: asked for, or given (standard input,
/// or a test).
pub trait Passwords {
    /// The password for `what` ("IMAP and SMTP", "SMTP").
    fn password(&mut self, what: &str) -> Result<String>;
}

/// Standard input with --password-stdin, else a prompt that does not echo.
pub struct Terminal {
    pub from_stdin: bool,
}

impl Passwords for Terminal {
    fn password(&mut self, what: &str) -> Result<String> {
        let password = if self.from_stdin || !std::io::stdin().is_terminal() {
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line)?;
            line.trim_end_matches(['\r', '\n']).to_owned()
        } else {
            rpassword::prompt_password(format!("Password for {what}: "))?
        };
        if password.is_empty() {
            return Err(format!("no password given for {what}").into());
        }
        Ok(password)
    }
}

impl AddImapArgs {
    pub fn password_stdin(&self) -> bool {
        self.password_stdin
    }
}

/// Adds the account: servers from the options or discovery, then the
/// sign-in, then the check, through [`ImapSync::add_account`].
pub async fn add_imap(
    sync: &ImapSync,
    oauth: &ImapOAuth,
    args: AddImapArgs,
    passwords: &mut dyn Passwords,
) -> Result<()> {
    let address = EmailAddress::parse(&args.address)?;
    let mut settings = match &args.imap_host {
        Some(_) => given_settings(&args, &address),
        None => discovered_settings(&args, &address).await?,
    };
    let sign_in = match args.oauth {
        Some(provider) => NewSignIn::OAuth {
            provider: provider.into(),
            open_browser: Box::new(|url: &str| {
                eprintln!("Opening your browser to sign in. If it does not open, visit:\n{url}\n");
                open::that_detached(url)
            }),
            // Ctrl-C ends the command, and the sign-in with it.
            cancel: None,
            on_grant: None,
        },
        None if args.separate_smtp_password => {
            settings.sign_in = SignInMethod::SeparateSmtpPassword;
            NewSignIn::Password(ImapSecret::Password {
                password: passwords.password("IMAP")?,
                smtp_password: Some(passwords.password("SMTP")?),
            })
        }
        None => {
            settings.sign_in = SignInMethod::Password;
            NewSignIn::Password(ImapSecret::password(passwords.password("IMAP and SMTP")?))
        }
    };
    let new = NewImapAccount {
        address: address.clone(),
        display_name: args.name.clone(),
        settings,
    };
    match sync.add_account(oauth, new, sign_in).await? {
        Added::Account(account) => {
            println!(
                "Added {}. `postrinch-cli sync {}` fetches its mail.",
                account.address, account.address
            );
            Ok(())
        }
        Added::CheckFailed(check) => {
            Err(format!("{address} was not added; nothing was kept.\n{check}").into())
        }
    }
}

/// The servers the options name.
fn given_settings(args: &AddImapArgs, address: &EmailAddress) -> ImapSettings {
    let username = args.username.clone().unwrap_or_else(|| address.to_string());
    let server = |host: &Option<String>, port: Option<u16>, security, ports: (u16, u16)| {
        let security = security_of(security);
        Server {
            host: host.clone().unwrap_or_default(),
            port: port.unwrap_or(match security {
                Security::Tls => ports.0,
                Security::StartTls => ports.1,
            }),
            security,
            username: username.clone(),
        }
    };
    settings(
        server(
            &args.imap_host,
            args.imap_port,
            args.imap_security,
            (993, 143),
        ),
        server(
            &args.smtp_host,
            args.smtp_port,
            args.smtp_security,
            (465, 587),
        ),
    )
}

/// The servers discovery finds for the address, best first.
async fn discovered_settings(args: &AddImapArgs, address: &EmailAddress) -> Result<ImapSettings> {
    eprintln!("Looking up {address}'s mail servers…");
    let discovery =
        postrinch_discover::discover(&address.to_string(), &Discoverer::new()?, |_| {}).await?;
    settings_from_discovery(args, address, &discovery)
}

fn settings_from_discovery(
    args: &AddImapArgs,
    address: &EmailAddress,
    discovery: &Discovery,
) -> Result<ImapSettings> {
    let (Some(imap), Some(smtp)) = (discovery.imap.first(), discovery.smtp.first()) else {
        return Err(format!(
            "no servers found for {address}: give them with --imap-host and --smtp-host"
        )
        .into());
    };
    let server = |c: &Candidate| Server {
        host: c.host.clone(),
        port: c.port,
        security: match c.security {
            postrinch_discover::Security::Tls => Security::Tls,
            postrinch_discover::Security::StartTls => Security::StartTls,
        },
        username: args
            .username
            .clone()
            .unwrap_or_else(|| c.username.for_address(&address.to_string())),
    };
    let (imap, smtp) = (server(imap), server(smtp));
    eprintln!(
        "IMAP {}:{} ({:?}), SMTP {}:{} ({:?}), user name {}",
        imap.host, imap.port, imap.security, smtp.host, smtp.port, smtp.security, imap.username
    );
    if discovery.confirm_servers && !args.accept_servers {
        return Err(
            "these servers are outside the address's domain and nobody vouches for them: \
             pass --accept-servers to use them, or give --imap-host and --smtp-host"
                .into(),
        );
    }
    if let Some(provider) = postrinch_sync::imap::OAuthProvider::for_discovery(discovery)
        && args.oauth.is_none()
    {
        eprintln!(
            "{address}'s mail is {}'s: if a password is refused, try --oauth {}.",
            provider.name(),
            provider.id()
        );
    }
    Ok(settings(imap, smtp))
}

fn settings(imap: Server, smtp: Server) -> ImapSettings {
    ImapSettings {
        imap,
        smtp,
        sign_in: SignInMethod::Password,
        roles: FolderRoles::default(),
        server_files_sent: true,
        server_files_sent_chosen: false,
    }
}

fn security_of(arg: SecurityArg) -> Security {
    match arg {
        SecurityArg::Tls => Security::Tls,
        SecurityArg::Starttls => Security::StartTls,
    }
}

#[cfg(test)]
#[path = "add_imap_tests.rs"]
mod tests;
