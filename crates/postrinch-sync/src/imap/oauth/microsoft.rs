//! Microsoft's OAuth for IMAP and SMTP (Outlook.com, Hotmail, Live and
//! Microsoft 365): the client file, the endpoints and scopes, and what the
//! identity platform's errors mean.

use std::path::{Path, PathBuf};

use postrinch_model::{EmailAddress, SignInReason};
use serde::Deserialize;

use crate::oauth::{Identity, Names, OAuthEndpoints, OAuthSpec, RedirectHost, TokenError};
use crate::{SyncError, SyncResult};

/// The `common` tenant: personal and work or school accounts alike.
const AUTHORIZE: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";
const TOKEN: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";

const IMAP_SCOPE: &str = "https://outlook.office.com/IMAP.AccessAsUser.All";
const SMTP_SCOPE: &str = "https://outlook.office.com/SMTP.Send";
const SCOPES: &[&str] = &[IMAP_SCOPE, SMTP_SCOPE, "offline_access", "openid", "email"];
const REQUIRED_SCOPES: &[&str] = &[IMAP_SCOPE, SMTP_SCOPE];

const NAMES: Names = Names {
    provider: "Microsoft",
    token: "Microsoft sign-in",
    userinfo: "Microsoft userinfo",
    revoke: "Microsoft revoke",
};

/// Postrinch's Microsoft client: a public "Mobile and desktop" app
/// registered in Microsoft Entra, with no secret. Read from
/// `<config dir>/postrinch/microsoft-oauth-client.json`:
///
/// ```json
/// {"client_id": "00000000-0000-0000-0000-000000000000"}
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicrosoftClient {
    pub client_id: String,
    authorize: String,
    token: String,
}

#[derive(Deserialize)]
struct File {
    client_id: String,
}

impl MicrosoftClient {
    /// The client file's name in Postrinch's config directory.
    pub const FILE_NAME: &str = "microsoft-oauth-client.json";

    /// The client, with Microsoft's endpoints.
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            authorize: AUTHORIZE.into(),
            token: TOKEN.into(),
        }
    }

    /// The same client with other endpoints, as tests point at a mock.
    pub fn with_endpoints(self, authorize: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            authorize: authorize.into(),
            token: token.into(),
            ..self
        }
    }

    /// Where the client file is looked for:
    /// `<config dir>/postrinch/microsoft-oauth-client.json`.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("postrinch").join(Self::FILE_NAME))
    }

    /// The client in the file at `path`, or `None` when there is no file.
    /// A file that does not read is an error.
    pub fn load(path: &Path) -> SyncResult<Option<Self>> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(SyncError::Config(format!("{}: {e}", path.display()))),
        };
        Self::parse(&text)
            .map(Some)
            .map_err(|e| SyncError::Config(format!("{}: {e}", path.display())))
    }

    fn parse(text: &str) -> Result<Self, String> {
        let file: File =
            serde_json::from_str(text).map_err(|e| format!("not a Microsoft client file ({e})"))?;
        let id = file.client_id.trim();
        if id.is_empty() {
            return Err("its client_id is empty".into());
        }
        Ok(Self::new(id))
    }

    pub(crate) fn spec(&self) -> OAuthSpec {
        OAuthSpec {
            names: NAMES,
            endpoints: OAuthEndpoints {
                authorize: self.authorize.clone(),
                token: self.token.clone(),
                userinfo: None,
                revoke: None,
            },
            client_id: self.client_id.clone(),
            client_secret: None,
            scopes: SCOPES,
            required_scopes: REQUIRED_SCOPES,
            redirect: RedirectHost::Localhost,
            identity: Identity::IdToken,
            scope_in_token_requests: true,
            authorize_params,
            classify_error: classify,
        }
    }
}

/// With a login hint Microsoft goes straight to that account; without, it
/// asks which.
fn authorize_params(login_hint: Option<&EmailAddress>) -> Vec<(&'static str, String)> {
    let mut params = vec![("response_mode", "query".to_owned())];
    if login_hint.is_none() {
        params.push(("prompt", "select_account".to_owned()));
    }
    params
}

/// What a refusal from Microsoft's token endpoint means, by its AADSTS
/// code where it gives one (`error_codes`, or the description's
/// `AADSTS<n>:` prefix), and otherwise by its OAuth error.
fn classify(status: u16, e: TokenError) -> SyncError {
    let code = e
        .error_codes
        .first()
        .copied()
        .or_else(|| aadsts_code(&e.error_description));
    let detail = e.detail();
    let rejected = || {
        SyncError::NeedsSignIn(SignInReason::CredentialRejected {
            detail: detail.clone(),
        })
    };
    match code {
        // 90094, 90095: the grant needs an administrator's consent.
        // 53003: blocked by a Conditional Access policy. 50105: the
        // account is not assigned to the app in its organization.
        // 53000, 53001: the device is not managed or compliant.
        Some(90094 | 90095 | 53003 | 50105 | 53000 | 53001) => {
            SyncError::NeedsSignIn(SignInReason::AdminPolicy { detail })
        }
        // 65001: the person (or admin) has not consented, or consent was
        // withdrawn. 65004: the person declined. 50076, 50079, 50158:
        // multi-factor or another challenge must be done interactively.
        // 700082, 70008, 70043: the refresh token expired (inactivity,
        // age, sign-in frequency). 50173: the grant was revoked (a
        // password change). 70000: the grant is invalid.
        Some(65001 | 65004 | 50076 | 50079 | 50158 | 700082 | 70008 | 70043 | 50173 | 70000) => {
            rejected()
        }
        // 7000218: the app is not registered as a public client. 700016:
        // no such app in the directory. 50011: the redirect URI is not
        // registered.
        Some(7000218 | 700016 | 50011) => SyncError::Config(format!("Microsoft client: {detail}")),
        _ => match e.error.as_str() {
            "invalid_grant" | "interaction_required" | "consent_required" => rejected(),
            "invalid_client" | "unauthorized_client" => {
                SyncError::Config(format!("Microsoft client: {detail}"))
            }
            _ => SyncError::api(NAMES.token, status, format!("{}: {detail}", e.error)),
        },
    }
}

/// The number in a description's leading `AADSTS<n>:`.
fn aadsts_code(description: &str) -> Option<u64> {
    let rest = description.trim_start().strip_prefix("AADSTS")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::OAuthFlow;

    fn flow() -> OAuthFlow {
        OAuthFlow::new(MicrosoftClient::new("id").spec(), reqwest::Client::new())
    }

    fn reason(err: SyncError) -> SignInReason {
        match err {
            SyncError::NeedsSignIn(reason) => reason,
            other => panic!("expected NeedsSignIn, got {other:?}"),
        }
    }

    #[test]
    fn reads_the_client_file() {
        let c = MicrosoftClient::parse(r#"{"client_id":" abc-123 "}"#).unwrap();
        assert_eq!(c.client_id, "abc-123");
        assert_eq!(c.token, TOKEN);
        assert!(MicrosoftClient::parse(r#"{"client_id":""}"#).is_err());
        assert!(MicrosoftClient::parse(r#"{"installed":{}}"#).is_err());
    }

    #[test]
    fn a_missing_file_is_no_client() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MicrosoftClient::FILE_NAME);
        assert_eq!(MicrosoftClient::load(&path).unwrap(), None);
        std::fs::write(&path, "{").unwrap();
        assert!(matches!(
            MicrosoftClient::load(&path),
            Err(SyncError::Config(_))
        ));
    }

    #[test]
    fn classifies_aadsts_codes() {
        let flow = flow();
        let expired = r#"{"error":"invalid_grant","error_description":"AADSTS700082: The refresh token has expired due to inactivity.\r\nTrace ID: t\r\nCorrelation ID: c","error_codes":[700082]}"#;
        assert_eq!(
            reason(flow.token_error(400, expired)),
            SignInReason::CredentialRejected {
                detail: "AADSTS700082: The refresh token has expired due to inactivity.".into()
            }
        );
        // The code read from the description when `error_codes` is absent.
        let admin = r#"{"error":"invalid_grant","error_description":"AADSTS90094: The grant requires admin permission."}"#;
        assert_eq!(
            reason(flow.token_error(400, admin)),
            SignInReason::AdminPolicy {
                detail: "AADSTS90094: The grant requires admin permission.".into()
            }
        );
        let mfa = r#"{"error":"interaction_required","error_description":"AADSTS50076: Due to a configuration change…","error_codes":[50076]}"#;
        assert!(matches!(
            reason(flow.token_error(400, mfa)),
            SignInReason::CredentialRejected { .. }
        ));
        let public = r#"{"error":"invalid_client","error_description":"AADSTS7000218: The request body must contain client_secret.","error_codes":[7000218]}"#;
        assert!(matches!(
            flow.token_error(401, public),
            SyncError::Config(_)
        ));
    }

    #[test]
    fn server_trouble_is_to_try_again() {
        let flow = flow();
        let busy = r#"{"error":"temporarily_unavailable","error_description":"AADSTS50196: busy","error_codes":[50196]}"#;
        assert!(matches!(
            flow.token_error(400, busy),
            SyncError::Api { status: 400, .. }
        ));
        let down = r#"{"error":"invalid_grant","error_description":"AADSTS700082: x"}"#;
        assert!(matches!(
            flow.token_error(503, down),
            SyncError::Api { status: 503, .. }
        ));
        assert!(matches!(
            flow.token_error(502, "<html>"),
            SyncError::Api { status: 502, .. }
        ));
    }

    #[test]
    fn asks_for_imap_and_smtp_and_a_refresh_token() {
        let spec = MicrosoftClient::new("id").spec();
        assert_eq!(
            spec.scope(),
            "https://outlook.office.com/IMAP.AccessAsUser.All \
             https://outlook.office.com/SMTP.Send offline_access openid email"
        );
        assert!(spec.client_secret.is_none());
        assert_eq!(aadsts_code("AADSTS65001: no"), Some(65001));
        assert_eq!(aadsts_code("nothing"), None);
    }
}
