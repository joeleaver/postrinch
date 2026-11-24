use super::*;

const FASTMAIL: &str = include_str!("../../tests/fixtures/fastmail.com.xml");
const ICLOUD: &str = include_str!("../../tests/fixtures/icloud.com.xml");
const YAHOO: &str = include_str!("../../tests/fixtures/yahoo.com.xml");
const GMAIL: &str = include_str!("../../tests/fixtures/gmail.com.xml");
const FASTMAIL_OWN: &str = include_str!("../../tests/fixtures/fastmail-autoconfig.xml");

#[test]
fn fastmails_own_file_lists_oauth2_only_and_instruction_pages() {
    let c = parse(FASTMAIL_OWN, &address("ada@fastmail.com")).unwrap();
    assert!(c.oauth2_only);
    assert_eq!(
        c.documentation_url.as_deref(),
        Some("https://www.fastmail.help/hc/en-us/articles/1500000278342")
    );
}

#[test]
fn the_first_https_page_is_taken() {
    let xml = config(
        r#"<documentation url="http://example.org/plain"/>
           <documentation url="https://example.org/secure"/>
           <enable visiturl="ftp://example.org/x"/>"#,
    );
    let c = parse(&xml, &address("ada@example.org")).unwrap();
    assert_eq!(
        c.documentation_url.as_deref(),
        Some("https://example.org/secure")
    );
    assert_eq!(c.enable_url, None);
}

fn address(text: &str) -> Address {
    Address::parse(text).unwrap()
}

fn server(host: &str, port: u16, security: Security, username: Username) -> Candidate {
    Candidate {
        host: host.to_owned(),
        port,
        security,
        username,
    }
}

/// A clientConfig with one provider holding `servers`.
fn config(servers: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
        <clientConfig version="1.1"><emailProvider id="example.org">
          <domain>example.org</domain><displayName>Example</displayName>
          {servers}
        </emailProvider></clientConfig>"#
    )
}

#[test]
fn fastmail() {
    let c = parse(FASTMAIL, &address("ada@fastmail.com")).unwrap();
    assert_eq!(c.provider_name.as_deref(), Some("Fastmail"));
    assert_eq!(
        c.imap,
        [server(
            "imap.fastmail.com",
            993,
            Security::Tls,
            Username::Address
        )]
    );
    assert_eq!(
        c.smtp,
        [server(
            "smtp.fastmail.com",
            465,
            Security::Tls,
            Username::Address
        )]
    );
    assert_eq!(
        c.documentation_url.as_deref(),
        Some("https://www.fastmail.help/hc/en-us/articles/1500000278342")
    );
    assert_eq!(c.enable_url, None);
    assert!(!c.oauth2_only, "Fastmail also takes passwords");
}

#[test]
fn icloud_signs_imap_in_with_the_local_part() {
    let c = parse(ICLOUD, &address("ada@me.com")).unwrap();
    assert_eq!(c.provider_name.as_deref(), Some("iCloud Mail"));
    assert_eq!(
        c.imap,
        [server(
            "imap.mail.me.com",
            993,
            Security::Tls,
            Username::LocalPart
        )]
    );
    assert_eq!(
        c.smtp,
        [server(
            "smtp.mail.me.com",
            587,
            Security::StartTls,
            Username::Address
        )]
    );
}

#[test]
fn yahoo_ignores_pop3() {
    let c = parse(YAHOO, &address("ada@yahoo.com")).unwrap();
    assert_eq!(c.provider_name.as_deref(), Some("Yahoo! Mail"));
    assert_eq!(c.imap.len(), 1);
    assert_eq!(c.imap[0].host, "imap.mail.yahoo.com");
    assert_eq!(c.smtp[0].host, "smtp.mail.yahoo.com");
}

#[test]
fn gmail_pages_https_only_and_unescaped() {
    let c = parse(GMAIL, &address("ada@gmail.com")).unwrap();
    assert_eq!(
        c.enable_url.as_deref(),
        Some("https://mail.google.com/mail/?ui=2&shva=1#settings/fwdandpop")
    );
    assert_eq!(c.documentation_url, None, "a plain-HTTP page is dropped");
}

#[test]
fn plain_text_servers_are_dropped() {
    let xml = config(
        r#"<incomingServer type="imap"><hostname>imap.example.org</hostname>
             <port>143</port><socketType>plain</socketType></incomingServer>
           <incomingServer type="imap"><hostname>nosocket.example.org</hostname>
             <port>143</port></incomingServer>
           <outgoingServer type="smtp"><hostname>smtp.example.org</hostname>
             <port>25</port><socketType>plain</socketType></outgoingServer>"#,
    );
    let c = parse(&xml, &address("ada@example.org")).unwrap();
    assert!(c.imap.is_empty());
    assert!(c.smtp.is_empty());
}

#[test]
fn several_servers_tls_first_duplicates_dropped() {
    let xml = config(
        r#"<incomingServer type="imap"><hostname>imap.example.org</hostname>
             <port>143</port><socketType>STARTTLS</socketType></incomingServer>
           <incomingServer type="imap"><hostname>imap.example.org</hostname>
             <port>993</port><socketType>SSL</socketType></incomingServer>
           <incomingServer type="imap"><hostname>imap2.example.org</hostname>
             <port>993</port><socketType>SSL</socketType></incomingServer>
           <incomingServer type="imap"><hostname>IMAP.example.org.</hostname>
             <port>993</port><socketType>SSL</socketType></incomingServer>
           <outgoingServer type="smtp"><hostname>smtp.example.org</hostname>
             <port>587</port><socketType>STARTTLS</socketType></outgoingServer>
           <outgoingServer type="smtp"><hostname>smtp.example.org</hostname>
             <port>465</port><socketType>SSL</socketType></outgoingServer>"#,
    );
    let c = parse(&xml, &address("ada@example.org")).unwrap();
    let imap: Vec<_> = c.imap.iter().map(|s| (s.host.as_str(), s.port)).collect();
    assert_eq!(
        imap,
        [
            ("imap.example.org", 993),
            ("imap2.example.org", 993),
            ("imap.example.org", 143)
        ]
    );
    let smtp: Vec<_> = c.smtp.iter().map(|s| s.port).collect();
    assert_eq!(smtp, [465, 587]);
}

#[test]
fn placeholders_are_filled_in() {
    let xml = config(
        r#"<incomingServer type="imap"><hostname>mail.%EMAILDOMAIN%</hostname>
             <port>993</port><socketType>SSL</socketType>
             <username>%EMAILLOCALPART%+imap@%EMAILDOMAIN%</username></incomingServer>
           <outgoingServer type="smtp"><hostname>smtp.example.org</hostname>
             <port>465</port><socketType>SSL</socketType>
             <username>shared-sender</username></outgoingServer>"#,
    );
    let c = parse(&xml, &address("Ada@Example.org")).unwrap();
    assert_eq!(c.imap[0].host, "mail.example.org");
    assert_eq!(
        c.imap[0].username,
        Username::Literal("Ada+imap@example.org".to_owned())
    );
    assert_eq!(
        c.smtp[0].username,
        Username::Literal("shared-sender".to_owned())
    );
}

#[test]
fn oauth2_only_providers() {
    let xml = config(
        r#"<incomingServer type="imap"><hostname>imap.example.org</hostname>
             <port>993</port><socketType>SSL</socketType>
             <authentication>OAuth2</authentication></incomingServer>
           <incomingServer type="pop3"><hostname>pop.example.org</hostname>
             <port>995</port><socketType>SSL</socketType>
             <authentication>password-cleartext</authentication></incomingServer>"#,
    );
    let c = parse(&xml, &address("ada@example.org")).unwrap();
    assert!(c.oauth2_only);

    let no_auth_listed = config(
        r#"<incomingServer type="imap"><hostname>imap.example.org</hostname>
             <port>993</port><socketType>SSL</socketType></incomingServer>"#,
    );
    let c = parse(&no_auth_listed, &address("ada@example.org")).unwrap();
    assert!(!c.oauth2_only);
}

#[test]
fn unusable_servers_are_dropped() {
    let xml = config(
        r#"<incomingServer type="imap"><hostname></hostname>
             <port>993</port><socketType>SSL</socketType></incomingServer>
           <incomingServer type="imap"><hostname>imap.example.org</hostname>
             <port>99999</port><socketType>SSL</socketType></incomingServer>
           <incomingServer type="imap"><hostname>bad host</hostname>
             <port>993</port><socketType>SSL</socketType></incomingServer>
           <incomingServer type="imap"><hostname>192.0.2.1</hostname>
             <port>993</port><socketType>SSL</socketType></incomingServer>"#,
    );
    let c = parse(&xml, &address("ada@example.org")).unwrap();
    assert!(c.imap.is_empty());
}

#[test]
fn only_the_first_provider_is_read() {
    let xml = r#"<clientConfig version="1.1">
        <emailProvider id="a"><displayName>First</displayName>
          <incomingServer type="imap"><hostname>imap.a.example</hostname>
            <port>993</port><socketType>SSL</socketType></incomingServer>
        </emailProvider>
        <emailProvider id="b"><displayName>Second</displayName>
          <incomingServer type="imap"><hostname>imap.b.example</hostname>
            <port>993</port><socketType>SSL</socketType></incomingServer>
        </emailProvider></clientConfig>"#;
    let c = parse(xml, &address("ada@a.example")).unwrap();
    assert_eq!(c.provider_name.as_deref(), Some("First"));
    assert_eq!(c.imap.len(), 1);
    assert_eq!(c.imap[0].host, "imap.a.example");
}

#[test]
fn short_name_when_there_is_no_display_name() {
    let xml = r#"<clientConfig><emailProvider id="x">
        <displayShortName>Short</displayShortName></emailProvider></clientConfig>"#;
    let c = parse(xml, &address("ada@example.org")).unwrap();
    assert_eq!(c.provider_name.as_deref(), Some("Short"));
}

#[test]
fn documents_that_are_not_autoconfig() {
    let a = address("ada@example.org");
    assert_eq!(
        parse("<html><body>Not here</body></html>", &a),
        Err(XmlError::NotClientConfig)
    );
    assert_eq!(parse("", &a), Err(XmlError::NotClientConfig));
    assert!(matches!(
        parse("<clientConfig><emailProvider></clientConfig>", &a),
        Err(XmlError::Malformed(_))
    ));
}

#[test]
fn other_top_level_elements_fill_nothing() {
    let xml = r#"<clientConfig version="1.1">
        <emailProvider id="a">
          <incomingServer type="imap"><hostname>imap.a.example</hostname>
            <port>993</port><socketType>SSL</socketType></incomingServer>
        </emailProvider>
        <webMail>
          <displayName>Not the provider</displayName>
          <documentation url="https://elsewhere.example/docs"/>
          <enable visiturl="https://elsewhere.example/enable"/>
          <incomingServer type="imap"><hostname>imap.elsewhere.example</hostname>
            <port>993</port><socketType>SSL</socketType></incomingServer>
        </webMail></clientConfig>"#;
    let c = parse(xml, &address("ada@a.example")).unwrap();
    assert_eq!(c.provider_name, None);
    assert_eq!(c.documentation_url, None);
    assert_eq!(c.enable_url, None);
    assert_eq!(hosts(&c.imap), ["imap.a.example"]);
}

#[test]
fn unicode_host_names_are_stored_in_ascii() {
    let xml = config(
        r#"<incomingServer type="imap"><hostname>IMAP.Bücher.example</hostname>
             <port>993</port><socketType>SSL</socketType></incomingServer>
           <outgoingServer type="smtp"><hostname>smtp.%EMAILDOMAIN%</hostname>
             <port>465</port><socketType>SSL</socketType></outgoingServer>"#,
    );
    let c = parse(&xml, &address("jo@bücher.de")).unwrap();
    assert_eq!(hosts(&c.imap), ["imap.xn--bcher-kva.example"]);
    assert_eq!(hosts(&c.smtp), ["smtp.xn--bcher-kva.de"]);
}

#[test]
fn placeholders_the_address_brings_in_are_not_filled() {
    let xml = config(
        r#"<incomingServer type="imap"><hostname>imap.example.org</hostname>
             <port>993</port><socketType>SSL</socketType>
             <username>%EMAILLOCALPART%+%EMAILDOMAIN%</username></incomingServer>
           <outgoingServer type="smtp"><hostname>smtp.example.org</hostname>
             <port>465</port><socketType>SSL</socketType>
             <username>x/%EMAILADDRESS%/100%</username></outgoingServer>"#,
    );
    let c = parse(&xml, &address("%EMAILDOMAIN%@example.org")).unwrap();
    assert_eq!(
        c.imap[0].username,
        Username::Literal("%EMAILDOMAIN%+example.org".to_owned())
    );
    assert_eq!(
        c.smtp[0].username,
        Username::Literal("x/%EMAILDOMAIN%@example.org/100%".to_owned())
    );
}

fn hosts(servers: &[Candidate]) -> Vec<&str> {
    servers.iter().map(|s| s.host.as_str()).collect()
}
