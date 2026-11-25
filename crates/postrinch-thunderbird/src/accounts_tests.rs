use super::*;

fn one(prefs: &str) -> Account {
    let mut found = accounts(&Prefs::parse(prefs));
    assert_eq!(found.len(), 1, "{found:?}");
    found.remove(0)
}

const IMAP: &str = r#"
user_pref("mail.accountmanager.accounts", "account1");
user_pref("mail.account.account1.server", "server1");
user_pref("mail.account.account1.identities", "id1,id9");
user_pref("mail.server.server1.type", "imap");
user_pref("mail.server.server1.hostname", "imap.example.org");
user_pref("mail.identity.id1.useremail", "ada@example.org");
user_pref("mail.identity.id9.useremail", "other@example.org");
"#;

#[test]
fn unset_ports_and_security_are_thunderbirds_defaults() {
    let account = one(&format!(
        "{IMAP}user_pref(\"mail.smtp.defaultserver\", \"smtp1\");\nuser_pref(\"mail.smtpserver.smtp1.hostname\", \"smtp.example.org\");"
    ));
    assert_eq!(account.kind, AccountKind::Imap);
    let incoming = account.incoming.unwrap();
    assert_eq!(
        (incoming.port, incoming.security, incoming.sign_in),
        (143, Security::Plain, SignIn::Password)
    );
    assert_eq!(incoming.username, None);
    let outgoing = account.outgoing.unwrap();
    // Thunderbird's SMTP client: 465 for TLS, else submission's 587.
    assert_eq!(
        (outgoing.port, outgoing.security, outgoing.sign_in),
        (587, Security::Plain, SignIn::Password)
    );
    // The first identity is the account's.
    assert_eq!(
        account.identity.unwrap().address.as_deref(),
        Some("ada@example.org")
    );
}

#[test]
fn tls_changes_the_usual_ports() {
    let account = one(&format!(
        "{IMAP}user_pref(\"mail.server.server1.socketType\", 3);\nuser_pref(\"mail.server.server1.port\", 0);\nuser_pref(\"mail.identity.id1.smtpServer\", \"smtp1\");\nuser_pref(\"mail.smtpserver.smtp1.hostname\", \"smtp.example.org\");\nuser_pref(\"mail.smtpserver.smtp1.try_ssl\", 3);"
    ));
    let incoming = account.incoming.unwrap();
    assert_eq!((incoming.port, incoming.security), (993, Security::Tls));
    let outgoing = account.outgoing.unwrap();
    assert_eq!((outgoing.port, outgoing.security), (465, Security::Tls));
}

#[test]
fn starttls_keeps_the_usual_ports() {
    let account = one(&format!(
        "{IMAP}user_pref(\"mail.server.server1.socketType\", 2);\nuser_pref(\"mail.identity.id1.smtpServer\", \"smtp1\");\nuser_pref(\"mail.smtpserver.smtp1.hostname\", \"smtp.example.org\");\nuser_pref(\"mail.smtpserver.smtp1.try_ssl\", 2);"
    ));
    let incoming = account.incoming.unwrap();
    assert_eq!(
        (incoming.port, incoming.security),
        (143, Security::StartTls)
    );
    let outgoing = account.outgoing.unwrap();
    assert_eq!(
        (outgoing.port, outgoing.security),
        (587, Security::StartTls)
    );
}

#[test]
fn security_codes() {
    assert_eq!(Security::from_code(None), Security::Plain);
    assert_eq!(Security::from_code(Some(0)), Security::Plain);
    assert_eq!(Security::from_code(Some(1)), Security::Plain);
    assert_eq!(Security::from_code(Some(2)), Security::StartTls);
    assert_eq!(Security::from_code(Some(3)), Security::Tls);
    assert_eq!(Security::from_code(Some(7)), Security::Plain);
}

#[test]
fn sign_in_codes() {
    for code in [None, Some(0), Some(2), Some(3), Some(4), Some(8), Some(9)] {
        assert_eq!(SignIn::from_code(code), SignIn::Password, "{code:?}");
    }
    assert_eq!(SignIn::from_code(Some(10)), SignIn::OAuth2);
    assert_eq!(SignIn::from_code(Some(1)), SignIn::None);
    assert_eq!(SignIn::from_code(Some(5)), SignIn::Kerberos);
    assert_eq!(SignIn::from_code(Some(6)), SignIn::Ntlm);
    assert_eq!(SignIn::from_code(Some(7)), SignIn::Certificate);
    for code in [-1, 11, 99] {
        assert_eq!(SignIn::from_code(Some(code)), SignIn::Other(code));
    }
}

#[test]
fn each_servers_sign_in_is_its_own() {
    let account = one(&format!(
        "{IMAP}user_pref(\"mail.server.server1.authMethod\", 5);\nuser_pref(\"mail.smtp.defaultserver\", \"smtp1\");\nuser_pref(\"mail.smtpserver.smtp1.hostname\", \"smtp.example.org\");\nuser_pref(\"mail.smtpserver.smtp1.authMethod\", 1);"
    ));
    assert_eq!(account.incoming.unwrap().sign_in, SignIn::Kerberos);
    assert_eq!(account.outgoing.unwrap().sign_in, SignIn::None);
}

#[test]
fn a_renamed_server_connects_to_its_real_host_and_user() {
    let account = one(&format!(
        "{IMAP}user_pref(\"mail.server.server1.realhostname\", \"new.example.org\");\nuser_pref(\"mail.server.server1.userName\", \"ada\");\nuser_pref(\"mail.server.server1.realuserName\", \"ada.l\");"
    ));
    let incoming = account.incoming.unwrap();
    assert_eq!(incoming.host, "new.example.org");
    assert_eq!(incoming.username.as_deref(), Some("ada.l"));
}

#[test]
fn a_server_without_a_host_has_no_incoming_server() {
    let account = one(r#"user_pref("mail.account.account1.server", "server1");
user_pref("mail.server.server1.type", "imap");
user_pref("mail.server.server1.hostname", "  ");"#);
    assert_eq!(account.incoming, None);
    assert_eq!(account.identity, None);
    assert_eq!(account.outgoing, None);
}

#[test]
fn without_the_account_managers_list_every_account_is_found_in_order() {
    let found = accounts(&Prefs::parse(
        r#"user_pref("mail.account.account10.server", "server10");
user_pref("mail.account.account9.server", "server9");
user_pref("mail.account.account2.server", "server2");
user_pref("mail.account.account2.identities", "id2");
user_pref("mail.account.lastLoadedVersion", 18);
user_pref("mail.server.server10.type", "pop3");
user_pref("mail.server.server9.type", "rss");
user_pref("mail.server.server2.type", "imap");"#,
    ));
    let keys: Vec<&str> = found.iter().map(|a| a.key.as_str()).collect();
    assert_eq!(keys, vec!["account2", "account9", "account10"]);
    let kinds: Vec<&AccountKind> = found.iter().map(|a| &a.kind).collect();
    assert_eq!(
        kinds,
        vec![&AccountKind::Imap, &AccountKind::Feeds, &AccountKind::Pop]
    );
}

#[test]
fn an_account_without_a_server_is_passed_over_and_a_listed_twice_once() {
    let found = accounts(&Prefs::parse(
        r#"user_pref("mail.accountmanager.accounts", "account1, account2,account1,");
user_pref("mail.account.account1.server", "server1");
user_pref("mail.server.server1.type", "ews");"#,
    ));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, AccountKind::Other("ews".into()));
}

#[test]
fn pop_and_news_have_their_own_ports_and_no_outgoing_server_for_news() {
    let found = accounts(&Prefs::parse(
        r#"user_pref("mail.accountmanager.accounts", "account1,account2");
user_pref("mail.account.account1.server", "server1");
user_pref("mail.account.account2.server", "server2");
user_pref("mail.server.server1.type", "pop3");
user_pref("mail.server.server1.hostname", "pop.example.org");
user_pref("mail.server.server2.type", "nntp");
user_pref("mail.server.server2.hostname", "news.example.org");
user_pref("mail.server.server2.socketType", 3);
user_pref("mail.smtp.defaultserver", "smtp1");
user_pref("mail.smtpserver.smtp1.hostname", "smtp.example.org");"#,
    ));
    assert_eq!(found[0].incoming.as_ref().unwrap().port, 110);
    assert!(found[0].outgoing.is_some());
    assert_eq!(found[1].incoming.as_ref().unwrap().port, 563);
    assert_eq!(found[1].outgoing, None);
}

#[test]
fn a_signature_is_taken_only_when_it_is_in_the_settings() {
    let inline = one(&format!(
        "{IMAP}user_pref(\"mail.identity.id1.htmlSigText\", \"<i>Ada</i>\");\nuser_pref(\"mail.identity.id1.htmlSigFormat\", true);"
    ));
    assert_eq!(
        inline.identity.unwrap().signature,
        Some(Signature {
            text: "<i>Ada</i>".into(),
            html: true,
        })
    );
    let from_file = one(&format!(
        "{IMAP}user_pref(\"mail.identity.id1.htmlSigText\", \"old\");\nuser_pref(\"mail.identity.id1.attach_signature\", true);"
    ));
    assert_eq!(from_file.identity.unwrap().signature, None);
    let blank = one(&format!(
        "{IMAP}user_pref(\"mail.identity.id1.htmlSigText\", \" \\n \");"
    ));
    assert_eq!(blank.identity.unwrap().signature, None);
}

#[test]
fn a_port_out_of_range_is_the_usual_one() {
    let account = one(&format!(
        "{IMAP}user_pref(\"mail.server.server1.port\", 70000);"
    ));
    assert_eq!(account.incoming.unwrap().port, 143);
}
