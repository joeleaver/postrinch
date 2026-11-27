//! Invite links (§5.6): `https://{provider}/invite/{token}`, the form the
//! spec recommends. The link names the provider that holds the group, so
//! redeeming it needs nothing else.

use std::fmt;

use postrinch_ofscp::ofscp_core::Authority;

/// The path a join link's token follows.
const PREFIX: &str = "/invite/";

/// An invite link: the provider holding the group, and the invite's
/// token. The token is a bearer secret, so `Debug` leaves it out.
#[derive(Clone, PartialEq, Eq)]
pub struct InviteLink {
    provider: Authority,
    token: String,
}

impl InviteLink {
    /// The link for `token` at `provider`.
    pub(crate) fn new(provider: Authority, token: String) -> Self {
        Self { provider, token }
    }

    /// Read a link as the person pasted it: spaces around it and a
    /// trailing `/` are let pass, the scheme must be `https`, and the
    /// token must be a single path segment. `None` for anything else.
    pub fn parse(text: &str) -> Option<Self> {
        let rest = text.trim().strip_prefix("https://")?;
        let (authority, path) = rest.split_at(rest.find('/')?);
        let path = path.split(['?', '#']).next().unwrap_or(path);
        let token = path.strip_prefix(PREFIX)?.trim_end_matches('/');
        let token_ok = !token.is_empty()
            && token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.~".contains(&b));
        if !token_ok {
            return None;
        }
        let provider = Authority::canonicalize(authority).ok()?;
        Some(Self::new(provider, token.to_owned()))
    }

    /// The provider that holds the group, canonical: `forumall.app`.
    pub fn provider(&self) -> String {
        self.provider.to_string()
    }

    pub(crate) fn authority(&self) -> &Authority {
        &self.provider
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    /// The link, to copy and share.
    pub fn url(&self) -> String {
        format!("https://{}{PREFIX}{}", self.provider, self.token)
    }
}

impl fmt::Debug for InviteLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InviteLink")
            .field("provider", &self.provider.to_string())
            .field("token", &"[redacted]")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_names_its_provider_and_token() {
        let link = InviteLink::parse("  https://Forumall.app/invite/AbC_12-x/ \n").unwrap();
        assert_eq!(link.provider(), "forumall.app");
        assert_eq!(link.token(), "AbC_12-x");
        assert_eq!(link.url(), "https://forumall.app/invite/AbC_12-x");
    }

    #[test]
    fn a_port_is_kept_and_a_query_ignored() {
        let link = InviteLink::parse("https://127.0.0.1:8443/invite/tok?utm=x").unwrap();
        assert_eq!(link.provider(), "127.0.0.1:8443");
        assert_eq!(link.token(), "tok");
    }

    #[test]
    fn other_text_is_not_a_link() {
        for text in [
            "",
            "tok",
            "http://forumall.app/invite/tok",
            "https://forumall.app/invite/",
            "https://forumall.app/invites/tok",
            "https://forumall.app/invite/a/b",
            "https://forumall.app/invite/a%20b",
            "https:///invite/tok",
        ] {
            assert!(InviteLink::parse(text).is_none(), "{text:?}");
        }
    }

    #[test]
    fn debug_hides_the_token() {
        let link = InviteLink::parse("https://forumall.app/invite/secret").unwrap();
        assert!(!format!("{link:?}").contains("secret"));
    }
}
