//! Discovery's steps, each answering or not. Every lookup has a timeout,
//! and a lookup that fails is the same as one that finds nothing.

use std::future::Future;
use std::time::Duration;

use tokio::task::JoinSet;

use crate::address::Address;
use crate::discoverer::{Discoverer, Endpoints};
use crate::dns::SrvRecord;
use crate::probe::Protocol;
use crate::providers;
use crate::types::{Candidate, Discovery, HostedBy, Security, Source, Username};
use crate::xml::{self, ProviderConfig};

/// Runs `future` for at most `limit`, logging what went wrong.
async fn within<T, E: std::fmt::Display>(
    limit: Duration,
    what: &str,
    future: impl Future<Output = Result<T, E>>,
) -> Option<T> {
    match tokio::time::timeout(limit, future).await {
        Ok(Ok(value)) => Some(value),
        Ok(Err(e)) => {
            tracing::debug!(%e, "discovery: {what}");
            None
        }
        Err(_) => {
            tracing::debug!("discovery: {what}: timed out");
            None
        }
    }
}

/// Fetches and reads one autoconfig file; `None` unless it offers an IMAP
/// server.
async fn fetch_config(d: &Discoverer, url: &str, address: &Address) -> Option<ProviderConfig> {
    let body = within(d.timeouts.http, url, d.fetch.get(url)).await?;
    let config = match xml::parse(&body, address) {
        Ok(config) => config,
        Err(e) => {
            tracing::debug!(%e, "discovery: {url}");
            return None;
        }
    };
    if config.imap.is_empty() {
        tracing::debug!("discovery: {url} offers no IMAP server that is not plain text");
        return None;
    }
    Some(config)
}

/// A discovery from an autoconfig file.
fn from_config(config: ProviderConfig, source: Source) -> Discovery {
    let app_password_url = providers::app_password_url(&config.imap);
    Discovery {
        source,
        hosted_by: providers::hosted_by_servers(&config.imap),
        // Some providers list only OAuth2 in their files (Fastmail's own
        // autoconfig does) yet take app passwords: what Postrinch knows
        // about them wins.
        oauth2_only: config.oauth2_only && app_password_url.is_none(),
        app_password_url,
        provider_name: config.provider_name,
        imap: config.imap,
        smtp: config.smtp,
        documentation_url: config.documentation_url,
        enable_url: config.enable_url,
    }
}

/// The domain's own autoconfig files: `autoconfig.<domain>`, then its
/// `.well-known` copy.
pub(crate) async fn domain_autoconfig(d: &Discoverer, address: &Address) -> Option<Discovery> {
    for template in [&d.endpoints.autoconfig, &d.endpoints.well_known] {
        let url = Endpoints::url(template, &address.domain, &address.full);
        if let Some(config) = fetch_config(d, &url, address).await {
            return Some(from_config(config, Source::DomainAutoconfig));
        }
    }
    None
}

/// Thunderbird's ISPDB entry for `domain`.
pub(crate) async fn ispdb(
    d: &Discoverer,
    address: &Address,
    domain: &str,
    source: Source,
) -> Option<Discovery> {
    let url = Endpoints::url(&d.endpoints.ispdb, domain, &address.full);
    let config = fetch_config(d, &url, address).await?;
    Some(from_config(config, source))
}

/// What the MX step learned.
#[derive(Debug, Default)]
pub(crate) struct MxOutcome {
    /// The ISPDB's answer for the mail exchanger's provider.
    pub found: Option<Discovery>,
    /// Google or Microsoft hosts the domain's mail, whatever else is found.
    pub hosted_by: Option<HostedBy>,
}

/// The domain's MX records: Google and Microsoft are recognised, and any
/// other mail exchanger's registrable domain is looked up in the ISPDB.
pub(crate) async fn mx(d: &Discoverer, address: &Address) -> MxOutcome {
    let what = format!("MX of {}", address.domain);
    let Some(mut records) = within(d.timeouts.dns, &what, d.resolver.mx(&address.domain)).await
    else {
        return MxOutcome::default();
    };
    records.sort_by_key(|r| r.preference);
    // A null MX (RFC 7505) says the domain takes no mail.
    let Some(exchange) = records
        .into_iter()
        .map(|r| r.exchange)
        .find(|e| !e.is_empty() && e != ".")
    else {
        return MxOutcome::default();
    };
    if let Some(host) = providers::hosted_by_mx(&exchange) {
        let domain = providers::ispdb_domain(host);
        let found = ispdb(d, address, domain, Source::IspdbViaMx)
            .await
            .map(|found| Discovery {
                hosted_by: Some(host),
                ..found
            });
        return MxOutcome {
            found,
            hosted_by: Some(host),
        };
    }
    let Some(domain) = psl::domain_str(&exchange) else {
        return MxOutcome::default();
    };
    if domain == address.domain {
        // The ISPDB has already been asked about this domain.
        return MxOutcome::default();
    }
    MxOutcome {
        found: ispdb(d, address, domain, Source::IspdbViaMx).await,
        hosted_by: None,
    }
}

/// SRV records at `_<service>._tcp.<domain>`, best first, without the ones
/// that say the service is not offered.
async fn srv_candidates(
    d: &Discoverer,
    address: &Address,
    service: &str,
    security: Security,
) -> Vec<Candidate> {
    let name = format!("_{service}._tcp.{}", address.domain);
    let mut records: Vec<SrvRecord> = within(d.timeouts.dns, &name, d.resolver.srv(&name))
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.port != 0 && !r.target.is_empty() && r.target != ".")
        .collect();
    records.sort_by(|a, b| a.priority.cmp(&b.priority).then(b.weight.cmp(&a.weight)));
    records
        .into_iter()
        .map(|r| Candidate {
            host: r.target.to_ascii_lowercase(),
            port: r.port,
            security,
            username: Username::Address,
        })
        .collect()
}

/// The domain's SRV records (RFC 6186, RFC 8314). Answers when there is an
/// IMAP server; a missing SMTP side is filled in by probing common names.
pub(crate) async fn srv(d: &Discoverer, address: &Address) -> Option<Discovery> {
    let (imaps, imap, submissions, submission) = tokio::join!(
        srv_candidates(d, address, "imaps", Security::Tls),
        srv_candidates(d, address, "imap", Security::StartTls),
        srv_candidates(d, address, "submissions", Security::Tls),
        srv_candidates(d, address, "submission", Security::StartTls),
    );
    let imap: Vec<Candidate> = imaps.into_iter().chain(imap).collect();
    if imap.is_empty() {
        return None;
    }
    let mut smtp: Vec<Candidate> = submissions.into_iter().chain(submission).collect();
    if smtp.is_empty() {
        smtp = guess(d, address, Protocol::Smtp).await;
    }
    Some(Discovery {
        source: Source::Srv,
        hosted_by: providers::hosted_by_servers(&imap),
        app_password_url: providers::app_password_url(&imap),
        imap,
        smtp,
        ..Discovery::nothing()
    })
}

/// The common names for `protocol` at `domain`, TLS first.
pub(crate) fn guesses(protocol: Protocol, domain: &str) -> Vec<Candidate> {
    let (prefixes, tls_port, starttls_port) = match protocol {
        Protocol::Imap => (["imap", "mail"], 993, 143),
        Protocol::Smtp => (["smtp", "mail"], 465, 587),
    };
    [
        (Security::Tls, tls_port),
        (Security::StartTls, starttls_port),
    ]
    .into_iter()
    .flat_map(|(security, port)| {
        prefixes.into_iter().map(move |prefix| Candidate {
            host: format!("{prefix}.{domain}"),
            port,
            security,
            username: Username::Address,
        })
    })
    .collect()
}

/// Probes the common names for `protocol` at once; the ones that answer,
/// in [`guesses`] order.
pub(crate) async fn guess(d: &Discoverer, address: &Address, protocol: Protocol) -> Vec<Candidate> {
    let candidates = guesses(protocol, &address.domain);
    let mut probes = JoinSet::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let prober = d.prober.clone();
        let limit = d.timeouts.probe;
        let (host, port, security) = (candidate.host.clone(), candidate.port, candidate.security);
        probes.spawn(async move {
            let what = format!("probe {host}:{port}");
            let probe = prober.probe(protocol, &host, port, security);
            (index, within(limit, &what, probe).await.is_some())
        });
    }
    let mut answered = vec![false; candidates.len()];
    while let Some(joined) = probes.join_next().await {
        match joined {
            Ok((index, ok)) => answered[index] = ok,
            Err(e) => tracing::debug!(%e, "discovery: a probe did not finish"),
        }
    }
    candidates
        .into_iter()
        .zip(answered)
        .filter_map(|(candidate, ok)| ok.then_some(candidate))
        .collect()
}

/// Common server names, probed. Answers when an IMAP server answers.
pub(crate) async fn common_names(d: &Discoverer, address: &Address) -> Option<Discovery> {
    let (imap, smtp) = tokio::join!(
        guess(d, address, Protocol::Imap),
        guess(d, address, Protocol::Smtp)
    );
    if imap.is_empty() {
        return None;
    }
    Some(Discovery {
        source: Source::Guess,
        imap,
        smtp,
        ..Discovery::nothing()
    })
}
