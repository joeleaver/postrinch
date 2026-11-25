//! What the commands work with: the store, and the sync for every account
//! ([`MailSync`]), Google accounts' over the Gmail API when Google
//! sign-in is set up, IMAP accounts' always, with their secrets in the
//! keychain.

use std::path::PathBuf;
use std::sync::Arc;

use postrinch_model::Provider;
use postrinch_store::Store;
use postrinch_sync::google::{ClientConfig, GmailApi, GmailSync, GoogleAuth};
use postrinch_sync::imap::{Connector, ImapOAuth, ImapSecrets, ImapSync, OAuthClients, TlsConfig};
use postrinch_sync::{Keychain, MailSync};

use crate::commands::Result;

pub struct Context {
    pub store: Store,
    /// The Google OAuth client file given with --client.
    pub client: Option<PathBuf>,
}

impl Context {
    /// Google sign-in, from the client file.
    pub fn auth(&self) -> Result<Arc<GoogleAuth>> {
        let path = match &self.client {
            Some(path) => path.clone(),
            None => ClientConfig::default_path().ok_or("no config directory; pass --client")?,
        };
        let config = ClientConfig::load(&path)?;
        Ok(Arc::new(GoogleAuth::new(
            config,
            Arc::new(Keychain::GOOGLE),
            ImapSecrets::keychain(),
        )?))
    }

    /// IMAP accounts' browser sign-ins, which also give OAuth accounts
    /// their access tokens.
    pub fn imap_oauth(&self) -> Result<Arc<ImapOAuth>> {
        Ok(Arc::new(ImapOAuth::new(
            OAuthClients::load(),
            ImapSecrets::keychain(),
            Arc::new(Keychain::GOOGLE),
        )?))
    }

    /// IMAP accounts' sync, trusting the system's certificates.
    pub fn imap(&self, oauth: &Arc<ImapOAuth>) -> Result<ImapSync> {
        Ok(ImapSync::new(
            self.store.clone(),
            Connector::new(TlsConfig::platform()?),
            ImapSecrets::keychain(),
            oauth.clone(),
        ))
    }

    /// The sync for every account. Without Google sign-in (no client
    /// file), Google accounts are off and say so when synced; IMAP
    /// accounts work all the same, unless --client named a file that
    /// cannot be read.
    pub fn mail(&self) -> Result<MailSync> {
        let gmail = match self.auth() {
            Ok(auth) => Some(GmailSync::new(
                Arc::new(GmailApi::new(auth)),
                self.store.clone(),
            )),
            Err(e) if self.client.is_some() => return Err(e),
            Err(e) => {
                if self
                    .store
                    .accounts()?
                    .iter()
                    .any(|a| a.provider == Provider::Google)
                {
                    eprintln!("Google accounts are off: {e}");
                }
                None
            }
        };
        let imap = self.imap(&self.imap_oauth()?)?;
        Ok(MailSync::new(self.store.clone(), gmail, imap))
    }
}
