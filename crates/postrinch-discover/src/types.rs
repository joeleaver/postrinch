//! What discovery answers with, and the progress it reports on the way.

use serde::{Deserialize, Serialize};

/// Where a [`Discovery`] came from: the step that answered first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Source {
    /// The domain's own autoconfig file (`autoconfig.<domain>` or its
    /// `.well-known` copy).
    DomainAutoconfig,
    /// Thunderbird's list of providers (the ISPDB), looked up by the
    /// address's domain.
    Ispdb,
    /// The ISPDB, looked up by the domain of the address's mail exchanger
    /// (MX record): the domain's mail is hosted by that provider.
    IspdbViaMx,
    /// The domain's DNS SRV records (RFC 6186 and RFC 8314).
    Srv,
    /// Common server names (`imap.`, `smtp.`, `mail.`), each probed.
    Guess,
    /// Nothing answered; the servers have to be entered by hand.
    Nothing,
}

/// How a connection to a server is secured. Plain-text connections are
/// never offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Security {
    /// TLS from the first byte (implicit TLS: IMAP on 993, SMTP on 465).
    Tls,
    /// A plain connection upgraded with STARTTLS before signing in.
    StartTls,
}

/// The user name to sign in with.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Username {
    /// The whole email address.
    Address,
    /// The part of the address before the `@`.
    LocalPart,
    /// A user name given by the provider, placeholders already filled in.
    Literal(String),
}

impl Username {
    /// The user name for `address`.
    pub fn for_address(&self, address: &str) -> String {
        match self {
            Username::Address => address.to_owned(),
            Username::LocalPart => address
                .rsplit_once('@')
                .map_or(address, |(local, _)| local)
                .to_owned(),
            Username::Literal(name) => name.clone(),
        }
    }
}

/// One server to try, IMAP or SMTP.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Candidate {
    /// The server's host name, lower case, without a trailing dot.
    pub host: String,
    /// The TCP port.
    pub port: u16,
    /// How the connection is secured.
    pub security: Security,
    /// The user name to sign in with.
    pub username: Username,
}

/// A big mail host that has a better way in than IMAP, found from the
/// domain's MX records or from the servers found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HostedBy {
    /// Gmail or Google Workspace: better added as a Google account.
    Google,
    /// Outlook.com or Microsoft 365.
    Microsoft,
}

/// What discovery found for an address.
///
/// `imap` and `smtp` are in order of preference: TLS before STARTTLS, then
/// in the order the source gave them. Either can be empty, and both are
/// when [`Source::Nothing`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    /// The step that answered.
    pub source: Source,
    /// The provider's name, when the source gives one ("Fastmail").
    pub provider_name: Option<String>,
    /// Incoming servers, best first.
    pub imap: Vec<Candidate>,
    /// Outgoing servers, best first.
    pub smtp: Vec<Candidate>,
    /// Set when the domain's mail is hosted by Google or Microsoft.
    pub hosted_by: Option<HostedBy>,
    /// The provider's page for making an app password, when the provider
    /// is known to need one for mail apps.
    pub app_password_url: Option<String>,
    /// The provider's page about setting up a mail app (the autoconfig
    /// file's `<documentation url>`, or `<instruction url>`).
    pub documentation_url: Option<String>,
    /// The provider's page for turning IMAP access on, when it is off by
    /// default (the autoconfig file's `<enable visiturl>`).
    pub enable_url: Option<String>,
    /// The provider signs mail apps in with OAuth2 only: its file lists no
    /// other way for IMAP, and it is not one Postrinch knows takes app
    /// passwords (see `app_password_url`).
    pub oauth2_only: bool,
}

impl Discovery {
    /// A discovery that found nothing.
    pub fn nothing() -> Self {
        Discovery {
            source: Source::Nothing,
            provider_name: None,
            imap: Vec::new(),
            smtp: Vec::new(),
            hosted_by: None,
            app_password_url: None,
            documentation_url: None,
            enable_url: None,
            oauth2_only: false,
        }
    }
}

/// The steps discovery reports, as the "Looking up" screen lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Step {
    /// The domain's own settings (its autoconfig files).
    DomainSettings,
    /// Thunderbird's list of providers (the ISPDB).
    ProviderList,
    /// The domain's DNS records: MX, then SRV.
    DnsRecords,
    /// Common server names, probed.
    CommonNames,
}

/// Where a step is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StepState {
    /// The step has started.
    Running,
    /// The step answered; discovery stops here.
    Found,
    /// The step found nothing (or could not be reached); discovery goes on.
    NothingFound,
}

/// One progress report: a step and where it is. Steps after the one that
/// answers are never reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Progress {
    /// The step.
    pub step: Step,
    /// Where it is.
    pub state: StepState,
}
