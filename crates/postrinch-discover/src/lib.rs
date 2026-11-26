//! Mail server discovery: from an email address to its IMAP and SMTP
//! servers, for adding an account by hand (`docs/imap.md`, "Adding an
//! account").
//!
//! [`discover`] runs these steps in order, and the first that answers wins:
//!
//! 1. **The domain's own settings**: `https://autoconfig.<domain>/mail/config-v1.1.xml`,
//!    then `https://<domain>/.well-known/autoconfig/mail/config-v1.1.xml`.
//! 2. **Thunderbird's list of providers** (the ISPDB):
//!    `https://autoconfig.thunderbird.net/v1.1/<domain>`.
//! 3. **The domain's DNS records**: its MX host, which marks mail hosted by
//!    Google or Microsoft (and answers with their ISPDB entries) or is
//!    looked up in the ISPDB by its registrable domain; then the SRV
//!    records `_imaps._tcp`, `_imap._tcp`, `_submissions._tcp` and
//!    `_submission._tcp` (RFC 6186, RFC 8314).
//! 4. **Common server names**: `imap.`/`mail.` on 993 (TLS) and 143
//!    (STARTTLS), `smtp.`/`mail.` on 465 (TLS) and 587 (STARTTLS), each
//!    probed at once, offered only if it answers.
//!
//! Everything is fetched over HTTPS. Plain-text servers are never offered.
//! Every lookup has a timeout, and one that fails moves on to the next.
//! The fetcher, the resolver and the prober are traits ([`Fetch`],
//! [`Resolve`], [`Probe`]) so tests can stand in for the network.

mod address;
mod discoverer;
mod dns;
mod fetch;
mod probe;
mod providers;
mod steps;
mod types;
mod xml;

use std::future::Future;
use std::pin::Pin;

pub use address::AddressError;
pub use discoverer::{Discoverer, Endpoints, SetupError, Timeouts};
pub use dns::{DnsError, HickoryResolver, MxRecord, Resolve, SrvRecord};
pub use fetch::{Fetch, FetchError, HttpsFetch, MAX_BODY, MAX_REDIRECTS};
pub use probe::{NetProbe, Probe, ProbeError, Protocol};
pub use providers::{KnownHost, known_host};
pub use types::{
    Candidate, Discovery, HostedBy, Progress, Security, Source, Step, StepState, Username,
};

/// A boxed, sendable future, as the [`Fetch`], [`Resolve`] and [`Probe`]
/// traits return.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Finds the IMAP and SMTP servers for `address`, reporting each step to
/// `progress` as it starts and ends.
///
/// Only an address that is not one fails; finding nothing answers
/// [`Source::Nothing`]. Mail hosted by Google or Microsoft has
/// [`Discovery::hosted_by`] set whichever step answered.
pub async fn discover(
    address: &str,
    discoverer: &Discoverer,
    mut progress: impl FnMut(Progress) + Send,
) -> Result<Discovery, AddressError> {
    let address = address::Address::parse(address)?;
    let mut report = |step, state| progress(Progress { step, state });
    let d = discoverer;

    report(Step::DomainSettings, StepState::Running);
    if let Some(found) = steps::domain_autoconfig(d, &address).await {
        report(Step::DomainSettings, StepState::Found);
        return Ok(found);
    }
    report(Step::DomainSettings, StepState::NothingFound);

    report(Step::ProviderList, StepState::Running);
    if let Some(found) = steps::ispdb(d, &address, &address.domain, Source::Ispdb).await {
        report(Step::ProviderList, StepState::Found);
        return Ok(found);
    }
    report(Step::ProviderList, StepState::NothingFound);

    report(Step::DnsRecords, StepState::Running);
    let mx = steps::mx(d, &address).await;
    let hosted_by = mx.hosted_by;
    if let Some(found) = mx.found {
        report(Step::DnsRecords, StepState::Found);
        return Ok(found);
    }
    if let Some(found) = steps::srv(d, &address).await {
        report(Step::DnsRecords, StepState::Found);
        return Ok(Discovery {
            hosted_by: found.hosted_by.or(hosted_by),
            ..found
        });
    }
    report(Step::DnsRecords, StepState::NothingFound);

    report(Step::CommonNames, StepState::Running);
    if let Some(found) = steps::common_names(d, &address).await {
        report(Step::CommonNames, StepState::Found);
        return Ok(Discovery { hosted_by, ..found });
    }
    report(Step::CommonNames, StepState::NothingFound);
    Ok(Discovery {
        hosted_by,
        ..Discovery::nothing()
    })
}
