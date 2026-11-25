//! The add-account dialog's state, apart from the window (docs/imap.md,
//! "Adding an account", after design/mockups/add-imap-account.html): its
//! steps, the form discovery fills in, what the form must hold before it
//! is checked, and what a failed check or sign-in says and where it
//! leaves the person. The slow work is in `add_account_flow`, and the
//! window's part in `add_account_run` and `ui::add_account`.

use postrinch_discover::{Candidate, Discovery, HostedBy, Source};
use postrinch_model::{EmailAddress, FolderRoles, ImapSettings, Security, Server, SignInMethod};
use postrinch_sync::imap::{ImapSecret, NewImapAccount, OAuthProvider};

/// Which tile of the chooser the person took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Any IMAP and SMTP account; discovery says which.
    Other,
    /// Outlook.com, Hotmail or Microsoft 365: Microsoft's sign-in.
    Microsoft,
}

/// Where the dialog is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Which kind of account.
    Choose,
    /// The address and name.
    Address,
    /// Discovery running.
    LookingUp,
    /// What was found, and the password or the browser sign-in.
    Found,
    /// The domain's mail is Google's: better added as a Google account.
    Workspace,
    /// The servers, by hand.
    Servers,
    /// Waiting for the browser sign-in.
    Browser,
    /// Both servers being checked.
    Checking,
}

/// One server as the form holds it: the port as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerForm {
    pub host: String,
    pub port: String,
    pub security: Security,
    pub username: String,
}

impl ServerForm {
    pub fn from_server(server: &Server) -> Self {
        Self {
            host: server.host.clone(),
            port: server.port.to_string(),
            security: server.security,
            username: server.username.clone(),
        }
    }

    /// The server, or why the form does not name one. `what` is
    /// "incoming" or "outgoing".
    pub fn server(&self, what: &str) -> Result<Server, String> {
        let host = self.host.trim();
        if host.is_empty() || host.contains(char::is_whitespace) {
            return Err(format!("Give the {what} server's name."));
        }
        let port = match self.port.trim().parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => return Err(format!("The {what} port is a number from 1 to 65535.")),
        };
        let username = self.username.trim();
        if username.is_empty() {
            return Err(format!("Give the {what} server's user name."));
        }
        Ok(Server {
            host: host.to_owned(),
            port,
            security: self.security,
            username: username.to_owned(),
        })
    }

    /// "imap.example.org · 993 · TLS", as the found box shows it.
    pub fn line(&self) -> String {
        format!(
            "{} · {} · {}",
            self.host.trim(),
            self.port.trim(),
            security_name(self.security)
        )
    }
}

pub fn security_name(security: Security) -> &'static str {
    match security {
        Security::Tls => "TLS",
        Security::StartTls => "STARTTLS",
    }
}

/// How the account will sign in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    Password,
    Browser(OAuthProvider),
}

/// What discovery found, as the dialog tells it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Found {
    /// "Fastmail"; none when the source gives no name.
    pub provider: Option<String>,
    /// Where the servers came from; none when they are guesses.
    pub source: Option<&'static str>,
    /// The provider's page for making an app password, when it needs one.
    pub app_password_url: Option<String>,
    /// The browser sign-in Postrinch has for the provider, if any.
    pub oauth: Option<OAuthProvider>,
    /// The provider still takes passwords (app passwords) from mail apps.
    pub takes_passwords: bool,
    /// The domain's mail is Google's (Gmail or Workspace).
    pub google: bool,
    /// The servers are outside the domain and nobody vouches for them:
    /// the person must say they trust them before any password goes.
    pub confirm_servers: bool,
}

/// Everything the dialog holds. Not `Debug`: it holds the password.
#[derive(Clone, PartialEq, Eq)]
pub struct AddForm {
    pub kind: Kind,
    pub address: String,
    pub name: String,
    pub imap: ServerForm,
    pub smtp: ServerForm,
    pub way: Way,
    pub password: String,
    pub separate_smtp: bool,
    pub smtp_password: String,
    /// The person trusts servers discovery could not vouch for.
    pub trusted: bool,
    pub found: Found,
}

impl AddForm {
    pub fn new(kind: Kind) -> Self {
        let blank = |port: u16, security| ServerForm {
            host: String::new(),
            port: port.to_string(),
            security,
            username: String::new(),
        };
        Self {
            kind,
            address: String::new(),
            name: String::new(),
            imap: blank(993, Security::Tls),
            smtp: blank(465, Security::Tls),
            way: Way::Password,
            password: String::new(),
            separate_smtp: false,
            smtp_password: String::new(),
            trusted: false,
            found: Found::default(),
        }
    }

    /// The address, or why it is not one.
    pub fn parsed_address(&self) -> Result<EmailAddress, String> {
        EmailAddress::parse(self.address.trim())
            .map_err(|_| format!("“{}” is not an email address.", self.address.trim()))
    }

    /// Takes in what discovery found for the address: the servers (or
    /// guesses when it found none), how to sign in, and what to say.
    /// `oauth_ready` says which browser sign-ins this copy has set up.
    pub fn take_discovery(
        &mut self,
        discovery: &Discovery,
        oauth_ready: impl Fn(OAuthProvider) -> bool,
    ) {
        let address = self.address.trim().to_owned();
        let domain = address.rsplit_once('@').map_or("", |(_, d)| d).to_owned();
        let microsoft =
            self.kind == Kind::Microsoft || discovery.hosted_by == Some(HostedBy::Microsoft);
        let candidate = |c: &Candidate| ServerForm {
            host: c.host.clone(),
            port: c.port.to_string(),
            security: match c.security {
                postrinch_discover::Security::Tls => Security::Tls,
                postrinch_discover::Security::StartTls => Security::StartTls,
            },
            username: c.username.for_address(&address),
        };
        let guess = |host: String, port: u16, security| ServerForm {
            host,
            port: port.to_string(),
            security,
            username: address.clone(),
        };
        self.imap = match discovery.imap.first() {
            Some(c) => candidate(c),
            None if microsoft => guess("outlook.office365.com".into(), 993, Security::Tls),
            None => guess(format!("mail.{domain}"), 993, Security::Tls),
        };
        self.smtp = match discovery.smtp.first() {
            Some(c) => candidate(c),
            None if microsoft => guess("smtp.office365.com".into(), 587, Security::StartTls),
            None => guess(format!("mail.{domain}"), 587, Security::StartTls),
        };
        let oauth = if self.kind == Kind::Microsoft {
            Some(OAuthProvider::Microsoft)
        } else {
            OAuthProvider::for_discovery(discovery)
        }
        .filter(|p| oauth_ready(*p));
        let takes_passwords = !microsoft && !discovery.oauth2_only;
        self.found = Found {
            provider: discovery
                .provider_name
                .clone()
                .or_else(|| microsoft.then(|| "Microsoft".to_owned())),
            source: source_line(discovery.source),
            app_password_url: discovery.app_password_url.clone(),
            oauth,
            takes_passwords,
            google: discovery.hosted_by == Some(HostedBy::Google),
            confirm_servers: discovery.confirm_servers,
        };
        // The browser first, wherever Postrinch has one for the provider.
        self.way = match oauth {
            Some(provider) => Way::Browser(provider),
            None => Way::Password,
        };
        self.trusted = false;
    }

    /// The step after discovery: Google's mail offers Google's own
    /// sign-in first; nothing found opens the servers.
    pub fn step_after_discovery(&self) -> Step {
        if self.found.google && self.kind == Kind::Other {
            Step::Workspace
        } else if self.found.source.is_none() {
            Step::Servers
        } else {
            Step::Found
        }
    }

    /// Why the account cannot be signed in to at all, when it cannot: a
    /// provider that takes only its browser sign-in, which this copy of
    /// Postrinch does not have set up.
    pub fn no_way_in(&self) -> Option<String> {
        if self.found.oauth.is_some() || self.found.takes_passwords {
            return None;
        }
        let who = self.found.provider.as_deref().unwrap_or("This provider");
        Some(format!(
            "{who} signs mail apps in only in the browser, and that sign-in is not set up in this copy of Postrinch."
        ))
    }

    /// The account to add, its settings and how it signs in (the secret
    /// for a password), or what the person must fix first.
    pub fn to_add(&self) -> Result<(NewImapAccount, Option<ImapSecret>), String> {
        let address = self.parsed_address()?;
        let imap = self.imap.server("incoming")?;
        let smtp = self.smtp.server("outgoing")?;
        if self.found.confirm_servers && !self.trusted {
            return Err(
                "Say that you trust these servers before Postrinch signs in to them.".into(),
            );
        }
        let (sign_in, secret) = match self.way {
            Way::Browser(provider) => (provider.sign_in_method(), None),
            Way::Password => {
                let (method, secret) = self.password_secret()?;
                (method, Some(secret))
            }
        };
        let name = self.name.trim();
        let new = NewImapAccount {
            address,
            display_name: (!name.is_empty()).then(|| name.to_owned()),
            settings: ImapSettings {
                imap,
                smtp,
                sign_in,
                roles: FolderRoles::default(),
                server_files_sent: true,
                server_files_sent_chosen: false,
            },
        };
        Ok((new, secret))
    }

    /// The password (and SMTP's own) as a secret, or why there is none.
    pub fn password_secret(&self) -> Result<(SignInMethod, ImapSecret), String> {
        if self.password.is_empty() {
            return Err("Give the password.".into());
        }
        if !self.separate_smtp {
            return Ok((
                SignInMethod::Password,
                ImapSecret::password(self.password.clone()),
            ));
        }
        if self.smtp_password.is_empty() {
            return Err("Give the outgoing server's password.".into());
        }
        Ok((
            SignInMethod::SeparateSmtpPassword,
            ImapSecret::Password {
                password: self.password.clone(),
                smtp_password: Some(self.smtp_password.clone()),
            },
        ))
    }

    /// The address as typed. What discovery found was for the address
    /// as it was, so a changed address forgets it all: the servers, how
    /// to sign in, what was said about the provider, the trust given to
    /// its servers and the passwords typed for it. Only the kind, the
    /// name and the address itself stay.
    pub fn set_address(&mut self, address: String) {
        if address.trim() != self.address.trim() {
            *self = Self {
                address,
                name: std::mem::take(&mut self.name),
                ..Self::new(self.kind)
            };
        } else {
            self.address = address;
        }
    }

    /// "Set up servers by hand" before discovery has filled anything in:
    /// the usual names, guessed from the address.
    pub fn guess_servers(&mut self) {
        if self.parsed_address().is_ok() && self.imap.host.is_empty() {
            self.take_discovery(&Discovery::nothing(), |_| false);
        }
    }

    /// "Use IMAP anyway" on Google's mail: Google's browser sign-in over
    /// IMAP when this copy has it, else an app password.
    pub fn use_imap_anyway(&mut self) {
        self.way = match self.found.oauth {
            Some(provider) => Way::Browser(provider),
            None => Way::Password,
        };
    }

    /// Forgets the passwords typed (after the account is added, or the
    /// dialog closes).
    pub fn forget_passwords(&mut self) {
        self.password.clear();
        self.smtp_password.clear();
    }
}

/// Where discovery's answer came from, as the found box says it.
fn source_line(source: Source) -> Option<&'static str> {
    match source {
        Source::DomainAutoconfig => Some("From the domain's own settings"),
        Source::Ispdb => Some("From Thunderbird's list of providers"),
        Source::IspdbViaMx => {
            Some("From Thunderbird's list of providers, by the domain's mail server")
        }
        Source::Srv => Some("From the domain's DNS records"),
        Source::Guess => Some("Found by trying the usual server names"),
        Source::Nothing => None,
    }
}

#[cfg(test)]
#[path = "add_account_tests.rs"]
mod tests;
