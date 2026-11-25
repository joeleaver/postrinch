//! Signing in to IMAP: AUTHENTICATE PLAIN (or LOGIN where PLAIN is not
//! offered) with a password; AUTHENTICATE XOAUTH2 or OAUTHBEARER with an
//! OAuth access token, fetched afresh and tried once more when the first
//! is refused.
//!
//! A refusal of the credential becomes the account's "needs to sign in"
//! state with the server's own sentence; RFC 5530's response codes say
//! whether it was the credential at all ([`RefusalKind::of`]).

use postrinch_model::{EmailAddress, Server, SignInReason};

use super::connect::{Client, Connector, Greeted, describe};
use super::reply::{Recorder, Status, Tagged};
use super::sasl::{self, Exchange, Mechanism};
use super::secret::Credential;
use super::tokens::AccessTokens;
use crate::{SyncError, SyncResult};

/// A signed-in IMAP session as async-imap gives it.
pub(crate) type Signed = async_imap::Session<super::reply::ImapStream>;

/// Why one attempt to sign in did not.
enum Refused {
    /// The server refused the credential: a fresh token may do.
    Credential(SyncError),
    /// Anything else: the connection, the server failing.
    Other(SyncError),
}

/// Signs in on a connection whose greeting was read.
pub(crate) async fn sign_in(
    connector: &Connector,
    greeted: Greeted,
    server: &Server,
    account: &EmailAddress,
    credential: Credential<'_>,
    tokens: &dyn AccessTokens,
) -> SyncResult<Signed> {
    let Greeted {
        client,
        capabilities,
        recorder,
    } = greeted;
    let attempt = Attempt {
        connector,
        server,
        recorder: &recorder,
    };
    match credential {
        Credential::Password(password) => {
            let result = if capabilities.has_auth("PLAIN") {
                let message = sasl::plain(&server.username, password);
                let exchange = Exchange::new(Mechanism::Plain, message);
                attempt
                    .authenticate(client, Mechanism::Plain, exchange)
                    .await
            } else if !capabilities.login_disabled() {
                attempt.login(client, password).await
            } else {
                return Err(attempt.error("the server offers no way to sign in with a password"));
            };
            result.map_err(|(refused, _)| refused.into_error())
        }
        Credential::OAuth2 => {
            let mechanism = Mechanism::for_oauth(|m| capabilities.has_auth(m))
                .ok_or_else(|| attempt.error("the server does not take OAuth sign-in"))?;
            let mut client = client;
            let mut refused_once = false;
            loop {
                // Within the command timeout, as SMTP's whole sign-in is:
                // a provider that never answers must not hang the account.
                let token = connector
                    .exchange(
                        server,
                        "getting an access token",
                        tokens.access_token(account),
                    )
                    .await?;
                let message = sasl::oauth_message(
                    mechanism,
                    &server.username,
                    &server.host,
                    server.port,
                    &token,
                );
                let exchange = Exchange::new(mechanism, message);
                match attempt.authenticate(client, mechanism, exchange).await {
                    Ok(session) => return Ok(session),
                    Err((Refused::Credential(_), Some(returned))) if !refused_once => {
                        // A token can be refused before it expires (revoked,
                        // or the server's clock differs): one fresh try.
                        tokens.invalidate(account).await;
                        refused_once = true;
                        client = *returned;
                    }
                    Err((refused, _)) => return Err(refused.into_error()),
                }
            }
        }
    }
}

/// What a tagged NO to a sign-in means, by its RFC 5530 response code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefusalKind {
    /// The credential was refused: the account needs to sign in (and an
    /// OAuth token is fetched afresh once first).
    Credential,
    /// An administrator's policy refuses it: signing in again will not
    /// help, nor will a fresh token.
    Admin,
    /// The server could not, or would not, sign anyone in now: a server
    /// failure to try again later, not the account's to fix.
    Server,
}

impl RefusalKind {
    /// Only `AUTHENTICATIONFAILED`, `AUTHORIZATIONFAILED` (the credential
    /// is good but not for this user), `EXPIRED` (the password or token
    /// has expired) and a NO with no code say the credential was refused.
    /// `ALERT` (RFC 9051 7.1) only asks for the text to be shown, so it
    /// counts as no code: Gmail's "[ALERT] Application-specific password
    /// required" is a refused password. `CONTACTADMIN` is the admin's.
    /// Every other code, RFC 5530's temporary ones (`UNAVAILABLE`,
    /// `INUSE`, `LIMIT`, `SERVERBUG`, `CORRUPTION`), `PRIVACYREQUIRED`
    /// (Postrinch signs in only over TLS, so the server is at fault), the
    /// rest and any unknown code, is the server's.
    fn of(code: Option<&str>) -> Self {
        match code {
            None | Some("ALERT" | "AUTHENTICATIONFAILED" | "AUTHORIZATIONFAILED" | "EXPIRED") => {
                Self::Credential
            }
            Some("CONTACTADMIN") => Self::Admin,
            Some(_) => Self::Server,
        }
    }
}

impl Refused {
    fn into_error(self) -> SyncError {
        match self {
            Refused::Credential(e) | Refused::Other(e) => e,
        }
    }
}

/// How an attempt ended when it did not sign in: why, and the client to
/// try again on, unless the connection is lost.
type Failed = (Refused, Option<Box<Client>>);

/// One server's sign-in.
struct Attempt<'a> {
    connector: &'a Connector,
    server: &'a Server,
    recorder: &'a Recorder,
}

impl Attempt<'_> {
    fn error(&self, message: impl Into<String>) -> SyncError {
        SyncError::Server {
            service: "IMAP",
            server: describe(self.server),
            message: message.into(),
        }
    }

    async fn authenticate(
        &self,
        client: Client,
        mechanism: Mechanism,
        exchange: Exchange,
    ) -> Result<Signed, Failed> {
        self.recorder.start();
        let result = self
            .connector
            .exchange(self.server, "signing in", async {
                Ok(client.authenticate(mechanism.name(), exchange).await)
            })
            .await;
        self.finish(result)
    }

    async fn login(&self, client: Client, password: &str) -> Result<Signed, Failed> {
        self.recorder.start();
        let user = &self.server.username;
        let result = self
            .connector
            .exchange(self.server, "signing in", async {
                Ok(client.login(user, password).await)
            })
            .await;
        self.finish(result)
    }

    /// Makes sense of how a sign-in ended. A timeout loses the client.
    fn finish(
        &self,
        result: SyncResult<Result<Signed, (async_imap::error::Error, Client)>>,
    ) -> Result<Signed, Failed> {
        let recorded = self.recorder.stop();
        match result {
            Err(timeout) => Err((Refused::Other(timeout), None)),
            Ok(Ok(session)) => Ok(session),
            Ok(Err((error, client))) => {
                let answer = Tagged::last_in(&recorded);
                Err((self.refusal(error, answer), Some(Box::new(client))))
            }
        }
    }

    fn refusal(&self, error: async_imap::error::Error, answer: Option<Tagged>) -> Refused {
        use async_imap::error::Error;
        match (error, answer) {
            (Error::No(_), Some(answer)) if answer.status == Status::No => {
                let detail = if answer.text.is_empty() {
                    answer.code.clone().unwrap_or_else(|| "refused".into())
                } else {
                    answer.text.clone()
                };
                match RefusalKind::of(answer.code.as_deref()) {
                    RefusalKind::Credential => Refused::Credential(SyncError::NeedsSignIn(
                        SignInReason::CredentialRejected { detail },
                    )),
                    // A fresh token would not change the admin's mind.
                    RefusalKind::Admin => {
                        Refused::Other(SyncError::NeedsSignIn(SignInReason::AdminPolicy { detail }))
                    }
                    RefusalKind::Server => Refused::Other(self.error(detail)),
                }
            }
            (Error::No(_) | Error::Bad(_), answer) => Refused::Other(
                self.error(answer.map_or_else(|| "sign-in refused".into(), |a| a.text)),
            ),
            (Error::Io(e), _) => Refused::Other(e.into()),
            (Error::ConnectionLost, _) => {
                Refused::Other(self.error("the server closed the connection"))
            }
            (other, _) => Refused::Other(self.error(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RefusalKind::{self, Admin, Credential, Server};

    #[test]
    fn only_credential_codes_and_no_code_refuse_the_credential() {
        let cases = [
            (None, Credential),
            (Some("AUTHENTICATIONFAILED"), Credential),
            (Some("AUTHORIZATIONFAILED"), Credential),
            (Some("EXPIRED"), Credential),
            (Some("ALERT"), Credential),
            (Some("CONTACTADMIN"), Admin),
            (Some("UNAVAILABLE"), Server),
            (Some("INUSE"), Server),
            (Some("LIMIT"), Server),
            (Some("SERVERBUG"), Server),
            (Some("CORRUPTION"), Server),
            (Some("PRIVACYREQUIRED"), Server),
            (Some("CANNOT"), Server),
            (Some("SOMETHINGNEW"), Server),
        ];
        for (code, kind) in cases {
            assert_eq!(RefusalKind::of(code), kind, "{code:?}");
        }
    }
}
