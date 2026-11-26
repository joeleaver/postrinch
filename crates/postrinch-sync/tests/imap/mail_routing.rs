//! [`MailSync`] sends each call to the account's provider: a Google
//! account's to the Gmail engine (against wiremock's Gmail API), an IMAP
//! account's to the IMAP engine (against the fake server), both in one
//! store. A copy without Google sign-in turns off Google accounts only,
//! and an IMAP account has no calendars.

use std::sync::Arc;

use postrinch_mailtest::{ImapConfig, ImapServer, UserHandle};
use postrinch_model::{Account, EmailAddress, Provider, Security, SignInMethod};
use postrinch_store::Store;
use postrinch_sync::imap::{ImapSecret, ImapSecrets, ImapSync};
use postrinch_sync::{MailSync, SyncError};
use postrinch_sync_core::MemorySecrets;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

use super::common::{self, get, mock_history, mock_labels, mock_meta, mock_raw, quiet};
use super::support::{PASSWORD, Tokens, USER, account, closed_port, connector, server, settings};
use super::sync_support::mail;

/// A Google account on wiremock's Gmail and an IMAP account on the fake
/// server, in one store.
struct Mixed {
    gmail: common::Fixture,
    _imap_server: ImapServer,
    imap_user: UserHandle,
    imap: ImapSync,
    ada: EmailAddress,
}

impl Mixed {
    fn store(&self) -> &Store {
        &self.gmail.store
    }

    fn google(&self) -> &EmailAddress {
        &self.gmail.me
    }

    /// The façade, with or without the Gmail engine.
    fn mail(&self, with_gmail: bool) -> MailSync {
        let gmail = with_gmail.then(|| self.gmail.sync.clone());
        MailSync::new(self.store().clone(), gmail, self.imap.clone())
    }
}

async fn mixed() -> Mixed {
    let gmail = common::fixture().await;
    let imap_server = ImapServer::start(ImapConfig::new().user(USER, PASSWORD))
        .await
        .unwrap();
    let imap_user = imap_server.user(USER);
    let ada = account();
    let mut stored = Account::new(ada.clone(), Provider::Imap);
    stored.imap = Some(settings(
        server(imap_server.tls_addr().unwrap(), Security::Tls),
        server(closed_port().await, Security::Tls),
        SignInMethod::Password,
    ));
    gmail.store.put_account(&stored).unwrap();
    let secrets = ImapSecrets::new(Arc::new(MemorySecrets::default()));
    secrets
        .set(&ada, &ImapSecret::password(PASSWORD))
        .await
        .unwrap();
    let imap = ImapSync::new(
        gmail.store.clone(),
        connector(imap_server.certificate()),
        secrets,
        Arc::new(Tokens::none()),
    );
    Mixed {
        gmail,
        _imap_server: imap_server,
        imap_user,
        imap,
        ada,
    }
}

/// Gmail answers a first sync with one message.
async fn gmail_has_one_message(m: &Mixed) {
    let server = &m.gmail.server;
    get(
        server,
        "profile",
        json!({"emailAddress": "me@example.org", "historyId": "100"}),
    )
    .await;
    mock_labels(server).await;
    get(
        server,
        "messages",
        json!({"messages": [{"id": "m1", "threadId": "t-m1"}]}),
    )
    .await;
    mock_meta(server, "m1", &["INBOX"], "From Gmail").await;
    mock_raw(server, "m1", &["INBOX"], "From Gmail").await;
    mock_history(server, "100", json!({"historyId": "100"})).await;
}

fn subjects(store: &Store, account: &EmailAddress) -> Vec<String> {
    let mut subjects: Vec<String> = store
        .message_ids(account)
        .unwrap()
        .iter()
        .map(|id| {
            store
                .message(account, id)
                .unwrap()
                .unwrap()
                .subject()
                .to_string()
        })
        .collect();
    subjects.sort();
    subjects
}

fn folder_names(user: &UserHandle) -> Vec<String> {
    user.folders()
        .unwrap()
        .into_iter()
        .map(|f| f.name)
        .collect()
}

#[tokio::test]
async fn each_account_syncs_over_its_own_provider() {
    let m = mixed().await;
    gmail_has_one_message(&m).await;
    m.imap_user
        .append("INBOX", mail(Some("i@x"), "From IMAP", ""))
        .unwrap();
    let sync = m.mail(true);

    sync.sync(m.google(), quiet()).await.unwrap();
    sync.sync(&m.ada, quiet()).await.unwrap();
    sync.backfill(&m.ada, quiet()).await.unwrap();

    assert_eq!(subjects(m.store(), m.google()), ["From Gmail"]);
    assert_eq!(subjects(m.store(), &m.ada), ["From IMAP"]);
}

#[tokio::test]
async fn labels_are_made_in_the_accounts_own_provider() {
    let m = mixed().await;
    Mock::given(method("POST"))
        .and(path("/gmail/labels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "Label_7", "name": "Trips", "type": "user"
        })))
        .expect(1)
        .mount(&m.gmail.server)
        .await;
    let sync = m.mail(true);

    let google = sync.create_label(m.google(), "Trips").await.unwrap();
    let imap = sync.create_label(&m.ada, "Plans").await.unwrap();

    assert_eq!(google.provider_id, "Label_7");
    assert_eq!(imap.name, "Plans");
    assert!(folder_names(&m.imap_user).contains(&"Plans".to_string()));
    let renamed = sync
        .rename_label(&m.ada, &imap.provider_id, "Ideas")
        .await
        .unwrap();
    assert_eq!(renamed.name, "Ideas");
    sync.delete_label(&m.ada, &renamed.provider_id)
        .await
        .unwrap();
    assert!(!folder_names(&m.imap_user).contains(&"Ideas".to_string()));
}

#[tokio::test]
async fn without_google_sign_in_only_google_accounts_are_off() {
    let m = mixed().await;
    m.imap_user
        .append("INBOX", mail(Some("i@x"), "From IMAP", ""))
        .unwrap();
    let sync = m.mail(false);

    let google = sync.sync(m.google(), quiet()).await;
    assert!(
        matches!(&google, Err(SyncError::OAuthNotSetUp { provider, .. }) if provider == "Google"),
        "{google:?}"
    );
    sync.sync(&m.ada, quiet()).await.unwrap();
    assert_eq!(subjects(m.store(), &m.ada).len(), 1);
    assert!(sync.gmail().is_none());
}

#[tokio::test]
async fn an_imap_account_has_no_calendars() {
    let m = mixed().await;
    let sync = m.mail(true);
    assert!(sync.calendar(m.google()).await.is_ok());
    let refused = sync.calendar(&m.ada).await.map(|_| ());
    assert!(
        matches!(&refused, Err(SyncError::NoCalendars { address }) if *address == m.ada),
        "{refused:?}"
    );
    assert!(refused.unwrap_err().to_string().contains("calendars"));
}

#[tokio::test]
async fn an_account_not_here_is_refused_by_name() {
    let m = mixed().await;
    let stranger = EmailAddress::parse("nobody@example.net").unwrap();
    let result = m.mail(true).waiting(&stranger).await;
    assert!(
        matches!(&result, Err(SyncError::Config(text)) if text.contains("nobody@example.net")),
        "{result:?}"
    );
}
