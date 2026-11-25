//! An IMAP account's secret: kept in the keychain under `postrinch.imap`,
//! keyed by the account's address, as JSON (docs/imap.md, "Credentials").
//!
//! ```json
//! {"kind":"password","password":"…"}
//! {"kind":"password","password":"…","smtp_password":"…"}
//! {"kind":"oauth2","provider":"microsoft","refresh_token":"…"}
//! ```
//!
//! Nothing here prints a secret: [`ImapSecret`]'s `Debug` hides them, and
//! a secret that does not parse is reported without its text.

use std::sync::Arc;

use postrinch_model::{EmailAddress, SignInMethod, SignInReason};
use serde::{Deserialize, Serialize};

use crate::{Keychain, SecretStore, SyncError, SyncResult};

/// What an IMAP account signs in with.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ImapSecret {
    /// A password or app password, for IMAP and (unless SMTP has its own)
    /// SMTP.
    Password {
        password: String,
        /// SMTP's own password, for accounts whose settings sign in with
        /// [`SignInMethod::SeparateSmtpPassword`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        smtp_password: Option<String>,
    },
    /// An OAuth 2 grant: the provider it is with (`microsoft`, `google`)
    /// and its refresh token. Access tokens come from an
    /// [`AccessTokens`](super::AccessTokens), never from here.
    OAuth2 {
        provider: String,
        refresh_token: String,
    },
}

impl std::fmt::Debug for ImapSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Password { smtp_password, .. } => f
                .debug_struct("Password")
                .field("password", &"<hidden>")
                .field("smtp_password", &smtp_password.as_ref().map(|_| "<hidden>"))
                .finish(),
            Self::OAuth2 { provider, .. } => f
                .debug_struct("OAuth2")
                .field("provider", provider)
                .field("refresh_token", &"<hidden>")
                .finish(),
        }
    }
}

/// Which server a credential is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Service {
    Imap,
    Smtp,
}

/// How to sign in to one server.
#[derive(Clone, Copy)]
pub(crate) enum Credential<'a> {
    Password(&'a str),
    /// With an access token from the account's [`AccessTokens`](super::AccessTokens).
    OAuth2,
}

impl ImapSecret {
    /// One password for IMAP and SMTP.
    pub fn password(password: impl Into<String>) -> Self {
        Self::Password {
            password: password.into(),
            smtp_password: None,
        }
    }

    /// The JSON kept in the keychain.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("an ImapSecret always serializes")
    }

    /// Reads the JSON kept in the keychain. `None` if it is not an
    /// [`ImapSecret`]; the reason is not given, as it could quote the
    /// secret.
    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }

    /// How to sign in to `service` when the account's settings say
    /// `method`. A secret of another kind than the settings ask for is no
    /// credential at all: the person signs in again.
    pub(crate) fn credential(
        &self,
        method: &SignInMethod,
        service: Service,
    ) -> SyncResult<Credential<'_>> {
        let missing = || SyncError::NeedsSignIn(SignInReason::NoCredential);
        match (method, self) {
            (SignInMethod::Password, Self::Password { password, .. }) => {
                Ok(Credential::Password(password))
            }
            (
                SignInMethod::SeparateSmtpPassword,
                Self::Password {
                    password,
                    smtp_password,
                },
            ) => match service {
                Service::Imap => Ok(Credential::Password(password)),
                Service::Smtp => smtp_password
                    .as_deref()
                    .map(Credential::Password)
                    .ok_or_else(missing),
            },
            (SignInMethod::OAuth2 { provider }, Self::OAuth2 { provider: kept, .. })
                if provider == kept =>
            {
                Ok(Credential::OAuth2)
            }
            _ => Err(missing()),
        }
    }
}

/// IMAP accounts' secrets in a [`SecretStore`] (the keychain's
/// `postrinch.imap` service in the app). Calls run on a blocking thread.
#[derive(Clone)]
pub struct ImapSecrets {
    store: Arc<dyn SecretStore>,
}

impl ImapSecrets {
    pub fn new(store: Arc<dyn SecretStore>) -> Self {
        Self { store }
    }

    /// The OS keychain, under [`Keychain::IMAP`].
    pub fn keychain() -> Self {
        Self::new(Arc::new(Keychain::IMAP))
    }

    /// The account's secret. Fails with [`SyncError::NeedsSignIn`] when
    /// there is none, or what is kept cannot be read.
    pub async fn get(&self, account: &EmailAddress) -> SyncResult<ImapSecret> {
        let json = self
            .stored(account)
            .await?
            .ok_or(SyncError::NeedsSignIn(SignInReason::NoCredential))?;
        ImapSecret::from_json(&json).ok_or_else(|| {
            tracing::warn!("the saved IMAP sign-in for {account} is not one Postrinch reads");
            SyncError::NeedsSignIn(SignInReason::NoCredential)
        })
    }

    /// Whether the account has a secret here at all, readable or not.
    pub(crate) async fn is_kept(&self, account: &EmailAddress) -> SyncResult<bool> {
        Ok(self.stored(account).await?.is_some())
    }

    /// Whether the account's secret is a Google grant: one that shares
    /// its OAuth client, and so its grant, with a Google account at the
    /// address.
    pub(crate) async fn holds_google_grant(&self, account: &EmailAddress) -> SyncResult<bool> {
        let Some(json) = self.stored(account).await? else {
            return Ok(false);
        };
        Ok(matches!(
            ImapSecret::from_json(&json),
            Some(ImapSecret::OAuth2 { provider, .. }) if provider == super::OAuthProvider::Google.id()
        ))
    }

    async fn stored(&self, account: &EmailAddress) -> SyncResult<Option<String>> {
        self.run(account, |store, account| store.get(account)).await
    }

    pub async fn set(&self, account: &EmailAddress, secret: &ImapSecret) -> SyncResult<()> {
        let json = secret.to_json();
        self.run(account, move |store, account| store.set(account, &json))
            .await
    }

    /// Removes the account's secret; one already gone is not an error.
    pub async fn delete(&self, account: &EmailAddress) -> SyncResult<()> {
        self.run(account, |store, account| store.delete(account))
            .await
    }

    async fn run<T: Send + 'static>(
        &self,
        account: &EmailAddress,
        call: impl FnOnce(&dyn SecretStore, &EmailAddress) -> SyncResult<T> + Send + 'static,
    ) -> SyncResult<T> {
        let store = Arc::clone(&self.store);
        let account = account.clone();
        tokio::task::spawn_blocking(move || call(store.as_ref(), &account))
            .await
            .map_err(|e| SyncError::Keychain(format!("keychain task failed: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_is_as_documented() {
        assert_eq!(
            ImapSecret::password("pw").to_json(),
            r#"{"kind":"password","password":"pw"}"#
        );
        let oauth = ImapSecret::OAuth2 {
            provider: "microsoft".into(),
            refresh_token: "rt".into(),
        };
        assert_eq!(
            oauth.to_json(),
            r#"{"kind":"oauth2","provider":"microsoft","refresh_token":"rt"}"#
        );
        assert_eq!(ImapSecret::from_json(&oauth.to_json()), Some(oauth));
        assert_eq!(ImapSecret::from_json(r#"{"kind":"pigeon"}"#), None);
    }

    #[test]
    fn debug_hides_secrets() {
        let shown = format!(
            "{:?} {:?}",
            ImapSecret::Password {
                password: "hunter2".into(),
                smtp_password: Some("swordfish".into()),
            },
            ImapSecret::OAuth2 {
                provider: "microsoft".into(),
                refresh_token: "1//refresh".into(),
            }
        );
        assert!(!shown.contains("hunter2") && !shown.contains("swordfish"));
        assert!(!shown.contains("1//refresh") && shown.contains("microsoft"));
    }

    #[test]
    fn the_settings_choose_the_credential() {
        let separate = ImapSecret::Password {
            password: "imap".into(),
            smtp_password: Some("smtp".into()),
        };
        let method = SignInMethod::SeparateSmtpPassword;
        assert!(matches!(
            separate.credential(&method, Service::Smtp),
            Ok(Credential::Password("smtp"))
        ));
        assert!(matches!(
            separate.credential(&SignInMethod::Password, Service::Smtp),
            Ok(Credential::Password("imap"))
        ));
        // SMTP's own password missing, or a secret of the wrong kind.
        let one = ImapSecret::password("imap");
        assert!(matches!(
            one.credential(&method, Service::Smtp),
            Err(SyncError::NeedsSignIn(SignInReason::NoCredential))
        ));
        let oauth = SignInMethod::OAuth2 {
            provider: "microsoft".into(),
        };
        assert!(one.credential(&oauth, Service::Imap).is_err());
    }
}
