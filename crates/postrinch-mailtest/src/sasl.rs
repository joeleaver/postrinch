//! SASL messages both fakes accept: PLAIN (RFC 4616), Google's XOAUTH2 and
//! OAUTHBEARER (RFC 7628).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// A SASL mechanism a fake can offer. The password mechanisms are offered
/// together: [`Mechanism::Plain`] stands for `AUTH=PLAIN` on IMAP, and for
/// `PLAIN` and `LOGIN` on SMTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mechanism {
    /// RFC 4616 PLAIN (and SMTP's LOGIN).
    Plain,
    /// Google's XOAUTH2.
    XOAuth2,
    /// RFC 7628 OAUTHBEARER.
    OAuthBearer,
}

impl Mechanism {
    /// What both fakes offer unless told otherwise.
    pub(crate) const DEFAULT: [Mechanism; 2] = [Mechanism::Plain, Mechanism::XOAuth2];

    /// Every mechanism, in the order they are advertised.
    pub(crate) const ALL: [Mechanism; 3] =
        [Mechanism::Plain, Mechanism::XOAuth2, Mechanism::OAuthBearer];

    /// The mechanism's name, as advertised and as clients ask for it.
    pub fn name(self) -> &'static str {
        match self {
            Mechanism::Plain => "PLAIN",
            Mechanism::XOAuth2 => "XOAUTH2",
            Mechanism::OAuthBearer => "OAUTHBEARER",
        }
    }

    /// The mechanism a client named, if it is one of these (SMTP's
    /// `LOGIN` exchange is handled on its own).
    pub(crate) fn named(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|m| m.name().eq_ignore_ascii_case(name))
    }

    /// Parses the client's message for this mechanism.
    pub(crate) fn parse(self, message: &[u8]) -> Option<Claim> {
        match self {
            Mechanism::Plain => parse_plain(message),
            Mechanism::XOAuth2 => parse_xoauth2(message),
            Mechanism::OAuthBearer => parse_oauthbearer(message),
        }
    }

    /// The error challenge sent before refusing a bearer token: the client
    /// answers it (XOAUTH2 with an empty line, OAUTHBEARER with `%x01`)
    /// and then gets the failure. `None` for PLAIN, refused at once.
    pub(crate) fn failure_challenge(self) -> Option<&'static [u8]> {
        match self {
            Mechanism::Plain => None,
            Mechanism::XOAuth2 => Some(XOAUTH2_FAILURE),
            Mechanism::OAuthBearer => Some(OAUTHBEARER_FAILURE),
        }
    }
}

/// What a client claimed in a SASL exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Claim {
    /// A user name and password.
    Password { user: String, password: String },
    /// A user name and an OAuth 2 bearer token.
    Bearer { user: String, token: String },
}

/// Decodes a base64 SASL response. `None` if it is not base64.
pub(crate) fn decode(line: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(line).ok()?.trim();
    // RFC 4954 and RFC 9051: "=" stands for an empty initial response.
    if text == "=" {
        return Some(Vec::new());
    }
    STANDARD.decode(text).ok()
}

/// Encodes a server challenge.
pub(crate) fn encode(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Parses a PLAIN message: `[authzid] NUL authcid NUL password`. An
/// authorization identity other than the user's own is refused.
pub(crate) fn parse_plain(message: &[u8]) -> Option<Claim> {
    let text = std::str::from_utf8(message).ok()?;
    let mut parts = text.split('\0');
    let authzid = parts.next()?;
    let user = parts.next()?;
    let password = parts.next()?;
    if parts.next().is_some() || (!authzid.is_empty() && authzid != user) {
        return None;
    }
    Some(Claim::Password {
        user: user.to_string(),
        password: password.to_string(),
    })
}

/// Parses an XOAUTH2 message: `user=<u>^Aauth=Bearer <token>^A^A`.
pub(crate) fn parse_xoauth2(message: &[u8]) -> Option<Claim> {
    let text = std::str::from_utf8(message).ok()?;
    let mut user = None;
    let mut token = None;
    for field in text.split('\x01').filter(|f| !f.is_empty()) {
        if let Some(value) = field.strip_prefix("user=") {
            user = Some(value.to_string());
        } else if let Some(value) = field.strip_prefix("auth=") {
            let (scheme, rest) = value.split_once(' ')?;
            if !scheme.eq_ignore_ascii_case("Bearer") {
                return None;
            }
            token = Some(rest.to_string());
        }
    }
    Some(Claim::Bearer {
        user: user?,
        token: token?,
    })
}

/// The JSON challenge Google sends when an XOAUTH2 token is refused. The
/// client answers it with an empty line and then gets the failure.
pub(crate) const XOAUTH2_FAILURE: &[u8] =
    br#"{"status":"401","schemes":"Bearer","scope":"https://mail.google.com/"}"#;

/// Parses an OAUTHBEARER message (RFC 7628 3.1): a GS2 header naming the
/// user (`n,a=<u>,`), then `^A`-separated key/value pairs ending in
/// `^A^A`, one of them `auth=Bearer <token>`. The fake needs the `a=`
/// authorization identity to know whose token it is.
pub(crate) fn parse_oauthbearer(message: &[u8]) -> Option<Claim> {
    let text = std::str::from_utf8(message).ok()?;
    let (gs2, pairs) = text.split_once('\x01')?;
    let pairs = pairs.strip_suffix("\x01\x01")?;
    // "n" or "y": no channel binding; "p=..." asks for one, which
    // OAUTHBEARER does not support.
    let mut header = gs2.split(',');
    if !matches!(header.next()?, "n" | "y") {
        return None;
    }
    let user = header.next()?.strip_prefix("a=")?;
    if user.is_empty() || header.next() != Some("") || header.next().is_some() {
        return None;
    }
    // RFC 5801: "," and "=" in the identity are written =2C and =3D.
    let user = user.replace("=2C", ",").replace("=3D", "=");
    let mut token = None;
    for pair in pairs.split('\x01') {
        let (key, value) = pair.split_once('=')?;
        if key == "auth" {
            let (scheme, rest) = value.split_once(' ')?;
            if !scheme.eq_ignore_ascii_case("Bearer") {
                return None;
            }
            token = Some(rest.to_string());
        }
    }
    Some(Claim::Bearer {
        user,
        token: token?,
    })
}

/// The JSON challenge RFC 7628 3.2.2 sends when an OAUTHBEARER token is
/// refused. The client answers it with a single `%x01` and then gets the
/// failure.
pub(crate) const OAUTHBEARER_FAILURE: &[u8] = br#"{"status":"invalid_token","scope":"mail"}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_with_and_without_authzid() {
        assert_eq!(
            parse_plain(b"\0alice\0pw"),
            Some(Claim::Password {
                user: "alice".into(),
                password: "pw".into()
            })
        );
        assert!(parse_plain(b"alice\0alice\0pw").is_some());
        assert!(parse_plain(b"bob\0alice\0pw").is_none());
        assert!(parse_plain(b"alice\0pw").is_none());
    }

    #[test]
    fn xoauth2() {
        assert_eq!(
            parse_xoauth2(b"user=alice\x01auth=Bearer tok\x01\x01"),
            Some(Claim::Bearer {
                user: "alice".into(),
                token: "tok".into()
            })
        );
        assert!(parse_xoauth2(b"user=alice\x01auth=Basic tok\x01\x01").is_none());
        assert!(parse_xoauth2(b"auth=Bearer tok\x01\x01").is_none());
    }

    #[test]
    fn oauthbearer() {
        let bearer = |user: &str| {
            Some(Claim::Bearer {
                user: user.into(),
                token: "tok".into(),
            })
        };
        assert_eq!(
            parse_oauthbearer(
                b"n,a=alice,\x01host=localhost\x01port=993\x01auth=Bearer tok\x01\x01"
            ),
            bearer("alice")
        );
        assert_eq!(
            parse_oauthbearer(b"n,a=a=2Cb,\x01auth=Bearer tok\x01\x01"),
            bearer("a,b")
        );
        // No identity, a channel binding, another scheme, a missing end.
        assert!(parse_oauthbearer(b"n,,\x01auth=Bearer tok\x01\x01").is_none());
        assert!(parse_oauthbearer(b"p=tls-unique,a=alice,\x01auth=Bearer tok\x01\x01").is_none());
        assert!(parse_oauthbearer(b"n,a=alice,\x01auth=Basic tok\x01\x01").is_none());
        assert!(parse_oauthbearer(b"n,a=alice,\x01auth=Bearer tok\x01").is_none());
    }

    #[test]
    fn empty_response() {
        assert_eq!(decode(b"="), Some(Vec::new()));
        assert_eq!(decode(b"YWJj"), Some(b"abc".to_vec()));
        assert_eq!(decode(b"!!"), None);
    }
}
