//! The providers IMAP accounts sign in to with OAuth, and which one an
//! address's discovery points at.

use postrinch_discover::{Discovery, HostedBy};
use postrinch_model::SignInMethod;

/// A provider Postrinch signs IMAP accounts in to with OAuth 2.
///
/// Non-exhaustive: Yahoo and AOL join once Yahoo approves an app for
/// mail (docs/imap.md, "Credentials"); until then they use app
/// passwords.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OAuthProvider {
    /// Gmail and Google Workspace over IMAP, with Postrinch's Google client.
    Google,
    /// Outlook.com, Hotmail, Live and Microsoft 365.
    Microsoft,
}

impl OAuthProvider {
    /// Every provider, in the order the dialog lists them.
    pub const ALL: &[OAuthProvider] = &[OAuthProvider::Google, OAuthProvider::Microsoft];

    /// The name kept in the account's settings ([`SignInMethod::OAuth2`])
    /// and in its secret.
    pub fn id(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Microsoft => "microsoft",
        }
    }

    /// The provider an [`id`](Self::id) names.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|p| p.id() == id)
    }

    /// The provider's name, for "Sign in with …".
    pub fn name(self) -> &'static str {
        match self {
            Self::Google => "Google",
            Self::Microsoft => "Microsoft",
        }
    }

    /// How an account signed in with this provider signs in.
    pub fn sign_in_method(self) -> SignInMethod {
        SignInMethod::OAuth2 {
            provider: self.id().to_owned(),
        }
    }

    /// The provider an address found by `discovery` signs in with, or
    /// `None` when it signs in with a password. Mail hosted by Google or
    /// Microsoft (by the domain's MX records or the servers found) is
    /// theirs; every other provider is a password's, even one whose
    /// autoconfig file offers OAuth 2, as Postrinch has no client for it.
    pub fn for_discovery(discovery: &Discovery) -> Option<Self> {
        match discovery.hosted_by? {
            HostedBy::Google => Some(Self::Google),
            HostedBy::Microsoft => Some(Self::Microsoft),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for p in OAuthProvider::ALL {
            assert_eq!(OAuthProvider::from_id(p.id()), Some(*p));
        }
        assert_eq!(OAuthProvider::from_id("yahoo"), None);
        assert_eq!(
            OAuthProvider::Microsoft.sign_in_method(),
            SignInMethod::OAuth2 {
                provider: "microsoft".into()
            }
        );
    }

    #[test]
    fn discovery_chooses_the_provider() {
        let mut found = Discovery::nothing();
        assert_eq!(OAuthProvider::for_discovery(&found), None);
        found.hosted_by = Some(HostedBy::Microsoft);
        assert_eq!(
            OAuthProvider::for_discovery(&found),
            Some(OAuthProvider::Microsoft)
        );
        found.hosted_by = Some(HostedBy::Google);
        assert_eq!(
            OAuthProvider::for_discovery(&found),
            Some(OAuthProvider::Google)
        );
        // OAuth 2 in a provider's own file, with no client for it here.
        let mut other = Discovery::nothing();
        other.oauth2_only = true;
        assert_eq!(OAuthProvider::for_discovery(&other), None);
    }
}
