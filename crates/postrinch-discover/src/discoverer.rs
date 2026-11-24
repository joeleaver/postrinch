//! The discoverer: the fetcher, resolver and prober discovery uses, where
//! it fetches from, and how long each step may take.

use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

use crate::dns::{DnsError, HickoryResolver, Resolve};
use crate::fetch::{Fetch, FetchError, HttpsFetch};
use crate::probe::{NetProbe, Probe};

/// The URLs autoconfig files are fetched from. Each is a template:
/// `{domain}` becomes the domain (ASCII) and `{address}` the address,
/// percent-encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// The domain's own autoconfig host.
    pub autoconfig: String,
    /// The domain's `.well-known` autoconfig file.
    pub well_known: String,
    /// Thunderbird's ISPDB.
    pub ispdb: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            autoconfig: "https://autoconfig.{domain}/mail/config-v1.1.xml?emailaddress={address}"
                .to_owned(),
            well_known: "https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml".to_owned(),
            ispdb: "https://autoconfig.thunderbird.net/v1.1/{domain}".to_owned(),
        }
    }
}

impl Endpoints {
    /// Fills in a template.
    pub(crate) fn url(template: &str, domain: &str, address: &str) -> String {
        let address: String = url::form_urlencoded::byte_serialize(address.as_bytes()).collect();
        template
            .replace("{domain}", domain)
            .replace("{address}", &address)
    }
}

/// How long each part of discovery may take before it is given up and the
/// next step tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// One HTTPS fetch, redirects included.
    pub http: Duration,
    /// One DNS lookup.
    pub dns: Duration,
    /// One server probe; the probes of the guessing step run at once.
    pub probe: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            http: Duration::from_secs(8),
            dns: Duration::from_secs(5),
            probe: Duration::from_secs(5),
        }
    }
}

/// The real discoverer could not be set up.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SetupError {
    /// The HTTPS client could not be built.
    #[error("could not set up HTTPS: {0}")]
    Https(FetchError),
    /// The system's DNS configuration could not be read.
    #[error(transparent)]
    Dns(DnsError),
    /// The TLS configuration for probing could not be built.
    #[error("could not set up TLS: {0}")]
    Tls(String),
}

/// What discovery uses to look things up. Cheap to clone.
#[derive(Clone)]
pub struct Discoverer {
    pub(crate) fetch: Arc<dyn Fetch>,
    pub(crate) resolver: Arc<dyn Resolve>,
    pub(crate) prober: Arc<dyn Probe>,
    pub(crate) endpoints: Endpoints,
    pub(crate) timeouts: Timeouts,
}

impl std::fmt::Debug for Discoverer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Discoverer")
            .field("endpoints", &self.endpoints)
            .field("timeouts", &self.timeouts)
            .finish_non_exhaustive()
    }
}

impl Discoverer {
    /// The real discoverer: HTTPS, the system's DNS, network probes, the
    /// default endpoints and timeouts.
    pub fn new() -> Result<Self, SetupError> {
        let timeouts = Timeouts::default();
        let fetch = HttpsFetch::new(timeouts.http).map_err(SetupError::Https)?;
        let resolver = HickoryResolver::from_system().map_err(SetupError::Dns)?;
        let prober = NetProbe::new().map_err(|e| SetupError::Tls(e.to_string()))?;
        Ok(Self::with_parts(
            Arc::new(fetch),
            Arc::new(resolver),
            Arc::new(prober),
        ))
    }

    /// A discoverer from its parts, with the default endpoints and
    /// timeouts.
    pub fn with_parts(
        fetch: Arc<dyn Fetch>,
        resolver: Arc<dyn Resolve>,
        prober: Arc<dyn Probe>,
    ) -> Self {
        Discoverer {
            fetch,
            resolver,
            prober,
            endpoints: Endpoints::default(),
            timeouts: Timeouts::default(),
        }
    }

    /// Fetches autoconfig files from `endpoints` instead.
    pub fn with_endpoints(mut self, endpoints: Endpoints) -> Self {
        self.endpoints = endpoints;
        self
    }

    /// Uses `timeouts` instead. The fetcher's own timeout is separate: set
    /// it when making the fetcher.
    pub fn with_timeouts(mut self, timeouts: Timeouts) -> Self {
        self.timeouts = timeouts;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_templates() {
        let e = Endpoints::default();
        assert_eq!(
            Endpoints::url(&e.autoconfig, "example.org", "ada+x@example.org"),
            "https://autoconfig.example.org/mail/config-v1.1.xml?emailaddress=ada%2Bx%40example.org"
        );
        assert_eq!(
            Endpoints::url(&e.ispdb, "example.org", "ada@example.org"),
            "https://autoconfig.thunderbird.net/v1.1/example.org"
        );
    }
}
