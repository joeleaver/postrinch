//! What Postrinch knows about a few big providers beyond what autoconfig
//! files say: who hosts a domain's mail, and where to make an app password.

use crate::types::{Candidate, HostedBy};

/// Who hosts mail whose MX record points at `exchange` (lower case, no
/// trailing dot).
pub(crate) fn hosted_by_mx(exchange: &str) -> Option<HostedBy> {
    if ends_with_domain(exchange, "google.com") || ends_with_domain(exchange, "googlemail.com") {
        Some(HostedBy::Google)
    } else if ends_with_domain(exchange, "mail.protection.outlook.com") {
        Some(HostedBy::Microsoft)
    } else {
        None
    }
}

/// The ISPDB domain that describes a host's IMAP and SMTP servers.
pub(crate) fn ispdb_domain(hosted_by: HostedBy) -> &'static str {
    match hosted_by {
        HostedBy::Google => "gmail.com",
        HostedBy::Microsoft => "outlook.com",
    }
}

/// Who runs the servers found, when it is Google or Microsoft.
pub(crate) fn hosted_by_servers(imap: &[Candidate]) -> Option<HostedBy> {
    imap.iter().find_map(|c| hosted_by_host(&c.host))
}

/// Who runs the IMAP server `host`, when it is Google or Microsoft.
fn hosted_by_host(host: &str) -> Option<HostedBy> {
    if ends_with_domain(host, "gmail.com") || ends_with_domain(host, "googlemail.com") {
        Some(HostedBy::Google)
    } else if ends_with_domain(host, "office365.com") || ends_with_domain(host, "outlook.com") {
        Some(HostedBy::Microsoft)
    } else {
        None
    }
}

/// Providers that need an app password for mail apps, by the domain their
/// IMAP servers are under: their name, and the page that makes one.
const APP_PASSWORD_PAGES: &[(&str, &str, &str)] = &[
    (
        "fastmail.com",
        "Fastmail",
        "https://app.fastmail.com/settings/security/integrations",
    ),
    (
        "me.com",
        "iCloud",
        "https://account.apple.com/account/manage",
    ),
    (
        "yahoo.com",
        "Yahoo",
        "https://login.yahoo.com/account/security",
    ),
    ("aol.com", "AOL", "https://login.aol.com/account/security"),
    (
        "gmail.com",
        "Google",
        "https://myaccount.google.com/apppasswords",
    ),
];

/// The page for making an app password, when the first IMAP server belongs
/// to a provider that needs one.
pub(crate) fn app_password_url(imap: &[Candidate]) -> Option<String> {
    let host = &imap.first()?.host;
    app_password_provider(host).map(|(_, url)| url.to_owned())
}

/// The provider needing an app password whose IMAP server is `host`: its
/// name and the page that makes one.
fn app_password_provider(host: &str) -> Option<(&'static str, &'static str)> {
    APP_PASSWORD_PAGES
        .iter()
        .find(|(domain, _, _)| ends_with_domain(host, domain))
        .map(|(_, name, url)| (*name, *url))
}

/// What Postrinch knows of a provider from its IMAP server's name alone,
/// with no lookup: for servers already known, such as those imported
/// from another mail app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KnownHost {
    /// The provider's name, when Postrinch knows it.
    pub provider_name: Option<&'static str>,
    /// Google or Microsoft, when they run the server.
    pub hosted_by: Option<HostedBy>,
    /// The page for making an app password, when the provider needs one.
    pub app_password_url: Option<&'static str>,
}

/// What Postrinch knows of the IMAP server `host` (any case).
pub fn known_host(host: &str) -> KnownHost {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let hosted_by = hosted_by_host(&host);
    let app = app_password_provider(&host);
    KnownHost {
        provider_name: app.map(|(name, _)| name).or(match hosted_by {
            Some(HostedBy::Google) => Some("Google"),
            Some(HostedBy::Microsoft) => Some("Microsoft"),
            None => None,
        }),
        hosted_by,
        app_password_url: app.map(|(_, url)| url),
    }
}

/// `host` is `domain` or a name under it.
pub(crate) fn ends_with_domain(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|rest| rest.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Security, Username};

    fn imap(host: &str) -> Vec<Candidate> {
        vec![Candidate {
            host: host.to_owned(),
            port: 993,
            security: Security::Tls,
            username: Username::Address,
        }]
    }

    #[test]
    fn mx_hosts() {
        assert_eq!(hosted_by_mx("aspmx.l.google.com"), Some(HostedBy::Google));
        assert_eq!(
            hosted_by_mx("alt1.aspmx.l.google.com"),
            Some(HostedBy::Google)
        );
        assert_eq!(
            hosted_by_mx("aspmx2.googlemail.com"),
            Some(HostedBy::Google)
        );
        assert_eq!(
            hosted_by_mx("acme-co.mail.protection.outlook.com"),
            Some(HostedBy::Microsoft)
        );
        assert_eq!(hosted_by_mx("notgoogle.com"), None);
        assert_eq!(hosted_by_mx("in1-smtp.messagingengine.com"), None);
    }

    #[test]
    fn server_hosts() {
        assert_eq!(
            hosted_by_servers(&imap("imap.gmail.com")),
            Some(HostedBy::Google)
        );
        assert_eq!(
            hosted_by_servers(&imap("outlook.office365.com")),
            Some(HostedBy::Microsoft)
        );
        assert_eq!(hosted_by_servers(&imap("imap.fastmail.com")), None);
    }

    #[test]
    fn known_hosts() {
        let gmail = known_host("IMAP.Gmail.com.");
        assert_eq!(gmail.hosted_by, Some(HostedBy::Google));
        assert_eq!(gmail.provider_name, Some("Google"));
        let outlook = known_host("outlook.office365.com");
        assert_eq!(outlook.hosted_by, Some(HostedBy::Microsoft));
        assert_eq!(outlook.provider_name, Some("Microsoft"));
        assert_eq!(outlook.app_password_url, None);
        let fastmail = known_host("imap.fastmail.com");
        assert_eq!(fastmail.hosted_by, None);
        assert_eq!(fastmail.provider_name, Some("Fastmail"));
        assert!(fastmail.app_password_url.is_some());
        assert_eq!(known_host("mail.example.org"), KnownHost::default());
        // A name that only ends like a provider's is not theirs.
        assert_eq!(known_host("notgmail.com").hosted_by, None);
    }

    #[test]
    fn app_password_pages() {
        assert!(app_password_url(&imap("imap.mail.me.com")).is_some());
        assert!(app_password_url(&imap("imap.fastmail.com")).is_some());
        assert_eq!(app_password_url(&imap("imap.example.org")), None);
        assert_eq!(app_password_url(&[]), None);
    }
}
