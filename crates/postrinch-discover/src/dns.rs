//! DNS lookups discovery needs (MX and SRV): a trait, so tests can answer
//! them, and the real resolver on `hickory-resolver`.

use hickory_resolver::TokioResolver;
use hickory_resolver::net::NetError;
use hickory_resolver::proto::rr::{Name, RData};
use thiserror::Error;

use crate::BoxFuture;

/// A lookup that failed. A name with no records of the type asked for is
/// not a failure: it answers an empty list.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DnsError {
    /// The system's resolver configuration could not be read.
    #[error("could not set up DNS: {0}")]
    Setup(String),
    /// The lookup failed (no answer, a server failure, a timeout).
    #[error("DNS lookup failed: {0}")]
    Lookup(String),
}

/// An MX record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MxRecord {
    /// Lower is preferred.
    pub preference: u16,
    /// The mail exchanger's host name, lower case, without a trailing dot.
    pub exchange: String,
}

/// An SRV record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrvRecord {
    /// Lower is preferred.
    pub priority: u16,
    /// Among records of the same priority, higher is preferred.
    pub weight: u16,
    /// The TCP port.
    pub port: u16,
    /// The host name, lower case, without a trailing dot; `"."` when the
    /// record says the service is not offered (RFC 2782).
    pub target: String,
}

/// Looks up MX and SRV records.
pub trait Resolve: Send + Sync {
    /// The MX records of `domain`.
    fn mx<'a>(&'a self, domain: &'a str) -> BoxFuture<'a, Result<Vec<MxRecord>, DnsError>>;
    /// The SRV records at `name` (such as `_imaps._tcp.example.org`).
    fn srv<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Vec<SrvRecord>, DnsError>>;
}

/// The real resolver, configured from the system (`/etc/resolv.conf` on
/// Unix, the registry on Windows).
#[derive(Clone)]
pub struct HickoryResolver {
    inner: TokioResolver,
}

impl std::fmt::Debug for HickoryResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HickoryResolver")
    }
}

impl HickoryResolver {
    /// A resolver using the system's DNS servers.
    pub fn from_system() -> Result<Self, DnsError> {
        let inner = TokioResolver::builder_tokio()
            .and_then(|builder| builder.build())
            .map_err(|e| DnsError::Setup(e.to_string()))?;
        Ok(HickoryResolver { inner })
    }

    async fn lookup_mx(&self, domain: &str) -> Result<Vec<MxRecord>, DnsError> {
        let lookup = match self.inner.mx_lookup(absolute(domain)).await {
            Ok(lookup) => lookup,
            Err(e) => return empty_or_error(e),
        };
        Ok(lookup
            .answers()
            .iter()
            .filter_map(|record| match &record.data {
                RData::MX(mx) => Some(MxRecord {
                    preference: mx.preference,
                    exchange: host_name(&mx.exchange),
                }),
                _ => None,
            })
            .collect())
    }

    async fn lookup_srv(&self, name: &str) -> Result<Vec<SrvRecord>, DnsError> {
        let lookup = match self.inner.srv_lookup(absolute(name)).await {
            Ok(lookup) => lookup,
            Err(e) => return empty_or_error(e),
        };
        Ok(lookup
            .answers()
            .iter()
            .filter_map(|record| match &record.data {
                RData::SRV(srv) => Some(SrvRecord {
                    priority: srv.priority,
                    weight: srv.weight,
                    port: srv.port,
                    target: host_name(&srv.target),
                }),
                _ => None,
            })
            .collect())
    }
}

impl Resolve for HickoryResolver {
    fn mx<'a>(&'a self, domain: &'a str) -> BoxFuture<'a, Result<Vec<MxRecord>, DnsError>> {
        Box::pin(self.lookup_mx(domain))
    }

    fn srv<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Vec<SrvRecord>, DnsError>> {
        Box::pin(self.lookup_srv(name))
    }
}

/// The name with a trailing dot, so the system's search domains are not
/// tried.
fn absolute(name: &str) -> String {
    if name.ends_with('.') {
        name.to_owned()
    } else {
        format!("{name}.")
    }
}

/// A record's host name, as [`MxRecord`] and [`SrvRecord`] keep it.
fn host_name(name: &Name) -> String {
    if name.is_root() {
        return ".".to_owned();
    }
    let text = name.to_ascii().to_ascii_lowercase();
    text.strip_suffix('.').unwrap_or(&text).to_owned()
}

fn empty_or_error<T>(e: NetError) -> Result<Vec<T>, DnsError> {
    if e.is_no_records_found() || e.is_nx_domain() {
        Ok(Vec::new())
    } else {
        Err(DnsError::Lookup(e.to_string()))
    }
}
