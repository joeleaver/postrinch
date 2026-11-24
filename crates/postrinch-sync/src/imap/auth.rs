//! Signing in to IMAP: AUTHENTICATE PLAIN (or LOGIN where PLAIN is not
//! offered) with a password; AUTHENTICATE XOAUTH2 or OAUTHBEARER with an
//! OAuth access token, fetched afresh and tried once more when the first
//! is refused.
//!
//! A refusal becomes the account's "needs to sign in" state with the
//! server's own sentence (RFC 5530's codes say which kind it is).

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
                let token = tokens.access_token(account).await?;
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
                match answer.code.as_deref() {
                    // RFC 5530: the server cannot check now; try later.
                    Some("UNAVAILABLE") => Refused::Other(self.error(detail)),
                    Some("CONTACTADMIN") => {
                        // A fresh token would not change the admin's mind.
                        Refused::Other(SyncError::NeedsSignIn(SignInReason::AdminPolicy { detail }))
                    }
                    _ => Refused::Credential(SyncError::NeedsSignIn(
                        SignInReason::CredentialRejected { detail },
                    )),
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
