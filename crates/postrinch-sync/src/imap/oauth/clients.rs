//! Which OAuth clients this copy of Postrinch has for IMAP accounts.

use std::path::Path;

use super::{MicrosoftClient, OAuthProvider};
use crate::google::{ClientConfig, Endpoints};
use crate::oauth::OAuthSpec;
use crate::{SyncError, SyncResult};

/// One provider's client, or why there is none.
#[derive(Clone)]
enum Client {
    Ready(Box<OAuthSpec>),
    /// No client file (`None`), or one that does not read.
    Missing(Option<String>),
}

/// The OAuth clients IMAP accounts sign in with: Google's (the one Google
/// accounts use, asking for mail alone) and Microsoft's. A provider whose
/// client is missing cannot sign in, and says so
/// ([`SyncError::OAuthNotSetUp`]); the others are unaffected.
#[derive(Clone)]
pub struct OAuthClients {
    google: Client,
    microsoft: Client,
}

impl Default for OAuthClients {
    fn default() -> Self {
        Self::none()
    }
}

impl OAuthClients {
    /// No clients at all.
    pub fn none() -> Self {
        Self {
            google: Client::Missing(None),
            microsoft: Client::Missing(None),
        }
    }

    /// The clients in Postrinch's config directory
    /// (`<config dir>/postrinch`).
    pub fn load() -> Self {
        match dirs::config_dir() {
            Some(dir) => Self::load_from(&dir.join("postrinch")),
            None => Self::none(),
        }
    }

    /// The clients in `dir`: `google-oauth-client.json` and
    /// `microsoft-oauth-client.json`. A file that is there but does not
    /// read leaves its provider not set up, with the reason, and is
    /// logged.
    pub fn load_from(dir: &Path) -> Self {
        let google_path = dir.join("google-oauth-client.json");
        let google = if google_path.exists() {
            match ClientConfig::load(&google_path) {
                Ok(config) => ready(google_spec(&config, Endpoints::google(&config))),
                Err(e) => missing_because(e),
            }
        } else {
            Client::Missing(None)
        };
        let microsoft = match MicrosoftClient::load(&dir.join(MicrosoftClient::FILE_NAME)) {
            Ok(Some(client)) => ready(client.spec()),
            Ok(None) => Client::Missing(None),
            Err(e) => missing_because(e),
        };
        Self { google, microsoft }
    }

    /// Google over IMAP with `config`, Google's own client.
    pub fn with_google(self, config: &ClientConfig) -> Self {
        self.with_google_endpoints(config, Endpoints::google(config))
    }

    /// Google over IMAP with `config` and other endpoints, as tests point
    /// at a mock.
    pub fn with_google_endpoints(self, config: &ClientConfig, endpoints: Endpoints) -> Self {
        Self {
            google: ready(google_spec(config, endpoints)),
            ..self
        }
    }

    pub fn with_microsoft(self, client: &MicrosoftClient) -> Self {
        Self {
            microsoft: ready(client.spec()),
            ..self
        }
    }

    /// Whether accounts with `provider` can sign in here.
    pub fn is_set_up(&self, provider: OAuthProvider) -> bool {
        matches!(self.client(provider), Client::Ready(_))
    }

    pub(crate) fn spec(&self, provider: OAuthProvider) -> SyncResult<&OAuthSpec> {
        match self.client(provider) {
            Client::Ready(spec) => Ok(spec),
            Client::Missing(detail) => Err(SyncError::OAuthNotSetUp {
                provider: provider.name().to_owned(),
                detail: detail.clone(),
            }),
        }
    }

    fn client(&self, provider: OAuthProvider) -> &Client {
        match provider {
            OAuthProvider::Google => &self.google,
            OAuthProvider::Microsoft => &self.microsoft,
        }
    }
}

fn google_spec(config: &ClientConfig, endpoints: Endpoints) -> OAuthSpec {
    crate::google::imap_spec(config, endpoints)
}

fn ready(spec: OAuthSpec) -> Client {
    Client::Ready(Box::new(spec))
}

fn missing_because(e: SyncError) -> Client {
    tracing::warn!("an OAuth client file does not read: {e}");
    Client::Missing(Some(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_files_nothing_is_set_up() {
        let dir = tempfile::tempdir().unwrap();
        let clients = OAuthClients::load_from(dir.path());
        for p in OAuthProvider::ALL {
            assert!(!clients.is_set_up(*p));
        }
        let err = clients.spec(OAuthProvider::Microsoft).err().unwrap();
        assert_eq!(
            err.to_string(),
            "Microsoft sign-in isn't set up in this copy of Postrinch"
        );
    }

    #[test]
    fn a_broken_file_says_why() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(MicrosoftClient::FILE_NAME), "{}").unwrap();
        std::fs::write(
            dir.path().join("google-oauth-client.json"),
            r#"{"installed":{"client_id":"g","client_secret":"s",
                "auth_uri":"https://a","token_uri":"https://t"}}"#,
        )
        .unwrap();
        let clients = OAuthClients::load_from(dir.path());
        assert!(clients.is_set_up(OAuthProvider::Google));
        match clients.spec(OAuthProvider::Microsoft) {
            Err(SyncError::OAuthNotSetUp {
                provider,
                detail: Some(detail),
            }) => {
                assert_eq!(provider, "Microsoft");
                assert!(detail.contains("client_id"), "{detail}");
            }
            _ => panic!("expected Microsoft not set up, with the reason"),
        }
    }
}
