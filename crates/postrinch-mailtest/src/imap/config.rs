//! How a fake IMAP server is set up: its listeners, extensions and users.

use std::collections::BTreeSet;

/// An IMAP extension the fake can offer or withhold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Extension {
    /// RFC 7162 CONDSTORE: MODSEQ, HIGHESTMODSEQ, CHANGEDSINCE, UNCHANGEDSINCE.
    Condstore,
    /// RFC 7162 QRESYNC: VANISHED, SELECT (QRESYNC ...). Implies ENABLE.
    Qresync,
    /// RFC 6851 MOVE.
    Move,
    /// RFC 4315 UIDPLUS: APPENDUID, COPYUID, UID EXPUNGE.
    Uidplus,
    /// RFC 6154 SPECIAL-USE: `\Sent`, `\Drafts`... on LIST.
    SpecialUse,
    /// RFC 2177 IDLE.
    Idle,
}

impl Extension {
    /// Every extension the fake knows.
    pub const ALL: [Extension; 6] = [
        Extension::Condstore,
        Extension::Qresync,
        Extension::Move,
        Extension::Uidplus,
        Extension::SpecialUse,
        Extension::Idle,
    ];

    /// The capability name advertised for it.
    pub fn capability(self) -> &'static str {
        match self {
            Extension::Condstore => "CONDSTORE",
            Extension::Qresync => "QRESYNC",
            Extension::Move => "MOVE",
            Extension::Uidplus => "UIDPLUS",
            Extension::SpecialUse => "SPECIAL-USE",
            Extension::Idle => "IDLE",
        }
    }
}

/// A user the fake accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserConfig {
    pub(crate) name: String,
    pub(crate) password: Option<String>,
    pub(crate) oauth_token: Option<String>,
}

/// The fake IMAP server's setup. By default it listens with implicit TLS
/// only, offers every extension, and has no users.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImapConfig {
    pub(crate) implicit_tls: bool,
    pub(crate) starttls: bool,
    pub(crate) extensions: BTreeSet<Extension>,
    pub(crate) users: Vec<UserConfig>,
}

impl Default for ImapConfig {
    fn default() -> Self {
        Self {
            implicit_tls: true,
            starttls: false,
            extensions: Extension::ALL.into_iter().collect(),
            users: Vec::new(),
        }
    }
}

impl ImapConfig {
    /// The default setup.
    pub fn new() -> Self {
        Self::default()
    }

    /// Lets `name` sign in with this password (LOGIN, AUTHENTICATE PLAIN).
    /// Each user starts with an empty INBOX.
    pub fn user(mut self, name: &str, password: &str) -> Self {
        self.entry(name).password = Some(password.to_string());
        self
    }

    /// Lets `name` sign in with XOAUTH2 and this bearer token. A user given
    /// only a token cannot sign in with a password.
    pub fn oauth_token(mut self, name: &str, token: &str) -> Self {
        self.entry(name).oauth_token = Some(token.to_string());
        self
    }

    fn entry(&mut self, name: &str) -> &mut UserConfig {
        match self.users.iter().position(|u| u.name == name) {
            Some(i) => &mut self.users[i],
            None => {
                self.users.push(UserConfig {
                    name: name.to_string(),
                    password: None,
                    oauth_token: None,
                });
                self.users.last_mut().expect("just pushed")
            }
        }
    }

    /// Whether to listen with implicit TLS (the 993-style port). On by default.
    pub fn implicit_tls(mut self, on: bool) -> Self {
        self.implicit_tls = on;
        self
    }

    /// Whether to also listen in plain text with STARTTLS (the 143-style
    /// port). Off by default. Before STARTTLS that port refuses to log in.
    pub fn starttls(mut self, on: bool) -> Self {
        self.starttls = on;
        self
    }

    /// Offers exactly these extensions.
    pub fn extensions(mut self, extensions: &[Extension]) -> Self {
        self.extensions = extensions.iter().copied().collect();
        self
    }

    /// Stops offering one extension.
    pub fn without(mut self, extension: Extension) -> Self {
        self.extensions.remove(&extension);
        self
    }

    /// Whether an extension is offered.
    pub fn has(&self, extension: Extension) -> bool {
        self.extensions.contains(&extension)
    }
}
