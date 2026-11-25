//! The accounts in a profile's prefs: each `mail.account.<key>` with its
//! incoming server (`mail.server.<key>.*`), its first identity
//! (`mail.identity.<key>.*`) and that identity's outgoing server
//! (`mail.smtpserver.<key>.*`). Settings only: Thunderbird keeps no
//! passwords in its prefs, and nothing here looks for any.

use crate::prefs::Prefs;

/// What kind of account Thunderbird has, from its server's `type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountKind {
    Imap,
    Pop,
    /// Newsgroups (`nntp`).
    News,
    /// Feeds (`rss`).
    Feeds,
    /// Local Folders (`none`).
    LocalFolders,
    /// Any other type, as Thunderbird names it (`ews`, `movemail`…).
    Other(String),
}

impl AccountKind {
    fn from_type(kind: &str) -> Self {
        match kind {
            "imap" => Self::Imap,
            "pop3" => Self::Pop,
            "nntp" => Self::News,
            "rss" => Self::Feeds,
            "none" => Self::LocalFolders,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// How a server's connection is secured, from `socketType` (incoming)
/// or `try_ssl` (outgoing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// None (0), or STARTTLS when the server offers it (1).
    Plain,
    /// STARTTLS, required (2).
    StartTls,
    /// TLS from the start (3).
    Tls,
}

impl Security {
    fn from_code(code: Option<i32>) -> Self {
        match code {
            Some(2) => Self::StartTls,
            Some(3) => Self::Tls,
            _ => Self::Plain,
        }
    }
}

/// How a server signs in, from its `authMethod`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignIn {
    /// A password, sent as it is or hashed (2, 3, 4, 8, 9; 3 when unset).
    Password,
    /// OAuth 2 in the browser (10).
    OAuth2,
    /// No sign-in (1).
    None,
    /// Kerberos (5), NTLM (6) or a TLS certificate (7), or a number
    /// Thunderbird does not have.
    Other(i32),
}

impl SignIn {
    fn from_code(code: Option<i32>) -> Self {
        match code {
            None | Some(0 | 2 | 3 | 4 | 8 | 9) => Self::Password,
            Some(10) => Self::OAuth2,
            Some(1) => Self::None,
            Some(other) => Self::Other(other),
        }
    }
}

/// A server, as Thunderbird connects to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub host: String,
    pub port: u16,
    pub security: Security,
    /// None when Thunderbird has no user name for it.
    pub username: Option<String>,
    pub sign_in: SignIn,
}

/// An identity's signature, when it is written in the settings (not a
/// file it points to).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub text: String,
    /// The text is HTML.
    pub html: bool,
}

/// The person an account sends as.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub address: Option<String>,
    pub name: Option<String>,
    pub signature: Option<Signature>,
}

/// One account in a profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// Thunderbird's key for it (`account1`).
    pub key: String,
    pub kind: AccountKind,
    /// The account's name in Thunderbird's folder pane.
    pub label: Option<String>,
    /// The incoming server; none when its settings have no host.
    pub incoming: Option<Server>,
    /// The first identity, the one Thunderbird sends as by default.
    pub identity: Option<Identity>,
    /// The identity's outgoing server, or the default one.
    pub outgoing: Option<Server>,
}

/// A text pref, unless it is missing or empty.
fn text(prefs: &Prefs, name: &str) -> Option<String> {
    prefs
        .string(name)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// A comma-separated list pref (`account1,account2`).
fn list(prefs: &Prefs, name: &str) -> Vec<String> {
    prefs.string(name).map_or_else(Vec::new, |s| {
        s.split(',')
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(str::to_owned)
            .collect()
    })
}

/// The keys of the accounts, in Thunderbird's order: the account
/// manager's list, or (without one) every `mail.account.<key>.server`.
fn account_keys(prefs: &Prefs) -> Vec<String> {
    let listed = list(prefs, "mail.accountmanager.accounts");
    if !listed.is_empty() {
        return dedup(listed);
    }
    let mut keys: Vec<String> = prefs
        .names()
        .filter_map(|n| n.strip_prefix("mail.account.")?.strip_suffix(".server"))
        .filter(|k| !k.contains('.'))
        .map(str::to_owned)
        .collect();
    keys.sort_by_key(|k| natural(k));
    keys
}

fn dedup(keys: Vec<String>) -> Vec<String> {
    let mut seen = Vec::new();
    for key in keys {
        if !seen.contains(&key) {
            seen.push(key);
        }
    }
    seen
}

/// `account10` after `account9`.
fn natural(key: &str) -> (String, u64) {
    let digits = key.len() - key.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (stem, n) = key.split_at(key.len() - digits);
    (stem.to_owned(), n.parse().unwrap_or(0))
}

/// The port pref, or the protocol's usual one when it is unset (or 0, or
/// not a port).
fn port(prefs: &Prefs, name: &str, default: u16) -> u16 {
    prefs
        .int(name)
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p > 0)
        .unwrap_or(default)
}

/// The incoming server `key`. The host and user name are the `real…`
/// ones when set: Thunderbird keeps `hostname` and `userName` as they
/// were first given (they name the account's folders) and writes a
/// change made later in its settings to `realhostname`/`realuserName`.
fn incoming(prefs: &Prefs, key: &str, kind: &AccountKind) -> Option<Server> {
    let p = |field: &str| format!("mail.server.{key}.{field}");
    let host = text(prefs, &p("realhostname")).or_else(|| text(prefs, &p("hostname")))?;
    let security = Security::from_code(prefs.int(&p("socketType")));
    let usual = match (kind, security) {
        (AccountKind::Pop, Security::Tls) => 995,
        (AccountKind::Pop, _) => 110,
        (AccountKind::News, Security::Tls) => 563,
        (AccountKind::News, _) => 119,
        (_, Security::Tls) => 993,
        _ => 143,
    };
    Some(Server {
        host,
        port: port(prefs, &p("port"), usual),
        security,
        username: text(prefs, &p("realuserName")).or_else(|| text(prefs, &p("userName"))),
        sign_in: SignIn::from_code(prefs.int(&p("authMethod"))),
    })
}

/// The outgoing server `key`. Thunderbird's own usual ports: 465 for
/// TLS, 25 otherwise.
fn outgoing(prefs: &Prefs, key: &str) -> Option<Server> {
    let p = |field: &str| format!("mail.smtpserver.{key}.{field}");
    let host = text(prefs, &p("hostname"))?;
    let security = Security::from_code(prefs.int(&p("try_ssl")));
    let usual = if security == Security::Tls { 465 } else { 25 };
    Some(Server {
        host,
        port: port(prefs, &p("port"), usual),
        security,
        username: text(prefs, &p("username")),
        sign_in: SignIn::from_code(prefs.int(&p("authMethod"))),
    })
}

fn identity(prefs: &Prefs, key: &str) -> Identity {
    let p = |field: &str| format!("mail.identity.{key}.{field}");
    // A signature read from a file is the file's, which is not read.
    let from_file = prefs.bool(&p("attach_signature")) == Some(true);
    let signature = (!from_file)
        .then(|| prefs.string(&p("htmlSigText")))
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .map(|s| Signature {
            text: s.to_owned(),
            html: prefs.bool(&p("htmlSigFormat")) == Some(true),
        });
    Identity {
        address: text(prefs, &p("useremail")),
        name: text(prefs, &p("fullName")),
        signature,
    }
}

/// Every account in `prefs`, in Thunderbird's order.
pub fn accounts(prefs: &Prefs) -> Vec<Account> {
    let default_smtp = text(prefs, "mail.smtp.defaultserver");
    account_keys(prefs)
        .into_iter()
        .filter_map(|key| {
            let server = text(prefs, &format!("mail.account.{key}.server"))?;
            let kind = AccountKind::from_type(
                &text(prefs, &format!("mail.server.{server}.type")).unwrap_or_default(),
            );
            let identity_key = list(prefs, &format!("mail.account.{key}.identities"))
                .into_iter()
                .next();
            let identity = identity_key.as_deref().map(|id| identity(prefs, id));
            let smtp_key = identity_key
                .as_deref()
                .and_then(|id| text(prefs, &format!("mail.identity.{id}.smtpServer")))
                .or_else(|| default_smtp.clone());
            // Only accounts that send have an outgoing server.
            let sends = matches!(kind, AccountKind::Imap | AccountKind::Pop);
            let outgoing = smtp_key
                .filter(|_| sends)
                .and_then(|smtp| outgoing(prefs, &smtp));
            Some(Account {
                incoming: incoming(prefs, &server, &kind),
                label: text(prefs, &format!("mail.server.{server}.name")),
                key,
                kind,
                identity,
                outgoing,
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "accounts_tests.rs"]
mod tests;
