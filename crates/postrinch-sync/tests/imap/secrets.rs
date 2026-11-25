//! The account's secret, as kept in the keychain (here, in memory).

use std::sync::Arc;

use postrinch_model::SignInReason;
use postrinch_sync::imap::{ImapSecret, ImapSecrets};
use postrinch_sync::{MemorySecrets, SecretStore, SyncError, SyncResult};

use crate::support::{PASSWORD, account, oauth_secret};

#[tokio::test]
async fn both_kinds_round_trip_through_the_store() {
    let memory = Arc::new(MemorySecrets::default());
    let secrets = ImapSecrets::new(memory.clone());
    let account = account();

    let password = ImapSecret::password(PASSWORD);
    secrets.set(&account, &password).await.unwrap();
    let stored = memory.get(&account).unwrap().unwrap();
    assert_eq!(stored, r#"{"kind":"password","password":"correct horse"}"#);
    assert_eq!(secrets.get(&account).await.unwrap(), password);

    let oauth = oauth_secret();
    secrets.set(&account, &oauth).await.unwrap();
    let stored = memory.get(&account).unwrap().unwrap();
    assert_eq!(
        stored,
        r#"{"kind":"oauth2","provider":"microsoft","refresh_token":"refresh"}"#
    );
    assert_eq!(secrets.get(&account).await.unwrap(), oauth);

    secrets.delete(&account).await.unwrap();
    assert!(memory.get(&account).unwrap().is_none());
    assert_no_credential(secrets.get(&account).await);
}

#[tokio::test]
async fn a_separate_smtp_password_is_kept_beside_the_imap_one() {
    let memory = Arc::new(MemorySecrets::default());
    let secrets = ImapSecrets::new(memory.clone());
    let secret = ImapSecret::Password {
        password: PASSWORD.into(),
        smtp_password: Some("sending".into()),
    };
    secrets.set(&account(), &secret).await.unwrap();
    let stored = memory.get(&account()).unwrap().unwrap();
    assert_eq!(
        stored,
        r#"{"kind":"password","password":"correct horse","smtp_password":"sending"}"#
    );
    assert_eq!(secrets.get(&account()).await.unwrap(), secret);
}

#[tokio::test]
async fn a_secret_that_does_not_parse_is_no_credential() {
    let memory = Arc::new(MemorySecrets::default());
    memory.set(&account(), "not json").unwrap();
    let secrets = ImapSecrets::new(memory);
    assert_no_credential(secrets.get(&account()).await);
}

fn assert_no_credential(result: SyncResult<ImapSecret>) {
    assert!(
        matches!(
            result,
            Err(SyncError::NeedsSignIn(SignInReason::NoCredential))
        ),
        "{result:?}"
    );
}
