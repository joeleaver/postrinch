//! Stand-ins for the network: a fetcher that talks plain HTTP to wiremock,
//! a resolver answering from tables, and a prober answering from a list.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use postrinch_discover::{
    BoxFuture, Discoverer, DnsError, Endpoints, Fetch, FetchError, MxRecord, Probe, ProbeError,
    Progress, Protocol, Resolve, Security, SrvRecord, Timeouts,
};
use wiremock::MockServer;

/// The fixtures, as the ISPDB would serve them.
pub const FASTMAIL: &str = include_str!("../fixtures/fastmail.com.xml");
pub const ICLOUD: &str = include_str!("../fixtures/icloud.com.xml");
pub const YAHOO: &str = include_str!("../fixtures/yahoo.com.xml");
pub const GMAIL: &str = include_str!("../fixtures/gmail.com.xml");
pub const FASTMAIL_OWN: &str = include_str!("../fixtures/fastmail-autoconfig.xml");

/// Fetches over plain HTTP, which only a test server on loopback should
/// ever be asked for.
pub struct LoopbackFetch(reqwest::Client);

impl LoopbackFetch {
    pub fn new() -> Self {
        LoopbackFetch(reqwest::Client::new())
    }

    async fn fetch(&self, url: &str) -> Result<String, FetchError> {
        assert!(
            url.starts_with("http://127.0.0.1:"),
            "tests fetch only from wiremock, not {url}"
        );
        let response = self
            .0
            .get(url)
            .send()
            .await
            .map_err(|e| FetchError::Http(e.to_string()))?;
        match response.status().as_u16() {
            404 | 410 => Err(FetchError::NotFound),
            200..=299 => response
                .text()
                .await
                .map_err(|e| FetchError::Http(e.to_string())),
            status => Err(FetchError::Status(status)),
        }
    }
}

impl Fetch for LoopbackFetch {
    fn get<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<String, FetchError>> {
        Box::pin(self.fetch(url))
    }
}

/// Answers MX and SRV lookups from tables; names not in them have no
/// records. `hang` makes every lookup wait forever.
#[derive(Default)]
pub struct FakeResolver {
    pub mx: HashMap<String, Vec<MxRecord>>,
    pub srv: HashMap<String, Vec<SrvRecord>>,
    pub hang: bool,
    pub asked: Mutex<Vec<String>>,
}

impl FakeResolver {
    pub fn with_mx(mut self, domain: &str, exchange: &str) -> Self {
        self.mx
            .entry(domain.to_owned())
            .or_default()
            .push(MxRecord {
                preference: 10,
                exchange: exchange.to_owned(),
            });
        self
    }

    pub fn with_srv(
        mut self,
        name: &str,
        priority: u16,
        weight: u16,
        target: &str,
        port: u16,
    ) -> Self {
        self.srv
            .entry(name.to_owned())
            .or_default()
            .push(SrvRecord {
                priority,
                weight,
                port,
                target: target.to_owned(),
            });
        self
    }

    pub fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }

    async fn answer<T: Clone>(
        &self,
        kind: &str,
        table: &HashMap<String, Vec<T>>,
        name: &str,
    ) -> Result<Vec<T>, DnsError> {
        self.asked.lock().unwrap().push(format!("{kind} {name}"));
        if self.hang {
            std::future::pending::<()>().await;
        }
        Ok(table.get(name).cloned().unwrap_or_default())
    }
}

impl Resolve for FakeResolver {
    fn mx<'a>(&'a self, domain: &'a str) -> BoxFuture<'a, Result<Vec<MxRecord>, DnsError>> {
        Box::pin(self.answer("MX", &self.mx, domain))
    }

    fn srv<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Vec<SrvRecord>, DnsError>> {
        Box::pin(self.answer("SRV", &self.srv, name))
    }
}

/// Answers probes of the `host:port`s listed; everything else refuses.
#[derive(Default)]
pub struct FakeProbe {
    pub answering: Vec<(String, u16)>,
    pub probed: Mutex<Vec<(Protocol, String, u16, Security)>>,
}

impl FakeProbe {
    pub fn answering(hosts: &[(&str, u16)]) -> Self {
        FakeProbe {
            answering: hosts.iter().map(|(h, p)| ((*h).to_owned(), *p)).collect(),
            probed: Mutex::default(),
        }
    }

    pub fn probed(&self) -> Vec<(Protocol, String, u16, Security)> {
        self.probed.lock().unwrap().clone()
    }
}

impl Probe for FakeProbe {
    fn probe<'a>(
        &'a self,
        protocol: Protocol,
        host: &'a str,
        port: u16,
        security: Security,
    ) -> BoxFuture<'a, Result<(), ProbeError>> {
        self.probed
            .lock()
            .unwrap()
            .push((protocol, host.to_owned(), port, security));
        let ok = self.answering.iter().any(|(h, p)| h == host && *p == port);
        Box::pin(async move {
            if ok {
                Ok(())
            } else {
                Err(ProbeError::Connect("refused".to_owned()))
            }
        })
    }
}

/// The endpoints, all on `server`.
pub fn endpoints(server: &MockServer) -> Endpoints {
    let base = server.uri();
    Endpoints {
        autoconfig: format!(
            "{base}/autoconfig/{{domain}}/mail/config-v1.1.xml?emailaddress={{address}}"
        ),
        well_known: format!(
            "{base}/wellknown/{{domain}}/.well-known/autoconfig/mail/config-v1.1.xml"
        ),
        ispdb: format!("{base}/ispdb/{{domain}}"),
    }
}

/// Short timeouts, so a test that waits for one does not take long.
pub fn short_timeouts() -> Timeouts {
    Timeouts {
        http: Duration::from_millis(500),
        dns: Duration::from_millis(300),
        probe: Duration::from_millis(300),
    }
}

/// A discoverer on `server`, `resolver` and `prober`.
pub fn discoverer(
    server: &MockServer,
    resolver: Arc<FakeResolver>,
    prober: Arc<FakeProbe>,
) -> Discoverer {
    Discoverer::with_parts(Arc::new(LoopbackFetch::new()), resolver, prober)
        .with_endpoints(endpoints(server))
        .with_timeouts(short_timeouts())
}

/// Collects progress reports.
#[derive(Clone, Default)]
pub struct Reports(Arc<Mutex<Vec<Progress>>>);

impl Reports {
    pub fn sink(&self) -> impl FnMut(Progress) + Send + use<> {
        let reports = self.0.clone();
        move |p| reports.lock().unwrap().push(p)
    }

    pub fn all(&self) -> Vec<Progress> {
        self.0.lock().unwrap().clone()
    }
}
