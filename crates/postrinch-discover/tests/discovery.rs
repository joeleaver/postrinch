//! Discovery end to end, with autoconfig files on wiremock, DNS from tables
//! and probes from a list.

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use postrinch_discover::{
    AddressError, HostedBy, Progress, Protocol, Security, Source, Step, StepState, Username,
    discover,
};
use support::{FakeProbe, FakeResolver, Reports, discoverer};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn serve(server: &MockServer, at: &str, body: &str) {
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(server)
        .await;
}

fn p(step: Step, state: StepState) -> Progress {
    Progress { step, state }
}

fn hosts(candidates: &[postrinch_discover::Candidate]) -> Vec<(&str, u16)> {
    candidates
        .iter()
        .map(|c| (c.host.as_str(), c.port))
        .collect()
}

#[tokio::test]
async fn the_domains_own_autoconfig_answers_first() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/autoconfig/example.org/mail/config-v1.1.xml"))
        .and(query_param("emailaddress", "ada+x@example.org"))
        .respond_with(ResponseTemplate::new(200).set_body_string(support::FASTMAIL))
        .mount(&server)
        .await;
    serve(&server, "/ispdb/example.org", support::YAHOO).await;
    let d = discoverer(&server, Arc::default(), Arc::default());
    let reports = Reports::default();

    let found = discover("ada+x@example.org", &d, reports.sink())
        .await
        .unwrap();

    assert_eq!(found.source, Source::DomainAutoconfig);
    assert_eq!(found.imap[0].host, "imap.fastmail.com");
    assert_eq!(
        reports.all(),
        [
            p(Step::DomainSettings, StepState::Running),
            p(Step::DomainSettings, StepState::Found)
        ]
    );
}

#[tokio::test]
async fn known_app_password_providers_are_not_oauth2_only() {
    let server = MockServer::start().await;
    serve(
        &server,
        "/autoconfig/fastmail.com/mail/config-v1.1.xml",
        support::FASTMAIL_OWN,
    )
    .await;
    let d = discoverer(&server, Arc::default(), Arc::default());

    let found = discover("ada@fastmail.com", &d, |_| {}).await.unwrap();

    assert_eq!(found.source, Source::DomainAutoconfig);
    assert!(found.app_password_url.is_some());
    assert!(!found.oauth2_only, "Fastmail takes app passwords");
}

#[tokio::test]
async fn an_oauth2_only_provider() {
    let server = MockServer::start().await;
    let xml = r#"<clientConfig version="1.1"><emailProvider id="example.org">
        <incomingServer type="imap"><hostname>imap.example.org</hostname>
          <port>993</port><socketType>SSL</socketType>
          <authentication>OAuth2</authentication></incomingServer>
        </emailProvider></clientConfig>"#;
    serve(&server, "/ispdb/example.org", xml).await;
    let d = discoverer(&server, Arc::default(), Arc::default());

    let found = discover("ada@example.org", &d, |_| {}).await.unwrap();

    assert!(found.oauth2_only);
    assert_eq!(found.app_password_url, None);
}

#[tokio::test]
async fn the_well_known_file_when_there_is_no_autoconfig_host() {
    let server = MockServer::start().await;
    serve(
        &server,
        "/wellknown/example.org/.well-known/autoconfig/mail/config-v1.1.xml",
        support::ICLOUD,
    )
    .await;
    let d = discoverer(&server, Arc::default(), Arc::default());

    let found = discover("ada@example.org", &d, |_| {}).await.unwrap();

    assert_eq!(found.source, Source::DomainAutoconfig);
    assert_eq!(found.imap[0].host, "imap.mail.me.com");
}

#[tokio::test]
async fn fastmail_from_the_ispdb() {
    let server = MockServer::start().await;
    serve(&server, "/ispdb/fastmail.com", support::FASTMAIL).await;
    let d = discoverer(&server, Arc::default(), Arc::default());
    let reports = Reports::default();

    let found = discover("ada@fastmail.com", &d, reports.sink())
        .await
        .unwrap();

    assert_eq!(found.source, Source::Ispdb);
    assert_eq!(found.provider_name.as_deref(), Some("Fastmail"));
    assert_eq!(hosts(&found.imap), [("imap.fastmail.com", 993)]);
    assert_eq!(hosts(&found.smtp), [("smtp.fastmail.com", 465)]);
    assert!(found.app_password_url.is_some());
    assert!(found.documentation_url.is_some());
    assert_eq!(found.hosted_by, None);
    assert!(!found.oauth2_only);
    assert_eq!(
        reports.all(),
        [
            p(Step::DomainSettings, StepState::Running),
            p(Step::DomainSettings, StepState::NothingFound),
            p(Step::ProviderList, StepState::Running),
            p(Step::ProviderList, StepState::Found),
        ]
    );
}

#[tokio::test]
async fn icloud_and_yahoo_from_the_ispdb() {
    let server = MockServer::start().await;
    serve(&server, "/ispdb/me.com", support::ICLOUD).await;
    serve(&server, "/ispdb/yahoo.com", support::YAHOO).await;
    let d = discoverer(&server, Arc::default(), Arc::default());

    let icloud = discover("ada@me.com", &d, |_| {}).await.unwrap();
    assert_eq!(icloud.source, Source::Ispdb);
    assert_eq!(icloud.imap[0].username, Username::LocalPart);
    assert_eq!(icloud.imap[0].username.for_address("ada@me.com"), "ada");
    assert_eq!(icloud.smtp[0].security, Security::StartTls);
    assert_eq!(icloud.smtp[0].username, Username::Address);

    let yahoo = discover("ada@yahoo.com", &d, |_| {}).await.unwrap();
    assert_eq!(yahoo.provider_name.as_deref(), Some("Yahoo! Mail"));
    assert_eq!(hosts(&yahoo.imap), [("imap.mail.yahoo.com", 993)]);
}

#[tokio::test]
async fn a_plain_text_only_answer_moves_on() {
    let server = MockServer::start().await;
    let plain = r#"<clientConfig version="1.1"><emailProvider id="example.org">
        <incomingServer type="imap"><hostname>imap.example.org</hostname>
          <port>143</port><socketType>plain</socketType></incomingServer>
        </emailProvider></clientConfig>"#;
    serve(
        &server,
        "/autoconfig/example.org/mail/config-v1.1.xml",
        plain,
    )
    .await;
    serve(&server, "/ispdb/example.org", support::YAHOO).await;
    let d = discoverer(&server, Arc::default(), Arc::default());

    let found = discover("ada@example.org", &d, |_| {}).await.unwrap();

    assert_eq!(found.source, Source::Ispdb);
}

#[tokio::test]
async fn google_workspace_from_the_mx() {
    let server = MockServer::start().await;
    serve(&server, "/ispdb/gmail.com", support::GMAIL).await;
    let resolver = Arc::new(FakeResolver::default().with_mx("acme.co", "aspmx.l.google.com"));
    let d = discoverer(&server, resolver, Arc::default());
    let reports = Reports::default();

    let found = discover("sam@acme.co", &d, reports.sink()).await.unwrap();

    assert_eq!(found.source, Source::IspdbViaMx);
    assert_eq!(found.hosted_by, Some(HostedBy::Google));
    assert_eq!(hosts(&found.imap), [("imap.gmail.com", 993)]);
    assert_eq!(found.imap[0].username, Username::Address);
    assert_eq!(
        reports.all().last(),
        Some(&p(Step::DnsRecords, StepState::Found))
    );
}

#[tokio::test]
async fn microsoft_365_is_told_even_when_its_ispdb_entry_is_missing() {
    let server = MockServer::start().await;
    let resolver =
        Arc::new(FakeResolver::default().with_mx("acme.co", "acme-co.mail.protection.outlook.com"));
    let d = discoverer(&server, resolver, Arc::default());

    let found = discover("sam@acme.co", &d, |_| {}).await.unwrap();

    assert_eq!(found.source, Source::Nothing);
    assert_eq!(found.hosted_by, Some(HostedBy::Microsoft));
}

#[tokio::test]
async fn another_mx_is_looked_up_by_its_registrable_domain() {
    let server = MockServer::start().await;
    serve(&server, "/ispdb/messagingengine.com", support::FASTMAIL).await;
    let resolver =
        Arc::new(FakeResolver::default().with_mx("example.org", "in1-smtp.messagingengine.com"));
    let d = discoverer(&server, resolver, Arc::default());

    let found = discover("ada@example.org", &d, |_| {}).await.unwrap();

    assert_eq!(found.source, Source::IspdbViaMx);
    assert_eq!(found.provider_name.as_deref(), Some("Fastmail"));
    assert_eq!(found.hosted_by, None);
    assert!(
        !found.confirm_servers,
        "the ISPDB vouches for servers outside the domain"
    );
}

#[tokio::test]
async fn srv_records_best_first_without_the_ones_not_offered() {
    let server = MockServer::start().await;
    let resolver = Arc::new(
        FakeResolver::default()
            .with_srv("_imaps._tcp.example.org", 20, 0, "backup.example.org", 993)
            .with_srv("_imaps._tcp.example.org", 10, 5, "light.example.org", 993)
            .with_srv("_imaps._tcp.example.org", 10, 50, "heavy.example.org", 993)
            .with_srv("_imap._tcp.example.org", 0, 0, ".", 0)
            .with_srv(
                "_submission._tcp.example.org",
                0,
                0,
                "smtp.example.org",
                587,
            )
            .with_srv(
                "_submissions._tcp.example.org",
                0,
                0,
                "smtp.example.org",
                465,
            ),
    );
    let d = discoverer(&server, resolver.clone(), Arc::default());

    let found = discover("ada@example.org", &d, |_| {}).await.unwrap();

    assert_eq!(found.source, Source::Srv);
    assert_eq!(
        hosts(&found.imap),
        [
            ("heavy.example.org", 993),
            ("light.example.org", 993),
            ("backup.example.org", 993)
        ]
    );
    assert!(found.imap.iter().all(|c| c.security == Security::Tls));
    assert_eq!(
        hosts(&found.smtp),
        [("smtp.example.org", 465), ("smtp.example.org", 587)]
    );
    assert_eq!(found.smtp[1].security, Security::StartTls);
    assert_eq!(resolver.asked()[0], "MX example.org");
    assert!(!found.confirm_servers, "every server is within the domain");
}

#[tokio::test]
async fn srv_servers_outside_the_domain_are_for_the_person_to_confirm() {
    for (imap, smtp) in [
        ("imap.elsewhere.net", "smtp.example.org"),
        ("mail.example.org", "smtp.notexample.org"),
    ] {
        let server = MockServer::start().await;
        let resolver = Arc::new(
            FakeResolver::default()
                .with_srv("_imaps._tcp.example.org", 0, 0, imap, 993)
                .with_srv("_submissions._tcp.example.org", 0, 0, smtp, 465),
        );
        let d = discoverer(&server, resolver, Arc::default());

        let found = discover("ada@example.org", &d, |_| {}).await.unwrap();

        assert_eq!(found.source, Source::Srv);
        assert!(found.confirm_servers, "{imap}, {smtp}");
    }
}

#[tokio::test]
async fn srv_without_smtp_probes_for_it() {
    let server = MockServer::start().await;
    let resolver = Arc::new(FakeResolver::default().with_srv(
        "_imap._tcp.example.org",
        0,
        0,
        "imap.example.org",
        143,
    ));
    let prober = Arc::new(FakeProbe::answering(&[("smtp.example.org", 587)]));
    let d = discoverer(&server, resolver, prober.clone());

    let found = discover("ada@example.org", &d, |_| {}).await.unwrap();

    assert_eq!(found.source, Source::Srv);
    assert_eq!(found.imap[0].security, Security::StartTls);
    assert_eq!(hosts(&found.smtp), [("smtp.example.org", 587)]);
    assert!(
        prober
            .probed()
            .iter()
            .all(|(proto, ..)| *proto == Protocol::Smtp)
    );
}

#[tokio::test]
async fn common_names_probed_tls_first() {
    let server = MockServer::start().await;
    let prober = Arc::new(FakeProbe::answering(&[
        ("imap.example.org", 143),
        ("mail.example.org", 993),
        ("smtp.example.org", 587),
        ("mail.example.org", 465),
    ]));
    let d = discoverer(&server, Arc::default(), prober.clone());
    let reports = Reports::default();

    let found = discover("ada@example.org", &d, reports.sink())
        .await
        .unwrap();

    assert_eq!(found.source, Source::Guess);
    assert!(!found.confirm_servers, "guesses are within the domain");
    assert_eq!(
        hosts(&found.imap),
        [("mail.example.org", 993), ("imap.example.org", 143)]
    );
    assert_eq!(found.imap[0].security, Security::Tls);
    assert_eq!(found.imap[1].security, Security::StartTls);
    assert_eq!(
        hosts(&found.smtp),
        [("mail.example.org", 465), ("smtp.example.org", 587)]
    );
    assert_eq!(prober.probed().len(), 8);
    assert_eq!(
        reports.all().last(),
        Some(&p(Step::CommonNames, StepState::Found))
    );
}

#[tokio::test]
async fn nothing_found() {
    let server = MockServer::start().await;
    let d = discoverer(&server, Arc::default(), Arc::default());
    let reports = Reports::default();

    let found = discover("ada@example.org", &d, reports.sink())
        .await
        .unwrap();

    assert_eq!(found.source, Source::Nothing);
    assert!(found.imap.is_empty() && found.smtp.is_empty());
    let nothing: Vec<Step> = reports
        .all()
        .iter()
        .filter(|r| r.state == StepState::NothingFound)
        .map(|r| r.step)
        .collect();
    assert_eq!(
        nothing,
        [
            Step::DomainSettings,
            Step::ProviderList,
            Step::DnsRecords,
            Step::CommonNames
        ]
    );
}

#[tokio::test]
async fn slow_steps_time_out_and_move_on() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/autoconfig/example.org/mail/config-v1.1.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(support::FASTMAIL)
                .set_delay(Duration::from_secs(30)),
        )
        .mount(&server)
        .await;
    let resolver = Arc::new(FakeResolver {
        hang: true,
        ..FakeResolver::default()
    });
    let prober = Arc::new(FakeProbe::answering(&[("imap.example.org", 993)]));
    let d = discoverer(&server, resolver, prober);
    let started = Instant::now();

    let found = discover("ada@example.org", &d, |_| {}).await.unwrap();

    assert_eq!(found.source, Source::Guess);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn an_address_that_is_not_one() {
    let server = MockServer::start().await;
    let d = discoverer(&server, Arc::default(), Arc::default());
    assert!(matches!(
        discover("ada", &d, |_| {}).await,
        Err(AddressError::NotAnAddress(_))
    ));
    assert!(matches!(
        discover("ada@127.0.0.1", &d, |_| {}).await,
        Err(AddressError::BadDomain(_))
    ));
}
