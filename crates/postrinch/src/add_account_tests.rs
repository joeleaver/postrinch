//! The add-account dialog's form: what discovery fills in, the step it
//! leads to, and what the form must hold before it is checked.

use postrinch_discover::{
    Candidate, Discovery, HostedBy, Progress, Security as Sec, Source, Step as LookStep, StepState,
    Username,
};
use postrinch_model::{Security, SignInMethod};
use postrinch_sync::imap::{ImapSecret, OAuthProvider};

use super::*;
use crate::add_account_run::{look_line, note_progress};

fn form(kind: Kind, address: &str) -> AddForm {
    let mut form = AddForm::new(kind);
    form.address = address.into();
    form
}

fn candidate(host: &str, port: u16, security: Sec, username: Username) -> Candidate {
    Candidate {
        host: host.into(),
        port,
        security,
        username,
    }
}

fn fastmail() -> Discovery {
    let mut found = Discovery::nothing();
    found.source = Source::Ispdb;
    found.provider_name = Some("Fastmail".into());
    found.imap = vec![candidate(
        "imap.fastmail.com",
        993,
        Sec::Tls,
        Username::Address,
    )];
    found.smtp = vec![candidate(
        "smtp.fastmail.com",
        465,
        Sec::Tls,
        Username::LocalPart,
    )];
    found.app_password_url = Some("https://app.fastmail.com/settings/security".into());
    found
}

fn every_sign_in(_: OAuthProvider) -> bool {
    true
}

fn no_sign_in(_: OAuthProvider) -> bool {
    false
}

#[test]
fn discovery_fills_in_the_servers_it_found() {
    let mut f = form(Kind::Other, " ann@fastmail.com ");
    f.take_discovery(&fastmail(), every_sign_in);
    assert_eq!(f.imap.line(), "imap.fastmail.com · 993 · TLS");
    assert_eq!(f.imap.username, "ann@fastmail.com");
    assert_eq!(f.smtp.username, "ann", "the provider's own user name");
    assert_eq!(f.found.provider.as_deref(), Some("Fastmail"));
    assert_eq!(f.found.source, Some("From Thunderbird's list of providers"));
    assert!(f.found.app_password_url.is_some());
    assert_eq!(f.way, Way::Password, "no browser sign-in for Fastmail");
    assert!(f.no_way_in().is_none());
    assert_eq!(f.step_after_discovery(), Step::Found);
}

#[test]
fn nothing_found_guesses_and_opens_the_servers() {
    let mut f = form(Kind::Other, "bo@example.org");
    f.take_discovery(&Discovery::nothing(), every_sign_in);
    assert_eq!(f.imap.line(), "mail.example.org · 993 · TLS");
    assert_eq!(f.smtp.line(), "mail.example.org · 587 · STARTTLS");
    assert_eq!(f.smtp.username, "bo@example.org");
    assert_eq!(f.step_after_discovery(), Step::Servers);
}

#[test]
fn microsoft_signs_in_in_the_browser_when_it_can() {
    let mut f = form(Kind::Microsoft, "cy@outlook.com");
    f.take_discovery(&Discovery::nothing(), every_sign_in);
    assert_eq!(f.imap.line(), "outlook.office365.com · 993 · TLS");
    assert_eq!(f.smtp.line(), "smtp.office365.com · 587 · STARTTLS");
    assert_eq!(f.way, Way::Browser(OAuthProvider::Microsoft));
    assert_eq!(f.found.provider.as_deref(), Some("Microsoft"));
    assert!(f.found.browser_only, "Microsoft takes no passwords");
    assert!(f.no_way_in().is_none());
    let (new, secret) = f.to_add().unwrap();
    assert!(secret.is_none(), "no password for the browser");
    assert_eq!(
        new.settings.sign_in,
        OAuthProvider::Microsoft.sign_in_method()
    );
}

#[test]
fn a_browser_sign_in_not_set_up_leaves_no_way_in() {
    let mut found = Discovery::nothing();
    found.source = Source::DomainAutoconfig;
    found.hosted_by = Some(HostedBy::Microsoft);
    let mut f = form(Kind::Other, "di@contoso.com");
    f.take_discovery(&found, no_sign_in);
    assert_eq!(f.way, Way::Password);
    let why = f.no_way_in().unwrap();
    assert!(why.contains("only in the browser"), "{why}");
    assert!(!f.asks_password());
    f.password = "secret".into();
    assert_eq!(f.to_add().err(), Some(why), "no password is taken");
}

/// Google's mail, as discovery tells it: OAuth 2 only, no app password
/// page.
fn google() -> Discovery {
    let mut found = Discovery::nothing();
    found.source = Source::IspdbViaMx;
    found.provider_name = Some("Google".into());
    found.hosted_by = Some(HostedBy::Google);
    found.oauth2_only = true;
    found.imap = vec![candidate(
        "imap.gmail.com",
        993,
        Sec::Tls,
        Username::Address,
    )];
    found.smtp = vec![candidate(
        "smtp.gmail.com",
        465,
        Sec::Tls,
        Username::Address,
    )];
    found
}

#[test]
fn google_mail_offers_google_first_then_imap_in_the_browser() {
    let mut f = form(Kind::Other, "ed@workspace.example");
    f.take_discovery(&google(), every_sign_in);
    assert_eq!(f.step_after_discovery(), Step::Workspace);
    f.use_imap_anyway();
    assert_eq!(f.way, Way::Browser(OAuthProvider::Google));
    assert!(f.found.browser_only, "no app password instead");
    assert_eq!(f.found.app_password_url, None);
    assert!(!f.asks_password());
    let (new, secret) = f.to_add().unwrap();
    assert!(secret.is_none());
    assert_eq!(new.settings.sign_in, OAuthProvider::Google.sign_in_method());
}

#[test]
fn google_mail_never_offers_a_password() {
    // Without Google's sign-in over IMAP in this copy, there is no way in
    // over IMAP at all: no app password, no password.
    let mut f = form(Kind::Other, "ed@gmail.com");
    f.take_discovery(&google(), no_sign_in);
    assert_eq!(f.step_after_discovery(), Step::Workspace);
    let why = f.no_way_in().expect("no way in over IMAP");
    assert!(
        why.starts_with("Google signs mail apps in only in the browser"),
        "{why}"
    );
    f.use_imap_anyway();
    assert!(!f.asks_password());
    f.password = "an app password".into();
    assert_eq!(f.to_add().err(), Some(why));
}

#[test]
fn the_servers_must_be_whole() {
    let mut s = ServerForm::from_server(&postrinch_model::Server {
        host: "imap.example.org".into(),
        port: 993,
        security: Security::Tls,
        username: "fi".into(),
    });
    assert!(s.server("incoming").is_ok());
    s.port = "0".into();
    assert_eq!(
        s.server("incoming").unwrap_err(),
        "The incoming port is a number from 1 to 65535."
    );
    s.port = "70000".into();
    assert!(s.server("incoming").is_err());
    s.port = " 143 ".into();
    assert_eq!(s.server("incoming").unwrap().port, 143);
    s.host = "imap example".into();
    assert_eq!(
        s.server("outgoing").unwrap_err(),
        "Give the outgoing server's name."
    );
    s.host = "imap.example.org".into();
    s.username = "  ".into();
    assert!(s.server("incoming").unwrap_err().contains("user name"));
}

#[test]
fn a_password_is_needed_and_the_outgoing_one_when_it_has_its_own() {
    let mut f = form(Kind::Other, "gu@fastmail.com");
    f.take_discovery(&fastmail(), no_sign_in);
    assert_eq!(f.to_add().err().unwrap(), "Give the password.");
    f.password = "pw".into();
    let (new, secret) = f.to_add().unwrap();
    assert_eq!(new.settings.sign_in, SignInMethod::Password);
    assert_eq!(secret, Some(ImapSecret::password("pw")));
    assert!(
        new.settings.server_files_sent,
        "until sync learns otherwise"
    );
    f.separate_smtp = true;
    assert_eq!(
        f.to_add().err().unwrap(),
        "Give the outgoing server's password."
    );
    f.smtp_password = "out".into();
    let (new, secret) = f.to_add().unwrap();
    assert_eq!(new.settings.sign_in, SignInMethod::SeparateSmtpPassword);
    assert_eq!(
        secret,
        Some(ImapSecret::Password {
            password: "pw".into(),
            smtp_password: Some("out".into())
        })
    );
    f.forget_passwords();
    assert!(f.password.is_empty() && f.smtp_password.is_empty());
}

#[test]
fn servers_nobody_vouches_for_need_the_person_to_trust_them() {
    let mut found = fastmail();
    found.source = Source::Srv;
    found.confirm_servers = true;
    let mut f = form(Kind::Other, "ha@example.org");
    f.take_discovery(&found, no_sign_in);
    f.password = "pw".into();
    assert!(f.to_add().unwrap_err().contains("trust"));
    f.trusted = true;
    assert!(f.to_add().is_ok());
}

#[test]
fn the_address_and_name_are_checked_and_trimmed() {
    let mut f = form(Kind::Other, "not an address");
    f.take_discovery(&fastmail(), no_sign_in);
    f.password = "pw".into();
    assert_eq!(
        f.to_add().err().unwrap(),
        "“not an address” is not an email address."
    );
    f.address = "io@fastmail.com".into();
    f.name = "  Io  ".into();
    let (new, _) = f.to_add().unwrap();
    assert_eq!(new.display_name.as_deref(), Some("Io"));
    f.name = " ".into();
    assert_eq!(f.to_add().unwrap().0.display_name, None);
}

#[test]
fn discovery_steps_report_in_place() {
    let mut steps = Vec::new();
    let at = |step, state| Progress { step, state };
    note_progress(&mut steps, at(LookStep::DomainSettings, StepState::Running));
    note_progress(
        &mut steps,
        at(LookStep::DomainSettings, StepState::NothingFound),
    );
    note_progress(&mut steps, at(LookStep::ProviderList, StepState::Found));
    assert_eq!(
        steps,
        [
            (LookStep::DomainSettings, StepState::NothingFound),
            (LookStep::ProviderList, StepState::Found)
        ]
    );
    assert_eq!(
        look_line("example.org", LookStep::DomainSettings, StepState::Running),
        "example.org's own settings…"
    );
    assert_eq!(
        look_line("example.org", LookStep::DnsRecords, StepState::NothingFound),
        "The domain's DNS records: nothing found"
    );
}

#[test]
fn a_changed_address_forgets_what_was_found_for_the_old_one() {
    let mut google = google();
    google.confirm_servers = true;
    let mut f = form(Kind::Other, "a@gmail.com");
    f.name = "Ada".into();
    f.take_discovery(&google, every_sign_in);
    f.password = "secret".into();
    f.trusted = true;

    // The same address, typed again with a space, keeps it all.
    f.set_address("a@gmail.com ".into());
    assert_eq!(f.way, Way::Browser(OAuthProvider::Google));
    assert!(f.found.google && f.found.confirm_servers);
    assert_eq!(f.password, "secret");

    f.set_address("b@example.org".into());
    assert_eq!(f.address, "b@example.org");
    assert_eq!(f.name, "Ada", "the name stays");
    assert_eq!(f.found, Found::default());
    assert_eq!(f.way, Way::Password);
    assert!(!f.trusted);
    assert!(f.password.is_empty(), "a password for the old address goes");
    assert!(f.imap.host.is_empty() && f.smtp.host.is_empty());

    // By hand, then: the new address's guesses, not the old servers.
    f.guess_servers();
    assert_eq!(f.imap.host, "mail.example.org");
    assert_eq!(f.imap.username, "b@example.org");
    assert_eq!(f.found.oauth, None);
    assert!(f.found.app_password_url.is_none() && !f.found.confirm_servers);
}

#[test]
fn servers_by_hand_keep_what_discovery_found_for_the_address() {
    let mut f = form(Kind::Other, "ed@fastmail.com");
    f.take_discovery(&fastmail(), no_sign_in);
    f.guess_servers();
    assert_eq!(f.imap.host, "imap.fastmail.com");
}
