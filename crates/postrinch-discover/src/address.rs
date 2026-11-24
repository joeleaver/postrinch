//! The email address discovery starts from.

use thiserror::Error;

/// An address that cannot be looked up.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AddressError {
    /// There is no `@`, or nothing before or after it.
    #[error("{0:?} is not an email address")]
    NotAnAddress(String),
    /// The part after the `@` is not a domain name (an IP address, or
    /// characters a domain cannot have).
    #[error("{0:?} is not a domain name")]
    BadDomain(String),
}

/// A parsed address: what is before the `@`, and the domain after it in
/// ASCII (internationalised domains in their `xn--` form), lower case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Address {
    /// The address as given, trimmed, with the domain normalised.
    pub full: String,
    /// The part before the `@`.
    pub local: String,
    /// The domain, ASCII, lower case, no trailing dot.
    pub domain: String,
}

impl Address {
    /// Parses `address`, splitting at its last `@`.
    pub fn parse(address: &str) -> Result<Self, AddressError> {
        let trimmed = address.trim();
        let (local, domain) = trimmed
            .rsplit_once('@')
            .filter(|(local, domain)| !local.is_empty() && !domain.is_empty())
            .ok_or_else(|| AddressError::NotAnAddress(trimmed.to_owned()))?;
        let domain = domain.strip_suffix('.').unwrap_or(domain);
        let bad = || AddressError::BadDomain(domain.to_owned());
        let domain = match url::Host::parse(domain).map_err(|_| bad())? {
            url::Host::Domain(ascii) if ascii.contains('.') => ascii.to_ascii_lowercase(),
            _ => return Err(bad()),
        };
        Ok(Address {
            full: format!("{local}@{domain}"),
            local: local.to_owned(),
            domain,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_normalises() {
        let a = Address::parse("  Ada@Example.ORG. ").unwrap();
        assert_eq!(a.local, "Ada");
        assert_eq!(a.domain, "example.org");
        assert_eq!(a.full, "Ada@example.org");
    }

    #[test]
    fn international_domains_become_ascii() {
        let a = Address::parse("jo@bücher.de").unwrap();
        assert_eq!(a.domain, "xn--bcher-kva.de");
    }

    #[test]
    fn splits_at_the_last_at() {
        let a = Address::parse("\"a@b\"@example.com").unwrap();
        assert_eq!(a.local, "\"a@b\"");
        assert_eq!(a.domain, "example.com");
    }

    #[test]
    fn refuses_what_is_not_an_address() {
        for bad in ["", "ada", "@example.org", "ada@"] {
            assert!(matches!(
                Address::parse(bad),
                Err(AddressError::NotAnAddress(_))
            ));
        }
        for bad in [
            "ada@[127.0.0.1]",
            "ada@127.0.0.1",
            "ada@localhost",
            "ada@exa mple.org",
        ] {
            assert!(
                matches!(Address::parse(bad), Err(AddressError::BadDomain(_))),
                "{bad}"
            );
        }
    }
}
