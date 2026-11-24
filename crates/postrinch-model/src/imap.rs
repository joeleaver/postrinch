//! An IMAP/SMTP account's settings, as kept on its [`Account`](crate::Account)
//! (docs/imap.md, "The account's settings"). No secret is ever here: a
//! password or refresh token is kept in the OS keychain, keyed by the
//! account's address.

use serde::{Deserialize, Serialize};

/// How an IMAP/SMTP account reaches its servers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImapSettings {
    /// Where mail is read.
    pub imap: Server,
    /// Where mail is sent.
    pub smtp: Server,
    #[serde(default)]
    pub sign_in: SignInMethod,
    /// The folders the person chose for the special roles, where they
    /// override what the server says.
    #[serde(default)]
    pub roles: FolderRoles,
    /// Whether the provider files sent mail in Sent itself (Gmail-like
    /// servers, Outlook, iCloud), so Postrinch does not append its own copy.
    #[serde(default = "files_sent_by_default")]
    pub server_files_sent: bool,
}

/// On until a send shows otherwise (docs/imap.md, "Writing and sending").
fn files_sent_by_default() -> bool {
    true
}

/// One server: IMAP's or SMTP's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub security: Security,
    /// The user name it signs in as, often the address.
    pub username: String,
}

/// How a connection is secured. Plain text is not offered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Security {
    /// TLS from the first byte (IMAP 993, SMTP 465).
    #[default]
    Tls,
    /// A plain connection upgraded with STARTTLS before anything else is
    /// said (IMAP 143, SMTP 587). Postrinch refuses to go on without it.
    StartTls,
}

/// How the account signs in. Which secret each needs is kept in the
/// keychain, never here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SignInMethod {
    /// One password (or app password) for IMAP and SMTP.
    #[default]
    Password,
    /// A password for IMAP and another for SMTP.
    SeparateSmtpPassword,
    /// OAuth 2 with a provider Postrinch has a client for, by its name
    /// (`microsoft`, `yahoo`).
    OAuth2 { provider: String },
}

/// The server folders the person chose for each role, by their names on
/// the server (as the server spells them). `None` follows SPECIAL-USE, or
/// failing that the names providers use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FolderRoles {
    #[serde(default)]
    pub sent: Option<String>,
    #[serde(default)]
    pub drafts: Option<String>,
    #[serde(default)]
    pub archive: Option<String>,
    #[serde(default)]
    pub trash: Option<String>,
    #[serde(default)]
    pub junk: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ImapSettings {
        ImapSettings {
            imap: Server {
                host: "imap.example.org".into(),
                port: 993,
                security: Security::Tls,
                username: "ada@example.org".into(),
            },
            smtp: Server {
                host: "smtp.example.org".into(),
                port: 587,
                security: Security::StartTls,
                username: "ada@example.org".into(),
            },
            sign_in: SignInMethod::OAuth2 {
                provider: "microsoft".into(),
            },
            roles: FolderRoles {
                archive: Some("Archives".into()),
                ..FolderRoles::default()
            },
            server_files_sent: false,
        }
    }

    #[test]
    fn round_trips_through_json() {
        let settings = settings();
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(
            serde_json::from_str::<ImapSettings>(&json).unwrap(),
            settings
        );
    }

    #[test]
    fn missing_fields_take_their_defaults() {
        let json = r#"{
            "imap": {"host": "imap.example.org", "port": 993, "username": "ada"},
            "smtp": {"host": "smtp.example.org", "port": 465, "username": "ada"}
        }"#;
        let settings: ImapSettings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.imap.security, Security::Tls);
        assert_eq!(settings.sign_in, SignInMethod::Password);
        assert_eq!(settings.roles, FolderRoles::default());
        assert!(settings.server_files_sent);
    }
}
