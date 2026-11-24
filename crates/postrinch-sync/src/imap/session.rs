//! A signed-in IMAP connection, what sync builds on: connected over TLS,
//! signed in, its capabilities known.

use futures_util::TryStreamExt;
use postrinch_model::{EmailAddress, ImapSettings, Server};

use super::auth::{self, Signed};
use super::capabilities::Capabilities;
use super::connect::{Connector, describe};
use super::folders::ListedFolder;
use super::reply::ImapStream;
use super::secret::{ImapSecret, Service};
use super::tokens::AccessTokens;
use crate::{SyncError, SyncResult};

/// A signed-in IMAP session.
pub struct ImapSession {
    session: Signed,
    capabilities: Capabilities,
    server: Server,
    connector: Connector,
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
    /// [`SyncError::NeedsSignIn`] with the server's words; a server
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
        let mut session =
            auth::sign_in(connector, greeted, server, account, credential, tokens).await?;
        // Asked again: what a server offers changes once signed in.
        let capabilities = connector
            .exchange(server, "asking its capabilities", async {
                session
                    .capabilities()
                    .await
                    .map_err(|e| imap_error(server, e))
            })
            .await?;
        Ok(Self {
            capabilities: Capabilities::from_async_imap(&capabilities),
            session,
            server: server.clone(),
            connector: connector.clone(),
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

    /// The async-imap session, for the commands sync sends.
    pub fn inner(&mut self) -> &mut async_imap::Session<ImapStream> {
        &mut self.session
    }

    /// Every folder (`LIST "" *`), with SPECIAL-USE attributes where the
    /// server offers them.
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
        self.connector
            .exchange(server, "listing folders", async {
                let names = session
                    .list(Some(""), Some(pattern))
                    .await
                    .map_err(|e| imap_error(server, e))?;
                let names: Vec<_> = names
                    .try_collect()
                    .await
                    .map_err(|e| imap_error(server, e))?;
                Ok(names.iter().map(ListedFolder::from_name).collect())
            })
            .await
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
