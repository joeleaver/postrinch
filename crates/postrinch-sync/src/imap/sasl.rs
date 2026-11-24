//! The SASL messages Postrinch signs in with, for IMAP's AUTHENTICATE and
//! SMTP's AUTH: PLAIN (RFC 4616), XOAUTH2 (Google's, which Microsoft also
//! speaks) and OAUTHBEARER (RFC 7628).

/// A mechanism Postrinch uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mechanism {
    Plain,
    XOAuth2,
    OAuthBearer,
}

impl Mechanism {
    /// Its name, as servers advertise it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Mechanism::Plain => "PLAIN",
            Mechanism::XOAuth2 => "XOAUTH2",
            Mechanism::OAuthBearer => "OAUTHBEARER",
        }
    }

    /// The OAuth mechanism to use, given what the server offers: XOAUTH2,
    /// which every OAuth mail provider speaks, or else OAUTHBEARER.
    pub(crate) fn for_oauth(offers: impl Fn(&str) -> bool) -> Option<Self> {
        [Mechanism::XOAuth2, Mechanism::OAuthBearer]
            .into_iter()
            .find(|m| offers(m.name()))
    }

    /// What the client answers an error challenge with, before the server
    /// sends the failure: XOAUTH2 an empty response, OAUTHBEARER a single
    /// `%x01` (RFC 7628 3.2.3). PLAIN has no such challenge.
    pub(crate) fn error_answer(self) -> &'static [u8] {
        match self {
            Mechanism::Plain | Mechanism::XOAuth2 => b"",
            Mechanism::OAuthBearer => b"\x01",
        }
    }
}

/// PLAIN: no authorization identity, the user, the password.
pub(crate) fn plain(user: &str, password: &str) -> Vec<u8> {
    format!("\0{user}\0{password}").into_bytes()
}

/// XOAUTH2: `user=<u>^Aauth=Bearer <token>^A^A`.
pub(crate) fn xoauth2(user: &str, token: &str) -> Vec<u8> {
    format!("user={user}\x01auth=Bearer {token}\x01\x01").into_bytes()
}

/// OAUTHBEARER: a GS2 header naming the user, then the server's host and
/// port and the token.
pub(crate) fn oauthbearer(user: &str, host: &str, port: u16, token: &str) -> Vec<u8> {
    // RFC 5801: "," and "=" in the identity are written =2C and =3D.
    let user = user.replace('=', "=3D").replace(',', "=2C");
    format!("n,a={user},\x01host={host}\x01port={port}\x01auth=Bearer {token}\x01\x01").into_bytes()
}

/// The first message of an OAuth mechanism.
pub(crate) fn oauth_message(
    mechanism: Mechanism,
    user: &str,
    host: &str,
    port: u16,
    token: &str,
) -> Vec<u8> {
    match mechanism {
        Mechanism::OAuthBearer => oauthbearer(user, host, port, token),
        Mechanism::XOAuth2 | Mechanism::Plain => xoauth2(user, token),
    }
}

/// Answers async-imap's challenges: the message first, then, should the
/// server send an error challenge, the mechanism's answer to it.
pub(crate) struct Exchange {
    first: Option<Vec<u8>>,
    after: &'static [u8],
}

impl Exchange {
    pub(crate) fn new(mechanism: Mechanism, message: Vec<u8>) -> Self {
        Self {
            first: Some(message),
            after: mechanism.error_answer(),
        }
    }
}

impl async_imap::Authenticator for Exchange {
    type Response = Vec<u8>;

    fn process(&mut self, _challenge: &[u8]) -> Vec<u8> {
        self.first.take().unwrap_or_else(|| self.after.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_imap::Authenticator;

    #[test]
    fn messages() {
        assert_eq!(plain("ada", "pw"), b"\0ada\0pw");
        assert_eq!(
            xoauth2("ada", "tok"),
            b"user=ada\x01auth=Bearer tok\x01\x01"
        );
        assert_eq!(
            oauthbearer("a,b=c", "imap.example.org", 993, "tok"),
            b"n,a=a=2Cb=3Dc,\x01host=imap.example.org\x01port=993\x01auth=Bearer tok\x01\x01"
        );
    }

    #[test]
    fn the_exchange_answers_an_error_challenge() {
        let mut exchange = Exchange::new(Mechanism::OAuthBearer, b"hello".to_vec());
        assert_eq!(exchange.process(b""), b"hello");
        assert_eq!(exchange.process(b"{\"status\":\"invalid_token\"}"), b"\x01");
    }

    #[test]
    fn xoauth2_is_preferred() {
        let offers = |list: &'static [&'static str]| move |m: &str| list.contains(&m);
        assert_eq!(
            Mechanism::for_oauth(offers(&["OAUTHBEARER", "XOAUTH2"])),
            Some(Mechanism::XOAuth2)
        );
        assert_eq!(
            Mechanism::for_oauth(offers(&["OAUTHBEARER"])),
            Some(Mechanism::OAuthBearer)
        );
        assert_eq!(Mechanism::for_oauth(offers(&["PLAIN"])), None);
    }
}
