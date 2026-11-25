//! How a fake IMAP server is set up: its listeners, extensions and users.

use std::collections::BTreeSet;

use crate::sasl::Mechanism;

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
    /// RFC 5258 LIST-EXTENDED: selection options (`LIST (SUBSCRIBED) ...`),
    /// several patterns at once, and `RETURN (...)`. Without it those forms
    /// are BAD; RFC 6154's `SPECIAL-USE` options also need it.
    ListExtended,
    /// RFC 2177 IDLE.
    Idle,
    /// RFC 8474 OBJECTID: each folder's `MAILBOXID`, kept across renames,
    /// in SELECT's and EXAMINE's responses.
    ObjectId,
}

impl Extension {
    /// Every extension the fake knows.
    pub const ALL: [Extension; 8] = [
        Extension::Condstore,
        Extension::Qresync,
        Extension::Move,
        Extension::Uidplus,
        Extension::SpecialUse,
        Extension::ListExtended,
        Extension::Idle,
        Extension::ObjectId,
    ];

    /// The capability name advertised for it.
    pub fn capability(self) -> &'static str {
        match self {
            Extension::Condstore => "CONDSTORE",
            Extension::Qresync => "QRESYNC",
            Extension::Move => "MOVE",
            Extension::Uidplus => "UIDPLUS",
            Extension::SpecialUse => "SPECIAL-USE",
            Extension::ListExtended => "LIST-EXTENDED",
            Extension::Idle => "IDLE",
            Extension::ObjectId => "OBJECTID",
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
/// only, offers every extension and the PLAIN and XOAUTH2 mechanisms, and
/// has no users.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImapConfig {
    pub(crate) implicit_tls: bool,
    pub(crate) starttls: bool,
    pub(crate) offer_starttls: bool,
    pub(crate) extensions: BTreeSet<Extension>,
    pub(crate) mechanisms: BTreeSet<Mechanism>,
    pub(crate) users: Vec<UserConfig>,
    pub(crate) greets: bool,
    pub(crate) bye_greeting: Option<String>,
    pub(crate) injects_after_starttls: bool,
    pub(crate) literal_plus: bool,
    pub(crate) splits_fetches: bool,
    pub(crate) unsolicited_fetches: bool,
    pub(crate) gmail: bool,
}

impl Default for ImapConfig {
    fn default() -> Self {
        Self {
            implicit_tls: true,
            starttls: false,
            offer_starttls: true,
            extensions: Extension::ALL.into_iter().collect(),
            mechanisms: Mechanism::DEFAULT.into_iter().collect(),
            users: Vec::new(),
            greets: true,
            bye_greeting: None,
            injects_after_starttls: false,
            literal_plus: true,
            splits_fetches: false,
            unsolicited_fetches: false,
            gmail: false,
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

    /// Whether the plain-text port offers STARTTLS. On by default. Off, it
    /// is a server with no encryption at all, as misconfigured servers
    /// are: it advertises neither STARTTLS nor LOGINDISABLED and signs
    /// users in over plain text. For testing that clients refuse it.
    pub fn offer_starttls(mut self, on: bool) -> Self {
        self.offer_starttls = on;
        self
    }

    /// Whether the server greets a new connection. On by default. Off, it
    /// accepts the connection (and the TLS handshake on the TLS port) and
    /// then says nothing at all, as a hung server does, until the client
    /// leaves. For testing that clients time out rather than wait.
    pub fn greets(mut self, on: bool) -> Self {
        self.greets = on;
        self
    }

    /// Greets every new connection with `* BYE <text>` and closes it, as
    /// a server turning connections away does (RFC 9051 7.1.5): `text`
    /// may start with a response code, as `[UNAVAILABLE] Try later`, and
    /// Dovecot sends `Maximum number of connections from user+IP
    /// exceeded` this way. Off by default.
    pub fn greets_with_bye(mut self, text: &str) -> Self {
        self.bye_greeting = Some(text.to_string());
        self
    }

    /// Whether the plain-text port misbehaves at STARTTLS by sending a
    /// response of its own straight after its OK, still in plain text
    /// (`* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN] injected`), as an attacker
    /// on the path would (RFC 8314's STARTTLS injection). Off by default.
    /// For testing that clients refuse whatever came before TLS.
    pub fn injects_after_starttls(mut self, on: bool) -> Self {
        self.injects_after_starttls = on;
        self
    }

    /// Whether LITERAL+ (RFC 7888) is offered. On by default. Off, a
    /// client must wait for `+` before each literal it sends.
    pub fn literal_plus(mut self, on: bool) -> Self {
        self.literal_plus = on;
        self
    }

    /// Whether each message's FETCH data is split across two responses,
    /// as RFC 3501 7.4.2 allows: the first with the UID and the first half
    /// of the items, the second, without the UID, with the rest. Off by
    /// default.
    pub fn splits_fetches(mut self, on: bool) -> Self {
        self.splits_fetches = on;
        self
    }

    /// Whether a FETCH that asks for a body section also brings
    /// unsolicited flag updates, as CONDSTORE servers send when another
    /// client changes flags: `* n FETCH (UID u FLAGS (...) MODSEQ (m))`
    /// for every message of the folder it did not ask for, and one for the
    /// first message it asked for, before that message's answer. Off by
    /// default.
    pub fn unsolicited_fetches(mut self, on: bool) -> Self {
        self.unsolicited_fetches = on;
        self
    }

    /// Whether the server says it is Gmail, by advertising Gmail's
    /// `X-GM-EXT-1` capability. Off by default. Only the capability: the
    /// fake has none of Gmail's `X-GM-` commands or attributes, and its
    /// `\All` folder is an ordinary one the test fills, as Gmail's All
    /// Mail is filled with every message.
    pub fn gmail(mut self, on: bool) -> Self {
        self.gmail = on;
        self
    }

    /// Offers exactly these SASL mechanisms for AUTHENTICATE. LOGIN stays
    /// available to users with a password whatever is offered.
    pub fn mechanisms(mut self, mechanisms: &[Mechanism]) -> Self {
        self.mechanisms = mechanisms.iter().copied().collect();
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
