//! Reaching and signing in to an SMTP submission server with lettre: TLS
//! from the first byte (465) or after STARTTLS (587), EHLO, then AUTH.
//!
//! lettre's own AUTH loses a refused XOAUTH2's answer (Google sends an
//! error challenge first, which lettre takes for a protocol error) and
//! knows no OAUTHBEARER, so AUTH is spoken here over lettre's connection;
//! only the LOGIN exchange, for servers that offer nothing else, is
//! lettre's.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use lettre::transport::smtp::authentication::{Credentials, Mechanism as LettreMechanism};
use lettre::transport::smtp::client::AsyncSmtpConnection;
use lettre::transport::smtp::commands::Ehlo;
use lettre::transport::smtp::extension::ClientId;
use lettre::transport::smtp::response::Response;
use postrinch_model::{EmailAddress, ImapSettings, Security, Server, SignInReason};

use super::connect::{Connector, describe, within};
use super::sasl::{self, Mechanism};
use super::secret::{Credential, ImapSecret, Service};
use super::tokens::AccessTokens;
use crate::{SyncError, SyncResult};

/// A signed-in SMTP connection.
pub struct SmtpConnection {
    connection: AsyncSmtpConnection,
    server: Server,
}

impl std::fmt::Debug for SmtpConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpConnection")
            .field("server", &describe(&self.server))
            .finish_non_exhaustive()
    }
}

impl SmtpConnection {
    /// Connects to the account's SMTP server and signs in with `secret`,
    /// as the account's settings say. A refused sign-in (535, 534, or 432
    /// for a password to change) is [`SyncError::NeedsSignIn`] with the
    /// server's words; any other 4xx ("try again later"), at the greeting,
    /// EHLO, STARTTLS or AUTH, is [`SyncError::Busy`]; a server
    /// offering no encryption is [`SyncError::Insecure`], and gets no
    /// credentials.
    pub async fn open(
        connector: &Connector,
        account: &EmailAddress,
        settings: &ImapSettings,
        secret: &ImapSecret,
        tokens: &dyn AccessTokens,
    ) -> SyncResult<Self> {
        let credential = secret.credential(&settings.sign_in, Service::Smtp)?;
        let server = &settings.smtp;
        let mut connection = connect(connector, server).await?;
        let signed_in = connector
            .exchange(server, "signing in", async {
                sign_in(&mut connection, server, account, credential, tokens).await
            })
            .await;
        if let Err(e) = signed_in {
            connection.abort().await;
            return Err(e);
        }
        Ok(Self {
            connection,
            server: server.clone(),
        })
    }

    /// lettre's connection, for sending.
    pub fn inner(&mut self) -> &mut AsyncSmtpConnection {
        &mut self.connection
    }

    /// Says QUIT and closes the connection.
    pub async fn quit(mut self) -> SyncResult<()> {
        let quit = self.connection.quit().await;
        self.connection.abort().await;
        quit.map(drop).map_err(|e| server_error(&self.server, &e))
    }
}

/// The name Postrinch gives in EHLO: lettre's default, which is not this
/// machine's name (that would tell every server it).
fn hello() -> ClientId {
    ClientId::default()
}

async fn connect(connector: &Connector, server: &Server) -> SyncResult<AsyncSmtpConnection> {
    let parameters = connector.tls().smtp_parameters(&server.host)?;
    let timeout = Some(connector.timeouts().command);
    let address = (server.host.as_str(), server.port);
    let unreachable = |e: lettre::transport::smtp::Error| connect_error(server, e);
    let connect = async {
        match server.security {
            Security::Tls => AsyncSmtpConnection::connect_tokio1(
                address,
                timeout,
                &hello(),
                Some(parameters),
                None,
            )
            .await
            .map_err(unreachable),
            Security::StartTls => {
                let mut connection =
                    AsyncSmtpConnection::connect_tokio1(address, timeout, &hello(), None, None)
                        .await
                        .map_err(unreachable)?;
                if !connection.can_starttls() {
                    connection.abort().await;
                    return Err(SyncError::Insecure {
                        server: describe(server),
                    });
                }
                connection
                    .starttls(parameters, &hello())
                    .await
                    .map_err(unreachable)?;
                Ok(connection)
            }
        }
    };
    within(connector.timeouts().connect, server, "connecting", connect).await
}

/// Signs in on a connection that has said EHLO over TLS.
async fn sign_in(
    connection: &mut AsyncSmtpConnection,
    server: &Server,
    account: &EmailAddress,
    credential: Credential<'_>,
    tokens: &dyn AccessTokens,
) -> SyncResult<()> {
    // lettre keeps only the mechanisms it knows from EHLO; ask again to
    // see them all.
    let ehlo = connection
        .command(Ehlo::new(hello()))
        .await
        .map_err(|e| server_error(server, &e))?;
    let offered = mechanisms(&ehlo);
    let offers = |name: &str| offered.iter().any(|m| m == name);
    let user = &server.username;
    match credential {
        Credential::Password(password) if offers("PLAIN") => {
            let message = sasl::plain(user, password);
            authenticate(connection, server, Mechanism::Plain, &message).await
        }
        Credential::Password(password) if offers("LOGIN") => {
            let credentials = Credentials::new(user.clone(), password.to_owned());
            connection
                .auth(&[LettreMechanism::Login], &credentials)
                .await
                .map(drop)
                .map_err(|e| refusal(server, &e))
        }
        Credential::Password(_) => Err(SyncError::Server {
            service: "SMTP",
            server: describe(server),
            message: "the server offers no way to sign in with a password".into(),
        }),
        Credential::OAuth2 => {
            let mechanism = Mechanism::for_oauth(offers).ok_or_else(|| SyncError::Server {
                service: "SMTP",
                server: describe(server),
                message: "the server does not take OAuth sign-in".into(),
            })?;
            let mut refused_once = false;
            loop {
                let token = tokens.access_token(account).await?;
                let message =
                    sasl::oauth_message(mechanism, user, &server.host, server.port, &token);
                match authenticate(connection, server, mechanism, &message).await {
                    Err(SyncError::NeedsSignIn(SignInReason::CredentialRejected { .. }))
                        if !refused_once =>
                    {
                        // As for IMAP: one fresh token before giving up.
                        tokens.invalidate(account).await;
                        refused_once = true;
                    }
                    other => return other,
                }
            }
        }
    }
}

/// `AUTH <mechanism> <initial response>` (RFC 4954), answering an error
/// challenge as the mechanism says.
async fn authenticate(
    connection: &mut AsyncSmtpConnection,
    server: &Server,
    mechanism: Mechanism,
    message: &[u8],
) -> SyncResult<()> {
    let command = format!("AUTH {} {}\r\n", mechanism.name(), STANDARD.encode(message));
    let mut response = connection
        .command(command)
        .await
        .map_err(|e| refusal(server, &e))?;
    if response.has_code(334) {
        // The error challenge: why the token was refused (RFC 7628 3.2.2).
        let challenge = decoded_challenge(&response);
        let answer = format!("{}\r\n", STANDARD.encode(mechanism.error_answer()));
        response = connection
            .command(answer)
            .await
            .map_err(|e| refusal(server, &e))?;
        if response.has_code(334) {
            // The answer to an error challenge must end the exchange; a
            // server asking again has refused the sign-in all the same.
            // Cancel (RFC 4954 4: `*`, answered 501, which is expected
            // and says nothing more) so the connection can try again.
            let _cancelled = connection.command("*\r\n").await;
            return Err(SyncError::NeedsSignIn(SignInReason::CredentialRejected {
                detail: challenge.unwrap_or_else(|| "the server refused the sign-in".into()),
            }));
        }
    }
    if response.has_code(235) {
        Ok(())
    } else {
        Err(SyncError::Server {
            service: "SMTP",
            server: describe(server),
            message: response.message().collect::<Vec<_>>().join(" "),
        })
    }
}

/// The text of a 334 challenge, base64-decoded, when it is text.
fn decoded_challenge(response: &Response) -> Option<String> {
    let encoded = response.message().next()?.trim();
    let text = String::from_utf8(STANDARD.decode(encoded).ok()?).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The SASL mechanisms an EHLO answer offers, upper-cased.
fn mechanisms(ehlo: &Response) -> Vec<String> {
    ehlo.message()
        .filter_map(|line| {
            let upper = line.to_ascii_uppercase();
            // "AUTH PLAIN LOGIN", or the old "AUTH=PLAIN LOGIN".
            let rest = upper.strip_prefix("AUTH")?;
            rest.starts_with([' ', '=']).then(|| {
                rest[1..]
                    .split_ascii_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
        })
        .flatten()
        .collect()
}

/// The server's words in a lettre error, less any enhanced status code
/// (`5.7.8 Authentication credentials invalid`).
fn reply_text(error: &lettre::transport::smtp::Error) -> String {
    use std::error::Error;
    let text = error
        .source()
        .map_or_else(|| error.to_string(), |s| s.to_string());
    let words = text.trim();
    match words.split_once(' ') {
        Some((first, rest)) if is_enhanced_code(first) => rest.to_owned(),
        _ => words.to_owned(),
    }
}

/// RFC 3463: `class.subject.detail`, as `5.7.8`.
fn is_enhanced_code(word: &str) -> bool {
    let parts: Vec<_> = word.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// A refused AUTH: 535 (credentials invalid), 534 (this way of signing
/// in is not allowed, as Google's "application-specific password
/// required") and 432 (RFC 4954 6, `432 4.7.12 A password transition is
/// needed`: the person must change the password before it is taken
/// again) are the account's to fix, with the server's words. Any other
/// 4xx is the server busy ([`server_error`]); anything else is the
/// server's.
fn refusal(server: &Server, error: &lettre::transport::smtp::Error) -> SyncError {
    match error.status().map(u16::from) {
        Some(432 | 534 | 535) => SyncError::NeedsSignIn(SignInReason::CredentialRejected {
            detail: reply_text(error),
        }),
        _ => server_error(server, error),
    }
}

/// An answer the server gave, or a TLS failure. A 4xx reply (RFC 5321
/// 4.2.1: "the action may be requested again") is [`SyncError::Busy`]:
/// Gmail's `421 4.7.0 Try again later, closing connection`, `454 4.7.0
/// Too many sign-in attempts, try again later` and `454 4.7.0 Cannot
/// sign in due to a temporary system problem` (Google Workspace, "Gmail
/// SMTP errors and codes",
/// <https://knowledge.workspace.google.com/admin/support/troubleshooting/gmail-smtp-errors-and-codes>),
/// at the greeting, EHLO, STARTTLS (`454 4.7.0 TLS not available due to
/// temporary reason`, RFC 3207 4) or AUTH alike. None of them is the
/// credential's fault, so none is retried with a fresh token. (AUTH's 432
/// is the person's to act on; [`refusal`] takes it first.)
fn server_error(server: &Server, error: &lettre::transport::smtp::Error) -> SyncError {
    if error.is_tls() {
        return SyncError::Tls {
            server: describe(server),
            detail: reply_text(error),
        };
    }
    if error.is_transient() {
        return SyncError::Busy {
            service: "SMTP",
            server: describe(server),
            message: reply_text(error),
        };
    }
    SyncError::Server {
        service: "SMTP",
        server: describe(server),
        message: reply_text(error),
    }
}

/// A failure while connecting: an answer from the server is its own; a
/// failure to reach it says so.
fn connect_error(server: &Server, error: lettre::transport::smtp::Error) -> SyncError {
    if error.status().is_some() || error.is_tls() || error.is_response() {
        return server_error(server, &error);
    }
    SyncError::Unreachable {
        server: describe(server),
        detail: reply_text(&error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enhanced_codes() {
        assert!(is_enhanced_code("5.7.8"));
        assert!(!is_enhanced_code("5.7"));
        assert!(!is_enhanced_code("Authentication"));
    }
}
