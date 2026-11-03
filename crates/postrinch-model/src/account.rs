use std::fmt;

use serde::{Deserialize, Serialize};

use crate::EmailAddress;

/// Where an account's mail comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Provider {
    /// Gmail and Google Calendar, over Google's APIs with OAuth.
    Google,
    /// Any other server, over IMAP and SMTP.
    Imap,
}

/// Why an account has to sign in again. Each is a normal state of an
/// account, shown on it, not an error for the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignInReason {
    /// The provider refused the stored credential: revoked, expired, the
    /// password changed, or unused too long. Google reports all of these as
    /// one `invalid_grant`, so `detail` keeps whatever description it gave.
    CredentialRejected { detail: String },
    /// An administrator's policy blocks this app for the account.
    AdminPolicy { detail: String },
    /// The person signed in but left out access Postrinch needs (Google lets
    /// them untick scopes on the consent screen).
    ScopesMissing { missing: Vec<String> },
    /// No credential is stored for the account (the keychain entry is gone).
    NoCredential,
}

impl fmt::Display for SignInReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CredentialRejected { detail } => {
                write!(f, "the provider no longer accepts its sign-in ({detail})")
            }
            Self::AdminPolicy { detail } => {
                write!(f, "an administrator's policy blocks Postrinch ({detail})")
            }
            Self::ScopesMissing { missing } => write!(
                f,
                "access to {} was not granted; sign in again and allow it",
                missing.join(" and ")
            ),
            Self::NoCredential => f.write_str("there is no saved sign-in for it on this machine"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountState {
    Active,
    NeedsSignIn(SignInReason),
}

/// One mail account. The address identifies it: it is the key its
/// credential is kept under in the OS keychain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub address: EmailAddress,
    pub display_name: Option<String>,
    pub provider: Provider,
    pub state: AccountState,
}

impl Account {
    pub fn new(address: EmailAddress, provider: Provider) -> Self {
        Self {
            address,
            display_name: None,
            provider,
            state: AccountState::Active,
        }
    }

    pub fn is_active(&self) -> bool {
        self.state == AccountState::Active
    }

    pub fn needs_sign_in(&mut self, reason: SignInReason) {
        self.state = AccountState::NeedsSignIn(reason);
    }

    pub fn signed_in(&mut self) {
        self.state = AccountState::Active;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_state_round_trip() {
        let address = EmailAddress::parse("ada@example.org").unwrap();
        let mut account = Account::new(address, Provider::Google);
        assert!(account.is_active());

        account.needs_sign_in(SignInReason::ScopesMissing {
            missing: vec!["https://mail.google.com/".into()],
        });
        assert!(!account.is_active());

        account.signed_in();
        assert!(account.is_active());
    }
}
