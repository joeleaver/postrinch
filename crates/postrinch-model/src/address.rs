use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    #[error("{0:?} has no '@'")]
    MissingAt(String),
    #[error("{0:?} has an empty local part or domain")]
    EmptyPart(String),
    #[error("{0:?} contains whitespace")]
    Whitespace(String),
}

/// An email address, `local@domain`.
///
/// The domain is lowercased (domains are case-insensitive); the local part is
/// kept as written, because RFC 5321 lets a server treat it case-sensitively.
/// [`EmailAddress::key`] gives the form two addresses are compared by when
/// deciding whether they are the same person: both parts lowercased, which is
/// how every mainstream provider behaves.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EmailAddress {
    local: String,
    domain: String,
}

impl EmailAddress {
    pub fn parse(input: &str) -> Result<Self, AddressError> {
        let trimmed = input.trim();
        if trimmed.chars().any(char::is_whitespace) {
            return Err(AddressError::Whitespace(input.to_owned()));
        }
        let (local, domain) = trimmed
            .rsplit_once('@')
            .ok_or_else(|| AddressError::MissingAt(input.to_owned()))?;
        if local.is_empty() || domain.is_empty() {
            return Err(AddressError::EmptyPart(input.to_owned()));
        }
        Ok(Self {
            local: local.to_owned(),
            domain: domain.to_ascii_lowercase(),
        })
    }

    pub fn local(&self) -> &str {
        &self.local
    }

    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// The comparison key: the whole address lowercased.
    pub fn key(&self) -> String {
        format!("{}@{}", self.local.to_lowercase(), self.domain)
    }
}

impl fmt::Display for EmailAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.local, self.domain)
    }
}

impl TryFrom<String> for EmailAddress {
    type Error = AddressError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<EmailAddress> for String {
    fn from(value: EmailAddress) -> Self {
        value.to_string()
    }
}

/// An address with the display name it was given in a header, as in
/// `Ada Lovelace <ada@example.org>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Mailbox {
    pub name: Option<String>,
    pub address: EmailAddress,
}

impl Mailbox {
    pub fn new(name: Option<String>, address: EmailAddress) -> Self {
        let name = name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty());
        Self { name, address }
    }

    /// The name if there is one, else the address.
    pub fn display_name(&self) -> String {
        match &self.name {
            Some(name) => name.clone(),
            None => self.address.to_string(),
        }
    }
}

impl fmt::Display for Mailbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name {
            Some(name) => write!(f, "{name} <{}>", self.address),
            None => write!(f, "{}", self.address),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercases_domain_only() {
        let a = EmailAddress::parse("Ada.Lovelace@Example.ORG").unwrap();
        assert_eq!(a.to_string(), "Ada.Lovelace@example.org");
        assert_eq!(a.key(), "ada.lovelace@example.org");
    }

    #[test]
    fn splits_on_last_at() {
        let a = EmailAddress::parse("\"odd@local\"@example.org").unwrap();
        assert_eq!(a.local(), "\"odd@local\"");
        assert_eq!(a.domain(), "example.org");
    }

    #[test]
    fn rejects_malformed() {
        assert!(matches!(
            EmailAddress::parse("nobody"),
            Err(AddressError::MissingAt(_))
        ));
        assert!(matches!(
            EmailAddress::parse("@example.org"),
            Err(AddressError::EmptyPart(_))
        ));
        assert!(matches!(
            EmailAddress::parse("a b@example.org"),
            Err(AddressError::Whitespace(_))
        ));
    }

    #[test]
    fn mailbox_drops_blank_names() {
        let addr = EmailAddress::parse("ada@example.org").unwrap();
        let m = Mailbox::new(Some("  ".into()), addr.clone());
        assert_eq!(m.name, None);
        assert_eq!(m.to_string(), "ada@example.org");
        let named = Mailbox::new(Some("Ada".into()), addr);
        assert_eq!(named.to_string(), "Ada <ada@example.org>");
    }

    #[test]
    fn serializes_as_string() {
        let a = EmailAddress::parse("ada@example.org").unwrap();
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(json, "\"ada@example.org\"");
        let back: EmailAddress = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a);
    }
}
