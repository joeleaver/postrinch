//! What an IMAP server says it can do (RFC 9051 7.2.2), as sync needs to
//! know it.

use std::collections::BTreeSet;

/// A server's capabilities, upper-cased (capability names are not case
/// sensitive).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities(BTreeSet<String>);

impl Capabilities {
    /// From the atoms of a `CAPABILITY` response or response code.
    pub fn parse(atoms: &str) -> Self {
        Self(
            atoms
                .split_ascii_whitespace()
                .map(str::to_ascii_uppercase)
                .collect(),
        )
    }

    /// From what async-imap parsed.
    pub(crate) fn from_async_imap(capabilities: &async_imap::types::Capabilities) -> Self {
        use async_imap::types::Capability;
        Self(
            capabilities
                .iter()
                .map(|c| match c {
                    Capability::Imap4rev1 => "IMAP4REV1".to_owned(),
                    Capability::Auth(mechanism) => format!("AUTH={mechanism}").to_ascii_uppercase(),
                    Capability::Atom(atom) => atom.to_ascii_uppercase(),
                })
                .collect(),
        )
    }

    /// Whether the server offers `name` (`IDLE`, `AUTH=PLAIN`...).
    pub fn has(&self, name: &str) -> bool {
        self.0.contains(&name.to_ascii_uppercase())
    }

    /// Whether `AUTHENTICATE <mechanism>` is offered.
    pub fn has_auth(&self, mechanism: &str) -> bool {
        self.has(&format!("AUTH={mechanism}"))
    }

    /// Every capability, upper-cased, in order.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    /// RFC 3501 STARTTLS.
    pub fn starttls(&self) -> bool {
        self.has("STARTTLS")
    }

    /// LOGINDISABLED: the LOGIN command is refused (until STARTTLS, or
    /// for good).
    pub fn login_disabled(&self) -> bool {
        self.has("LOGINDISABLED")
    }

    /// RFC 7162 CONDSTORE.
    pub fn condstore(&self) -> bool {
        self.has("CONDSTORE")
    }

    /// RFC 7162 QRESYNC.
    pub fn qresync(&self) -> bool {
        self.has("QRESYNC")
    }

    /// RFC 6851 MOVE.
    pub fn move_command(&self) -> bool {
        self.has("MOVE")
    }

    /// RFC 4315 UIDPLUS.
    pub fn uidplus(&self) -> bool {
        self.has("UIDPLUS")
    }

    /// RFC 6154 SPECIAL-USE.
    pub fn special_use(&self) -> bool {
        self.has("SPECIAL-USE")
    }

    /// RFC 5258 LIST-EXTENDED.
    pub fn list_extended(&self) -> bool {
        self.has("LIST-EXTENDED")
    }

    /// RFC 2177 IDLE.
    pub fn idle(&self) -> bool {
        self.has("IDLE")
    }

    /// RFC 5161 ENABLE.
    pub fn enable(&self) -> bool {
        self.has("ENABLE")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_case_insensitive() {
        let caps = Capabilities::parse("IMAP4rev1 auth=PLAIN Idle CONDSTORE STARTTLS");
        assert!(caps.has("imap4rev1"));
        assert!(caps.has_auth("plain"));
        assert!(!caps.has_auth("XOAUTH2"));
        assert!(caps.idle() && caps.condstore() && caps.starttls());
        assert!(!caps.qresync() && !caps.login_disabled());
    }
}
