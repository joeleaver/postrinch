//! The one transport an application keeps: the HTTPS connection pool, the
//! trust settings, the limits, and each provider's clock.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::http::{Client, Provider};
use crate::{Endpoint, Error, Identity, ProviderClock, Result, Tls};

/// Limits and timeouts. The defaults suit a desktop client.
#[derive(Debug, Clone)]
pub struct Options {
    /// How long to wait for a TCP and TLS connection.
    pub connect_timeout: Duration,
    /// How long a whole request may take, uploads excepted.
    pub request_timeout: Duration,
    /// How long a connection may sit idle in the middle of a response, or
    /// of an upload.
    pub read_timeout: Duration,
    /// The largest response body read (uploads go the other way).
    pub max_response_bytes: usize,
    /// Retrying a `503` (§4.5, §7.3).
    pub retry: RetryPolicy,
    /// The `User-Agent`.
    pub user_agent: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            read_timeout: Duration::from_secs(30),
            max_response_bytes: 16 * 1024 * 1024,
            retry: RetryPolicy::default(),
            user_agent: concat!("postrinch-ofscp/", env!("CARGO_PKG_VERSION")).to_owned(),
        }
    }
}

/// How a `503 service_unavailable` is retried. The provider may be warming
/// its replay store after a restart, or unable to fetch the signer's keys
/// yet (§4.5, §4.6); each retry is signed afresh, as the spec requires.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Retries after the first attempt.
    pub max_retries: u32,
    /// The longest wait for one retry. A `Retry-After` longer than this
    /// ends the retries, and the caller gets the `503` with its
    /// `retry_after`, to try again later its own way.
    pub max_wait: Duration,
    /// The wait before the first retry when the provider gives no
    /// `Retry-After`; it doubles each time.
    pub first_backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            max_wait: Duration::from_secs(30),
            first_backoff: Duration::from_secs(1),
        }
    }
}

impl RetryPolicy {
    /// The wait before retry number `retry` (0 for the first), given the
    /// provider's `Retry-After`; `None` when there is to be no retry.
    pub(crate) fn wait(&self, retry: u32, retry_after: Option<Duration>) -> Option<Duration> {
        if retry >= self.max_retries {
            return None;
        }
        let wait = retry_after.unwrap_or_else(|| {
            self.first_backoff
                .saturating_mul(2u32.saturating_pow(retry))
        });
        (wait <= self.max_wait).then_some(wait)
    }
}

/// The transport: cheap to clone, shared by every provider and identity.
#[derive(Clone, Debug)]
pub struct Transport {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    http: reqwest::Client,
    tls: Tls,
    options: Options,
    clocks: Mutex<HashMap<ofscp_core::Authority, ProviderClock>>,
}

impl Transport {
    /// A transport trusting `tls`, with `options`.
    pub fn new(tls: Tls, options: Options) -> Result<Self> {
        let http = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(options.connect_timeout)
            .read_timeout(options.read_timeout)
            .user_agent(options.user_agent.clone())
            .tls_certs_merge(
                tls.extra_roots()
                    .iter()
                    .map(|der| reqwest::Certificate::from_der(der))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| Error::Tls(crate::TlsError::new(e)))?,
            )
            .build()
            .map_err(|e| Error::Tls(crate::TlsError::new(e)))?;
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                tls,
                options,
                clocks: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// The provider at `endpoint`, for what needs no signature.
    pub fn provider(&self, endpoint: Endpoint) -> Provider {
        let clock = self.clock(&endpoint);
        Provider::new(self.clone(), endpoint, clock)
    }

    /// `identity`'s signed client for the provider at `endpoint`: its home
    /// provider, or any other it talks to (§8).
    pub fn client(&self, identity: Arc<Identity>, endpoint: Endpoint) -> Client {
        Client::new(self.provider(endpoint), identity)
    }

    /// The clock kept for a provider, shared by everything that talks to it.
    pub fn clock(&self, endpoint: &Endpoint) -> ProviderClock {
        let mut clocks = self
            .inner
            .clocks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        clocks
            .entry(endpoint.authority().clone())
            .or_default()
            .clone()
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.inner.http
    }

    pub(crate) fn tls(&self) -> &Tls {
        &self.inner.tls
    }

    /// The limits and timeouts.
    pub fn options(&self) -> &Options {
        &self.inner.options
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_back_off_and_stop() {
        let p = RetryPolicy::default();
        assert_eq!(p.wait(0, None), Some(Duration::from_secs(1)));
        assert_eq!(p.wait(1, None), Some(Duration::from_secs(2)));
        assert_eq!(p.wait(2, None), Some(Duration::from_secs(4)));
        assert_eq!(p.wait(3, None), None);
    }

    #[test]
    fn retry_after_is_honoured_up_to_the_limit() {
        let p = RetryPolicy::default();
        assert_eq!(
            p.wait(0, Some(Duration::from_secs(7))),
            Some(Duration::from_secs(7))
        );
        assert_eq!(p.wait(0, Some(Duration::from_secs(300))), None);
    }

    #[test]
    fn a_provider_keeps_its_clock() {
        let transport = Transport::new(Tls::platform().unwrap(), Options::default()).unwrap();
        let endpoint = Endpoint::new(ofscp_core::Authority::canonicalize("a.example").unwrap());
        let date = httpdate::fmt_http_date(std::time::SystemTime::now() + Duration::from_secs(900));
        transport
            .provider(endpoint.clone())
            .clock()
            .observe_date(&date);
        assert!(transport.provider(endpoint).clock().offset_secs() > 800);
        let other = Endpoint::new(ofscp_core::Authority::canonicalize("b.example").unwrap());
        assert_eq!(transport.provider(other).clock().offset_secs(), 0);
    }
}
