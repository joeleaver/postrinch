//! A signed-in IMAP connection, what sync builds on: connected over TLS,
//! signed in, its capabilities known.

use futures_util::TryStreamExt;
use postrinch_model::{EmailAddress, ImapSettings, Server};

use super::auth::{self, Signed};
use super::capabilities::Capabilities;
use super::connect::{Connector, describe};
use super::folders::ListedFolder;
use super::refusal::is_temporary_limit;
use super::reply::{ImapStream, Recorder, Status, Tagged};
use super::secret::{ImapSecret, Service};
use super::tokens::AccessTokens;
use crate::{SyncError, SyncResult};

/// A signed-in IMAP session.
pub struct ImapSession {
    session: Signed,
    capabilities: Capabilities,
    server: Server,
    connector: Connector,
    /// Records the answers to the first commands after sign-in, for
    /// [`after_sign_in`].
    recorder: Recorder,
}

impl std::fmt::Debug for ImapSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImapSession")
            .field("server", &describe(&self.server))
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

impl ImapSession {
    /// Connects to the account's IMAP server and signs in with `secret`,
    /// as the account's settings say. A refused sign-in is
    /// [`SyncError::NeedsSignIn`] with the server's words; a server too
    /// busy for the account now (a connection or bandwidth limit, its
    /// sign-in service down, also when it says so with a `BYE` in place
    /// of the greeting or a `BAD` to the CAPABILITY after signing in) is
    /// [`SyncError::Busy`]; a server
    /// offering no encryption is [`SyncError::Insecure`], and gets no
    /// credentials.
    pub async fn open(
        connector: &Connector,
        account: &EmailAddress,
        settings: &ImapSettings,
        secret: &ImapSecret,
        tokens: &dyn AccessTokens,
    ) -> SyncResult<Self> {
        let credential = secret.credential(&settings.sign_in, Service::Imap)?;
        let server = &settings.imap;
        let greeted = connector.imap(server).await?;
        let recorder = greeted.recorder.clone();
        let mut session =
            auth::sign_in(connector, greeted, server, account, credential, tokens).await?;
        // Asked again: what a server offers changes once signed in.
        recorder.start();
        let answered = connector
            .exchange(server, "asking its capabilities", async {
                Ok(session.capabilities().await)
            })
            .await;
        let recorded = recorder.stop();
        let capabilities = after_sign_in(server, &recorded, answered?)?;
        Ok(Self {
            capabilities: Capabilities::from_async_imap(&capabilities),
            session,
            server: server.clone(),
            connector: connector.clone(),
            recorder,
        })
    }

    /// What the server offers, once signed in.
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// The server this session is with.
    pub fn server(&self) -> &Server {
        &self.server
    }

    /// How this session reaches its server: the TLS trust and timeouts.
    pub(crate) fn connector(&self) -> &Connector {
        &self.connector
    }

    /// The async-imap session, for the commands sync sends.
    pub fn inner(&mut self) -> &mut async_imap::Session<ImapStream> {
        &mut self.session
    }

    /// Every folder (`LIST "" *`), with SPECIAL-USE attributes where the
    /// server offers them. As one of the first commands after signing in,
    /// a `BAD` naming a known limit is [`SyncError::Busy`].
    pub async fn list_folders(&mut self) -> SyncResult<Vec<ListedFolder>> {
        // RFC 6154 5.1: RETURN (SPECIAL-USE) asks for the attributes; it
        // is LIST-EXTENDED syntax. Without it a SPECIAL-USE server may
        // still show them, and the plain form is all others take.
        let pattern = if self.capabilities.special_use() && self.capabilities.list_extended() {
            "* RETURN (SPECIAL-USE)"
        } else {
            "*"
        };
        let server = &self.server;
        let session = &mut self.session;
        self.recorder.start();
        let answered = self
            .connector
            .exchange(server, "listing folders", async {
                let names = match session.list(Some(""), Some(pattern)).await {
                    Ok(names) => names,
                    Err(e) => return Ok(Err(e)),
                };
                Ok(names.try_collect::<Vec<_>>().await)
            })
            .await;
        let recorded = self.recorder.stop();
        let names = after_sign_in(server, &recorded, answered?)?;
        Ok(names.iter().map(ListedFolder::from_name).collect())
    }

    /// Signs out and closes the connection.
    pub async fn logout(mut self) -> SyncResult<()> {
        let server = self.server.clone();
        self.connector
            .exchange(&server, "signing out", async {
                self.session
                    .logout()
                    .await
                    .map_err(|e| imap_error(&server, e))
            })
            .await
    }
}

/// How one of the first commands after signing in (CAPABILITY, LIST)
/// ended, from async-imap's `result` and what the server sent meanwhile
/// (`recorded`).
///
/// async-imap 0.12 reads CAPABILITY and LIST to their tagged answer
/// without looking at its status (`parse_capabilities`, `parse_names`),
/// so a `NO` or `BAD` would pass as an empty answer: the recorded tagged
/// answer is checked here, and anything but OK fails the command.
fn after_sign_in<T>(
    server: &Server,
    recorded: &[u8],
    result: Result<T, async_imap::error::Error>,
) -> SyncResult<T> {
    use async_imap::error::Error;
    let refused = Tagged::last_in(recorded).filter(|answer| answer.status != Status::Ok);
    match (result, refused) {
        (Ok(_) | Err(Error::No(_) | Error::Bad(_)), Some(answer)) => {
            Err(refused_after_sign_in(server, answer))
        }
        (Ok(value), None) => Ok(value),
        (Err(e), _) => Err(imap_error(server, e)),
    }
}

/// A `NO` or `BAD` to one of the first commands after signing in. A
/// `BAD` naming a known temporary limit is the server busy: Microsoft 365
/// accepts the sign-in and then answers the next command `BAD User is
/// authenticated but not connected.` while it sheds sessions opened too
/// fast ([`super::refusal::TEMPORARY_LIMITS`]). Anything else is the
/// server's, in its words.
fn refused_after_sign_in(server: &Server, answer: Tagged) -> SyncError {
    if answer.status == Status::Bad && is_temporary_limit(&answer.text) {
        return SyncError::Busy {
            service: "IMAP",
            server: describe(server),
            message: answer.text,
        };
    }
    SyncError::Server {
        service: "IMAP",
        server: describe(server),
        message: answer.text,
    }
}

/// An async-imap error after sign-in, naming the server.
pub(crate) fn imap_error(server: &Server, error: async_imap::error::Error) -> SyncError {
    use async_imap::error::Error;
    match error {
        Error::Io(e) => e.into(),
        other => SyncError::Server {
            service: "IMAP",
            server: describe(server),
            message: other.to_string(),
        },
    }
}
