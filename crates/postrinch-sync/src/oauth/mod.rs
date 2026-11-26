//! The OAuth 2 installed-app flow, for any provider: PKCE (RFC 7636), the
//! system browser, a loopback redirect (RFC 8252), the code exchange,
//! refresh and revocation. What differs between providers (endpoints,
//! client, scopes, how the signed-in address is learnt, what their errors
//! mean) is an [`OAuthSpec`]; Google's are in `google::oauth`, Microsoft's
//! in `imap::oauth::microsoft`.

mod browser;
mod flow;
mod http;
mod loopback;
mod pkce;
mod spec;
mod tokens;

pub(crate) use browser::{Granted, browser_sign_in};
pub(crate) use flow::OAuthFlow;
pub use http::OAuthTimeouts;
pub(crate) use http::http_client;
pub(crate) use loopback::RedirectHost;
pub(crate) use spec::{AuthorizeParams, Identity, Names, OAuthEndpoints, OAuthSpec, TokenError};
pub(crate) use tokens::{Identified, Tokens};

/// The first line of a provider's message, trimmed. Microsoft's go on
/// with trace and correlation ids and a timestamp, line by line.
pub(crate) fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_owned()
}

/// The scopes of `required` that `granted` (space-separated, as token
/// responses list them) leaves out. Scopes compare without regard to case,
/// as Microsoft treats them.
pub(crate) fn missing_scopes(required: &[&str], granted: &str) -> Vec<String> {
    required
        .iter()
        .filter(|r| {
            !granted
                .split_whitespace()
                .any(|g| g.eq_ignore_ascii_case(r))
        })
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_lines() {
        assert_eq!(
            first_line("AADSTS70008: Expired.\r\nTrace ID: 1\r\nTimestamp: 2"),
            "AADSTS70008: Expired."
        );
        assert_eq!(first_line(""), "");
    }

    #[test]
    fn missing_scopes_ignore_case_and_order() {
        let required = ["https://outlook.office.com/IMAP.AccessAsUser.All", "x"];
        assert_eq!(
            missing_scopes(
                &required,
                "X https://outlook.office.com/imap.accessasuser.all"
            ),
            Vec::<String>::new()
        );
        assert_eq!(missing_scopes(&required, "openid"), required);
    }
}
