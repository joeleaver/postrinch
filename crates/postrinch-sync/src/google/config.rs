//! The OAuth client Postrinch signs in to Google with.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{SyncError, SyncResult};

/// Postrinch's own client, given at build time in
/// `POSTRINCH_GOOGLE_CLIENT` as the JSON Google Cloud's "Download JSON"
/// writes. The Release workflow sets it from the repository secret of the
/// same name; other builds have none (or an empty one, as an unset secret
/// gives), and sign in only with a client file.
const BUILT_IN: Option<&str> = option_env!("POSTRINCH_GOOGLE_CLIENT");

/// A "Desktop app" OAuth client, as Google Cloud's "Download JSON" writes
/// it. A desktop client's secret is not secret (Google says so; PKCE is
/// what protects the flow), so release builds carry Postrinch's own
/// ([`BUILT_IN`]). It is kept out of the repository all the same, and a
/// client file still overrides it, so a person can bring their own.
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

    /// The client to sign in with: the client file at `path` when there
    /// is one, or else the one this build carries. `Ok(None)` when there is
    /// neither. A file that is there but does not read is an error, never
    /// passed over for the built-in client: a person who made a client of
    /// their own means it to be used.
    pub fn find(path: Option<&Path>) -> SyncResult<Option<Self>> {
        Self::find_with(path, BUILT_IN)
    }

    /// [`Self::find`] with `built_in` in place of this build's client.
    fn find_with(path: Option<&Path>, built_in: Option<&str>) -> SyncResult<Option<Self>> {
        if let Some(path) = path.filter(|p| p.exists()) {
            return Self::load(path).map(Some);
        }
        match built_in.filter(|text| !text.trim().is_empty()) {
            Some(text) => Self::parse(text)
                .map(Some)
                .map_err(|e| SyncError::Config(format!("the built-in client: {e}"))),
            None => Ok(None),
        }
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

    fn client_json(id: &str) -> String {
        format!(
            r#"{{"installed":{{"client_id":"{id}","client_secret":"s",
            "auth_uri":"https://accounts.google.com/o/oauth2/auth",
            "token_uri":"https://oauth2.googleapis.com/token"}}}}"#
        )
    }

    fn found_id(found: SyncResult<Option<ClientConfig>>) -> Option<String> {
        found.unwrap().map(|c| c.client_id)
    }

    #[test]
    fn a_client_file_overrides_the_built_in_client() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("google-oauth-client.json");
        std::fs::write(&file, client_json("file")).unwrap();
        let built_in = client_json("built-in");
        let found = ClientConfig::find_with(Some(&file), Some(&built_in));
        assert_eq!(found_id(found).as_deref(), Some("file"));
    }

    #[test]
    fn without_a_file_the_built_in_client_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("google-oauth-client.json");
        let built_in = client_json("built-in");
        let found = ClientConfig::find_with(Some(&file), Some(&built_in));
        assert_eq!(found_id(found).as_deref(), Some("built-in"));
        let found = ClientConfig::find_with(None, Some(&built_in));
        assert_eq!(found_id(found).as_deref(), Some("built-in"));
    }

    #[test]
    fn with_neither_there_is_no_client() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("google-oauth-client.json");
        assert_eq!(found_id(ClientConfig::find_with(Some(&file), None)), None);
        assert_eq!(found_id(ClientConfig::find_with(None, None)), None);
        // An unset repository secret builds in as an empty string.
        assert_eq!(found_id(ClientConfig::find_with(Some(&file), Some(""))), None);
        assert_eq!(found_id(ClientConfig::find_with(None, Some(" \n"))), None);
    }

    #[test]
    fn a_broken_file_is_an_error_not_passed_over() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("google-oauth-client.json");
        std::fs::write(&file, r#"{"web":{"client_id":"x"}}"#).unwrap();
        let built_in = client_json("built-in");
        let err = ClientConfig::find_with(Some(&file), Some(&built_in)).unwrap_err();
        assert!(err.to_string().contains("Desktop app"), "{err}");
    }

    #[test]
    fn a_broken_built_in_client_says_so() {
        let err = ClientConfig::find_with(None, Some("{}")).unwrap_err();
        assert!(err.to_string().contains("the built-in client"), "{err}");
    }

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
