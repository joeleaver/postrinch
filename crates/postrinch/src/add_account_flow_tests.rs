//! The add-account dialog's whole add, through the code the window calls
//! (the form's [`AddForm::to_add`], [`ImapSync::add_account`] and
//! [`outcome`]), against the fake IMAP and SMTP servers of
//! `postrinch-mailtest`: added, refused, unreachable, already here; and
//! what each failure says. Nothing touches the keychain or a real server.

use std::sync::Arc;

use postrinch_discover::{Discovery, HostedBy, Source};
use postrinch_mailtest::{
    ImapConfig, ImapServer, SmtpConfig, SmtpServer, SpecialUse as FakeSpecialUse,
};
use postrinch_model::{
    Account, EmailAddress, Provider, Security, Server, SignInMethod, SignInReason,
};
use postrinch_store::Store;
use postrinch_sync::imap::{
    AccountCheck, CheckFailure, Connector, ImapOAuth, ImapSecrets, ImapSync, NewSignIn,
    OAuthClients, TlsConfig,
};
use postrinch_sync::{MemorySecrets, SyncError};
use tokio::runtime::Runtime;

use super::*;
use crate::add_account::{AddForm, Found, Kind, ServerForm, Step};

const USER: &str = "ada@example.org";
const PASSWORD: &str = "correct horse";

struct Fixture {
    runtime: Runtime,
    imap_server: ImapServer,
    smtp_server: SmtpServer,
    store: Store,
    sync: ImapSync,
    oauth: ImapOAuth,
    _dir: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let (imap_server, smtp_server) = runtime.block_on(async {
        let imap = ImapServer::start(ImapConfig::new().user(USER, PASSWORD))
            .await
            .unwrap();
        imap.user(USER)
            .create_special_folder("Sent", FakeSpecialUse::Sent)
            .unwrap();
        let smtp = SmtpServer::start(SmtpConfig::new().user(USER, PASSWORD))
            .await
            .unwrap();
        (imap, smtp)
    });
    let secrets = ImapSecrets::new(Arc::new(MemorySecrets::default()));
    let oauth = ImapOAuth::new(
        OAuthClients::none(),
        secrets.clone(),
        Arc::new(MemorySecrets::default()),
    )
    .unwrap();
    let roots = vec![
        imap_server.certificate().der().clone(),
        smtp_server.certificate().der().clone(),
    ];
    let connector = Connector::new(TlsConfig::with_extra_roots(roots).unwrap());
    let tokens = ImapOAuth::new(
        OAuthClients::none(),
        secrets.clone(),
        Arc::new(MemorySecrets::default()),
    )
    .unwrap();
    let sync = ImapSync::new(store.clone(), connector, secrets, Arc::new(tokens));
    Fixture {
        runtime,
        imap_server,
        smtp_server,
        store,
        sync,
        oauth,
        _dir: dir,
    }
}

fn server(addr: std::net::SocketAddr, security: Security) -> Server {
    Server {
        host: addr.ip().to_string(),
        port: addr.port(),
        security,
        username: USER.into(),
    }
}

impl Fixture {
    /// The form as the servers panel leaves it: both fakes typed in by
    /// hand, and `password`.
    fn form(&self, password: &str) -> AddForm {
        let mut form = AddForm::new(Kind::Other);
        form.address = USER.into();
        form.name = "Ada".into();
        form.imap =
            ServerForm::from_server(&server(self.imap_server.tls_addr().unwrap(), Security::Tls));
        form.smtp = ServerForm::from_server(&server(
            self.smtp_server.starttls_addr().unwrap(),
            Security::StartTls,
        ));
        form.password = password.into();
        form
    }

    /// "Check and add" on `form`, from `from`, as the window runs it.
    fn add(&self, form: &AddForm, from: Step) -> Outcome {
        let (new, secret) = form.to_add().unwrap();
        let settings = new.settings.clone();
        let result = self.runtime.block_on(self.sync.add_account(
            &self.oauth,
            new,
            NewSignIn::Password(secret.unwrap()),
        ));
        outcome(result, &settings, &form.found, from)
    }
}

fn ada() -> EmailAddress {
    EmailAddress::parse(USER).unwrap()
}

#[test]
fn the_servers_typed_in_add_the_account() {
    let f = fixture();
    let Outcome::Done(account) = f.add(&f.form(PASSWORD), Step::Servers) else {
        panic!("not added");
    };
    assert_eq!(account.address, ada());
    assert_eq!(account.display_name.as_deref(), Some("Ada"));
    let stored = f.store.account(&ada()).unwrap().unwrap();
    assert_eq!(stored.provider, Provider::Imap);
    let settings = stored.imap.unwrap();
    assert_eq!(settings.smtp.security, Security::StartTls);
    assert_eq!(settings.sign_in, SignInMethod::Password);
}

#[test]
fn a_refused_password_stays_where_it_was_given() {
    let f = fixture();
    let mut form = f.form("wrong");
    form.found = Found {
        provider: Some("Example Mail".into()),
        app_password_url: Some("https://example.org/app-passwords".into()),
        ..Found::default()
    };
    let Outcome::Failed { step, error } = f.add(&form, Step::Found) else {
        panic!("a wrong password was taken");
    };
    assert_eq!(step, Step::Found);
    assert!(error.contains("refused the password"), "{error}");
    assert!(
        error.ends_with("Example Mail needs an app password."),
        "{error}"
    );
    assert!(!error.contains("wrong"), "the password is never shown");
    assert!(f.store.account(&ada()).unwrap().is_none(), "nothing kept");
}

#[test]
fn googles_mail_is_never_added_with_a_password() {
    let f = fixture();
    // Discovery's answer for Google's mail: OAuth 2 only, no app password
    // page; and no Google sign-in over IMAP in this copy.
    let mut google = Discovery::nothing();
    google.source = Source::IspdbViaMx;
    google.provider_name = Some("Google".into());
    google.hosted_by = Some(HostedBy::Google);
    google.oauth2_only = true;
    let mut form = f.form(PASSWORD);
    let servers = (form.imap.clone(), form.smtp.clone());
    form.take_discovery(&google, |_| false);
    // Even with servers and a password typed in by hand.
    (form.imap, form.smtp) = servers;
    form.password = PASSWORD.into();
    assert_eq!(form.found.app_password_url, None);
    assert!(!form.asks_password());
    let error = form.to_add().unwrap_err();
    assert!(error.contains("only in the browser"), "{error}");
    assert!(f.store.account(&ada()).unwrap().is_none(), "nothing kept");
}

#[test]
fn a_server_not_there_opens_the_servers_panel() {
    let f = fixture();
    let mut form = f.form(PASSWORD);
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    form.smtp.port = closed.port().to_string();
    let Outcome::Failed { step, error } = f.add(&form, Step::Found) else {
        panic!("a closed port was taken");
    };
    assert_eq!(step, Step::Servers);
    assert!(error.contains("could not be reached"), "{error}");
    assert!(f.store.account(&ada()).unwrap().is_none());
}

#[test]
fn an_imap_account_added_again_is_signed_in_again() {
    let f = fixture();
    let Outcome::Done(_) = f.add(&f.form(PASSWORD), Step::Found) else {
        panic!("not added");
    };
    let mut form = f.form(PASSWORD);
    form.name = "Ada L.".into();
    let Outcome::Done(again) = f.add(&form, Step::Found) else {
        panic!("not added again");
    };
    assert_eq!(again.display_name.as_deref(), Some("Ada L."));
    let accounts = f.store.accounts().unwrap();
    assert_eq!(accounts.len(), 1, "one account, not two");
}

#[test]
fn an_address_of_a_google_account_goes_back_to_the_address() {
    let f = fixture();
    f.store
        .put_account(&Account::new(ada(), Provider::Google))
        .unwrap();
    // The look-up refuses it before asking anyone about its servers.
    let looked = f.runtime.block_on(look_up(&f.store, &ada(), |_| {}));
    assert!(looked.unwrap_err().contains(USER));
    // And adding it by hand is refused, back on the address.
    let Outcome::Failed { step, error } = f.add(&f.form(PASSWORD), Step::Servers) else {
        panic!("added over a Google account");
    };
    assert_eq!(step, Step::Address);
    assert!(error.contains(USER), "{error}");
    let kept = f.store.account(&ada()).unwrap().unwrap();
    assert_eq!(kept.provider, Provider::Google, "left as it was");
}

#[test]
fn each_failure_names_its_server() {
    let refused = CheckFailure::SignIn(SignInReason::CredentialRejected {
        detail: "AUTHENTICATIONFAILED Invalid credentials".into(),
    });
    assert_eq!(
        failure_text(&refused, "imap.example.org"),
        "imap.example.org refused the password: “AUTHENTICATIONFAILED Invalid credentials”."
    );
    let bare = CheckFailure::SignIn(SignInReason::CredentialRejected {
        detail: String::new(),
    });
    assert_eq!(failure_text(&bare, "h"), "h refused the password.");
    assert!(failure_text(&CheckFailure::Insecure, "h").contains("no encryption"));
    assert!(
        failure_text(&CheckFailure::Tls("bad certificate".into()), "h").contains("bad certificate")
    );
}

#[test]
fn two_servers_refusing_alike_say_it_once() {
    let f = fixture();
    let form = f.form("wrong");
    let (new, _) = form.to_add().unwrap();
    let mut settings = new.settings;
    settings.smtp.host = settings.imap.host.clone();
    let refused = || {
        CheckFailure::SignIn(SignInReason::CredentialRejected {
            detail: "no".into(),
        })
    };
    let check = AccountCheck {
        imap: Err(refused()),
        smtp: Err(refused()),
    };
    let (text, servers_wrong) = check_text(&check, &settings, &Found::default());
    assert_eq!(
        text,
        format!("{} refused the password: “no”.", settings.imap.host)
    );
    assert!(!servers_wrong);
}

#[test]
fn one_host_for_both_names_the_server_that_failed() {
    let f = fixture();
    let (new, _) = f.form(PASSWORD).to_add().unwrap();
    let settings = new.settings;
    assert_eq!(
        settings.imap.host, settings.smtp.host,
        "both fakes on 127.0.0.1"
    );
    let check = AccountCheck {
        imap: Ok(Vec::new()),
        smtp: Err(CheckFailure::Unreachable("refused".into())),
    };
    let (text, servers_wrong) = check_text(&check, &settings, &Found::default());
    assert_eq!(
        text,
        "The outgoing server, 127.0.0.1, could not be reached: refused."
    );
    assert!(servers_wrong);
}

#[test]
fn errors_say_what_happened() {
    let not_set_up = SyncError::OAuthNotSetUp {
        provider: "Microsoft".into(),
        detail: None,
    };
    assert!(error_text(&not_set_up).starts_with("Microsoft sign-in is not set up"));
    let timed_out = SyncError::SignIn("timed out waiting for the browser".into());
    assert_eq!(
        error_text(&timed_out),
        "Signing in did not finish: timed out waiting for the browser."
    );
}

#[test]
fn a_cancelled_sign_in_is_no_failure() {
    let (new, _) = fixture().form("pw").to_add().unwrap();
    let settings = new.settings;
    let cancelled = outcome(
        Err(SyncError::SignInCancelled),
        &settings,
        &Found::default(),
        Step::Found,
    );
    assert!(matches!(cancelled, Outcome::Cancelled), "{cancelled:?}");
}

/// A Thunderbird home whose one profile has ada's account on the fakes,
/// with a decoy `logins.json` beside its prefs.
fn thunderbird_home(f: &Fixture) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".thunderbird");
    let profile = root.join("t1.default-release");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(
        root.join("profiles.ini"),
        "[Profile0]\nName=default-release\nIsRelative=1\nPath=t1.default-release\nDefault=1\n",
    )
    .unwrap();
    let imap = f.imap_server.tls_addr().unwrap();
    let smtp = f.smtp_server.starttls_addr().unwrap();
    std::fs::write(
        profile.join("prefs.js"),
        format!(
            r#"user_pref("mail.accountmanager.accounts", "account1");
user_pref("mail.account.account1.server", "server1");
user_pref("mail.account.account1.identities", "id1");
user_pref("mail.identity.id1.useremail", "{USER}");
user_pref("mail.identity.id1.fullName", "Ada Lovelace");
user_pref("mail.identity.id1.smtpServer", "smtp1");
user_pref("mail.server.server1.type", "imap");
user_pref("mail.server.server1.hostname", "{}");
user_pref("mail.server.server1.port", {});
user_pref("mail.server.server1.socketType", 3);
user_pref("mail.server.server1.userName", "{USER}");
user_pref("mail.smtpserver.smtp1.hostname", "{}");
user_pref("mail.smtpserver.smtp1.port", {});
user_pref("mail.smtpserver.smtp1.try_ssl", 2);
user_pref("mail.smtpserver.smtp1.username", "{USER}");
"#,
            imap.ip(),
            imap.port(),
            smtp.ip(),
            smtp.port()
        ),
    )
    .unwrap();
    std::fs::write(profile.join("logins.json"), "{\"logins\": [\"DECOY\"]}").unwrap();
    home
}

/// The form of the one account in `home`'s Thunderbird.
fn imported_form(home: &std::path::Path) -> AddForm {
    use crate::import_thunderbird::{Route, listing};

    let list = listing(&postrinch_thunderbird::find(home), &[], |_| false).unwrap();
    let row = list.rows().next().unwrap();
    assert_eq!(row.how, "Password");
    match &row.route {
        Some(Route::Imap(form)) => (**form).clone(),
        _ => panic!("not an IMAP account"),
    }
}

#[test]
fn an_account_imported_from_thunderbird_is_added() {
    let f = fixture();
    let home = thunderbird_home(&f);
    let mut form = imported_form(home.path());
    assert!(
        form.password.is_empty(),
        "nothing secret comes from Thunderbird"
    );
    // The person gives the password, as the import's turn asks.
    form.password = PASSWORD.into();
    let Outcome::Done(account) = f.add(&form, Step::ImportSignIn) else {
        panic!("not added");
    };
    assert_eq!(account.address, ada());
    assert_eq!(account.display_name.as_deref(), Some("Ada Lovelace"));
    let settings = f.store.account(&ada()).unwrap().unwrap().imap.unwrap();
    assert_eq!(settings.imap.security, Security::Tls);
    assert_eq!(settings.smtp.security, Security::StartTls);
    assert_eq!(settings.sign_in, SignInMethod::Password);

    // Looked at again, it is here already.
    let here: Vec<EmailAddress> = f
        .store
        .accounts()
        .unwrap()
        .into_iter()
        .map(|a| a.address)
        .collect();
    let found = postrinch_thunderbird::find(home.path());
    let again = crate::import_thunderbird::listing(&found, &here, |_| false).unwrap();
    let row = again.rows().next().unwrap();
    assert_eq!(row.how, "Already in Postrinch");
    assert!(row.route.is_none());
}

#[test]
fn a_wrong_password_on_an_imported_account_stays_on_its_turn() {
    let f = fixture();
    let home = thunderbird_home(&f);
    let mut form = imported_form(home.path());
    form.password = "wrong".into();
    let Outcome::Failed { step, error } = f.add(&form, Step::ImportSignIn) else {
        panic!("a wrong password was taken");
    };
    assert_eq!(step, Step::ImportSignIn);
    assert!(error.contains("refused the password"), "{error}");
    assert!(f.store.account(&ada()).unwrap().is_none());
}
