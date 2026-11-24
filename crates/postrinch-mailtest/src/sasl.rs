//! SASL messages both fakes accept: PLAIN (RFC 4616) and Google's XOAUTH2.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

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
    fn empty_response() {
        assert_eq!(decode(b"="), Some(Vec::new()));
        assert_eq!(decode(b"YWJj"), Some(b"abc".to_vec()));
        assert_eq!(decode(b"!!"), None);
    }
}
