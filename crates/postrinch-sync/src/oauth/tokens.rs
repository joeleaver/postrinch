//! What a token endpoint answers, and who signed in.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use postrinch_model::EmailAddress;
use serde::{Deserialize, Deserializer};

use crate::{SyncError, SyncResult};

/// A successful token response.
#[derive(Deserialize)]
pub(crate) struct Tokens {
    pub access_token: String,
    #[serde(deserialize_with = "seconds")]
    pub expires_in: u64,
    /// Sent at sign-in, and by providers that rotate them (Microsoft) on
    /// a refresh too.
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub scope: String,
    /// The OpenID Connect ID token, when `openid` was asked for.
    pub id_token: Option<String>,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("expires_in", &self.expires_in)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

/// `expires_in` as a number, or as the string Microsoft's older endpoints
/// sent.
fn seconds<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Seconds {
        Number(u64),
        Text(String),
    }
    match Seconds::deserialize(d)? {
        Seconds::Number(n) => Ok(n),
        Seconds::Text(t) => t.trim().parse().map_err(serde::de::Error::custom),
    }
}

/// The userinfo endpoint's answer.
#[derive(Debug, Deserialize)]
pub(crate) struct UserInfo {
    pub email: String,
    #[serde(default)]
    pub email_verified: bool,
    pub name: Option<String>,
}

/// Who signed in.
#[derive(Debug, Clone)]
pub(crate) struct Identified {
    pub address: EmailAddress,
    /// Whether the provider vouches for the address. An ID token from
    /// Microsoft is taken to: see [`from_id_token`].
    pub verified: bool,
    pub name: Option<String>,
}

impl Identified {
    pub(crate) fn from_userinfo(user: UserInfo) -> SyncResult<Self> {
        Ok(Self {
            address: parse_address(&user.email)?,
            verified: user.email_verified,
            name: user.name.filter(|n| !n.trim().is_empty()),
        })
    }
}

#[derive(Deserialize)]
struct Claims {
    email: Option<String>,
    preferred_username: Option<String>,
    /// A work account's user principal name, where the token has one.
    upn: Option<String>,
    name: Option<String>,
}

/// Who an ID token says signed in. Its addresses are the `email` claim,
/// the `preferred_username` (a work account's sign-in name) and the `upn`,
/// those of them that are addresses: `email` may be an alias of the
/// sign-in name, or missing. With `asked`, the address the sign-in was
/// for, whichever of them is that address (compared without regard to
/// case); when none is, or without `asked`, the first.
///
/// The token's signature is not checked: it came straight from the token
/// endpoint over TLS, which OpenID Connect Core (3.1.3.7) accepts in place
/// of one. Nor does the address grant anything: the mail server checks
/// the access token against the user it signs in.
pub(crate) fn from_id_token(
    id_token: &str,
    asked: Option<&EmailAddress>,
) -> SyncResult<Identified> {
    let unreadable = || SyncError::SignIn("the sign-in's ID token does not parse".into());
    let payload = id_token.split('.').nth(1).ok_or_else(unreadable)?;
    let json = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| unreadable())?;
    let claims: Claims = serde_json::from_slice(&json).map_err(|_| unreadable())?;
    let addresses: Vec<EmailAddress> = [claims.email, claims.preferred_username, claims.upn]
        .into_iter()
        .flatten()
        .filter_map(|a| EmailAddress::parse(&a).ok())
        .collect();
    let address = asked
        .and_then(|asked| addresses.iter().find(|a| a.key() == asked.key()))
        .or(addresses.first())
        .cloned()
        .ok_or_else(|| SyncError::SignIn("the sign-in did not say which address it is".into()))?;
    Ok(Identified {
        address,
        verified: true,
        name: claims.name.filter(|n| !n.trim().is_empty()),
    })
}

fn parse_address(text: &str) -> SyncResult<EmailAddress> {
    EmailAddress::parse(text).map_err(|e| SyncError::SignIn(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id_token(claims: serde_json::Value) -> String {
        let part = |v: &serde_json::Value| URL_SAFE_NO_PAD.encode(v.to_string());
        format!(
            "{}.{}.sig",
            part(&serde_json::json!({"alg": "RS256"})),
            part(&claims)
        )
    }

    #[test]
    fn reads_the_address_from_an_id_token() {
        let who = from_id_token(
            &id_token(serde_json::json!({
                "email": "Ada@Outlook.com", "preferred_username": "x", "name": "Ada"
            })),
            None,
        )
        .unwrap();
        assert_eq!(who.address.key(), "ada@outlook.com");
        assert_eq!(who.name.as_deref(), Some("Ada"));

        // A work account without the optional `email` claim.
        let work = from_id_token(
            &id_token(serde_json::json!({
                "preferred_username": "ada@contoso.com"
            })),
            None,
        )
        .unwrap();
        assert_eq!(work.address.key(), "ada@contoso.com");

        assert!(from_id_token(&id_token(serde_json::json!({"name": "Ada"})), None).is_err());
        assert!(from_id_token("not-a-jwt", None).is_err());
    }

    #[test]
    fn the_address_asked_for_is_found_among_the_claims() {
        let asked = EmailAddress::parse("Ada@Contoso.com").unwrap();
        // `email` is an alias; the sign-in name is the address asked for.
        let alias = id_token(serde_json::json!({
            "email": "ada.lovelace@contoso.com", "preferred_username": "ada@contoso.com"
        }));
        let who = from_id_token(&alias, Some(&asked)).unwrap();
        assert_eq!(who.address.key(), "ada@contoso.com");

        let by_upn = id_token(serde_json::json!({
            "email": "ada.lovelace@contoso.com", "preferred_username": "Ada Lovelace",
            "upn": "ADA@contoso.com"
        }));
        assert_eq!(
            from_id_token(&by_upn, Some(&asked)).unwrap().address.key(),
            "ada@contoso.com"
        );

        // None of them is: the first, to name the account that signed in.
        let other = id_token(serde_json::json!({
            "email": "grace@contoso.com", "preferred_username": "grace.h@contoso.com"
        }));
        assert_eq!(
            from_id_token(&other, Some(&asked)).unwrap().address.key(),
            "grace@contoso.com"
        );
    }

    #[test]
    fn expiry_is_a_number_or_a_string() {
        let t: Tokens =
            serde_json::from_str(r#"{"access_token":"a","expires_in":"3599"}"#).unwrap();
        assert_eq!(t.expires_in, 3599);
        let t: Tokens = serde_json::from_str(r#"{"access_token":"a","expires_in":60}"#).unwrap();
        assert_eq!(t.expires_in, 60);
        assert!(!format!("{t:?}").contains("\"a\""));
    }
}
