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
    imap.iter().find_map(|c| {
        let host = c.host.as_str();
        if ends_with_domain(host, "gmail.com") || ends_with_domain(host, "googlemail.com") {
            Some(HostedBy::Google)
        } else if ends_with_domain(host, "office365.com") || ends_with_domain(host, "outlook.com") {
            Some(HostedBy::Microsoft)
        } else {
            None
        }
    })
}

/// Providers that need an app password for mail apps, by the domain their
/// IMAP servers are under, with the page that makes one.
const APP_PASSWORD_PAGES: &[(&str, &str)] = &[
    (
        "fastmail.com",
        "https://app.fastmail.com/settings/security/integrations",
    ),
    ("me.com", "https://account.apple.com/account/manage"),
    ("yahoo.com", "https://login.yahoo.com/account/security"),
    ("aol.com", "https://login.aol.com/account/security"),
    ("gmail.com", "https://myaccount.google.com/apppasswords"),
];

/// The page for making an app password, when the first IMAP server belongs
/// to a provider that needs one.
pub(crate) fn app_password_url(imap: &[Candidate]) -> Option<String> {
    let host = &imap.first()?.host;
    APP_PASSWORD_PAGES
        .iter()
        .find(|(domain, _)| ends_with_domain(host, domain))
        .map(|(_, url)| (*url).to_owned())
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
    fn app_password_pages() {
        assert!(app_password_url(&imap("imap.mail.me.com")).is_some());
        assert!(app_password_url(&imap("imap.fastmail.com")).is_some());
        assert_eq!(app_password_url(&imap("imap.example.org")), None);
        assert_eq!(app_password_url(&[]), None);
    }
}
