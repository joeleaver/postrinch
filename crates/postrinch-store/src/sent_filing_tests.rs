use postrinch_model::{Account, FolderRoles, Provider, Security, Server, SignInMethod};

use super::*;

fn setup() -> (tempfile::TempDir, Store, EmailAddress) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let address = EmailAddress::parse("ada@example.org").unwrap();
    let server = |host: &str| Server {
        host: host.into(),
        port: 993,
        security: Security::Tls,
        username: "ada".into(),
    };
    let mut account = Account::new(address.clone(), Provider::Imap);
    account.imap = Some(ImapSettings {
        imap: server("imap.example.org"),
        smtp: server("smtp.example.org"),
        sign_in: SignInMethod::Password,
        roles: FolderRoles::default(),
        server_files_sent: true,
    });
    store.put_account(&account).unwrap();
    (dir, store, address)
}

fn settings(store: &Store, address: &EmailAddress) -> ImapSettings {
    store.account(address).unwrap().unwrap().imap.unwrap()
}

fn id(n: u32) -> ProviderMessageId {
    ProviderMessageId(format!("m{n}"))
}

#[test]
fn off_only_after_misses_in_a_row() {
    let (_dir, store, me) = setup();
    assert!(!store.sent_copy_filed(&me, &id(1), true, 3).unwrap());
    assert!(!store.sent_copy_filed(&me, &id(2), true, 3).unwrap());
    // A copy found: the count starts again.
    store.sent_copy_found(&me).unwrap();
    assert!(!store.sent_copy_filed(&me, &id(3), true, 3).unwrap());
    assert!(!store.sent_copy_filed(&me, &id(4), true, 3).unwrap());
    assert!(settings(&store, &me).server_files_sent);
    assert!(store.sent_copy_filed(&me, &id(5), true, 3).unwrap());
    assert!(!settings(&store, &me).server_files_sent);
    assert!(!store.server_files_sent(&me, Some("SENT")).unwrap());
    // Filed while off: not a miss, and nothing to turn off.
    assert!(!store.sent_copy_filed(&me, &id(6), false, 3).unwrap());
}

#[test]
fn settings_saved_meanwhile_are_kept() {
    let (_dir, store, me) = setup();
    store.sent_copy_filed(&me, &id(1), true, 2).unwrap();
    // The person changes a setting between two sends.
    let mut changed = settings(&store, &me);
    changed.roles.sent = Some("Sent Items".into());
    store.set_imap_settings(&me, &changed).unwrap();
    assert!(store.sent_copy_filed(&me, &id(2), true, 2).unwrap());
    let now = settings(&store, &me);
    assert!(!now.server_files_sent);
    assert_eq!(now.roles.sent.as_deref(), Some("Sent Items"));
}

#[test]
fn an_account_without_imap_settings_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let me = EmailAddress::parse("ada@example.org").unwrap();
    store
        .put_account(&Account::new(me.clone(), Provider::Google))
        .unwrap();
    assert!(store.sent_copy_found(&me).is_err());
}
