//! The OAuth client Postrinch signs in to Google with.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{SyncError, SyncResult};

/// A "Desktop app" OAuth client, as Google Cloud's "Download JSON" writes
/// it. A desktop client's secret is not secret (Google says so; PKCE is
/// what protects the flow), but it is kept out of the repository all the
/// same, so a person can bring their own client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    pub client_id: String,
    pub client_secret: String,
    pub auth_uri: String,
    pub token_uri: String,
}

#[derive(Deserialize)]
struct File {
    installed: Installed,
}

#[derive(Deserialize)]
struct Installed {
    client_id: String,
    client_secret: String,
    auth_uri: String,
    token_uri: String,
}

impl ClientConfig {
    /// Where the client file is looked for when no path is given:
    /// `<config dir>/postrinch/google-oauth-client.json`.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("postrinch").join("google-oauth-client.json"))
    }

    pub fn load(path: &Path) -> SyncResult<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| SyncError::Config(format!("{}: {e}", path.display())))?;
        Self::parse(&text).map_err(|e| SyncError::Config(format!("{}: {e}", path.display())))
    }

    fn parse(text: &str) -> Result<Self, String> {
        let file: File = serde_json::from_str(text)
            .map_err(|e| format!("not a Desktop app client file ({e})"))?;
        let i = file.installed;
        Ok(Self {
            client_id: i.client_id,
            client_secret: i.client_secret,
            auth_uri: i.auth_uri,
            token_uri: i.token_uri,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_googles_installed_format() {
        let text = r#"{"installed":{"client_id":"id.apps.googleusercontent.com","project_id":"p",
            "auth_uri":"https://accounts.google.com/o/oauth2/auth","token_uri":"https://oauth2.googleapis.com/token",
            "auth_provider_x509_cert_url":"x","client_secret":"s","redirect_uris":["http://localhost"]}}"#;
        let c = ClientConfig::parse(text).unwrap();
        assert_eq!(c.client_id, "id.apps.googleusercontent.com");
        assert_eq!(c.token_uri, "https://oauth2.googleapis.com/token");
    }

    #[test]
    fn refuses_a_web_client() {
        let err = ClientConfig::parse(r#"{"web":{"client_id":"x"}}"#).unwrap_err();
        assert!(err.contains("Desktop app"), "{err}");
    }
}
