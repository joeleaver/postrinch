//! Where a provider is: its canonical authority, and the URLs built from it.

use ofscp_core::{Actor, Authority};
use url::Url;

/// The port of `https:` and `wss:`, left out of a canonical authority.
const DEFAULT_PORT: u16 = 443;

/// One provider, named by its canonical authority (§2.2). Its REST API is
/// at `https://{authority}/api/...` (§3.2) and its socket at
/// `wss://{authority}/api/ws` (§7.1). There is nothing else to configure:
/// the authority is also what requests to it are signed for (§4.4.2).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Endpoint {
    authority: Authority,
}

/// A URL could not be made for a provider.
#[derive(Debug, thiserror::Error)]
#[error("no URL for {authority}{path}: {source}")]
pub struct EndpointError {
    authority: String,
    path: String,
    #[source]
    source: url::ParseError,
}

impl Endpoint {
    /// The provider at `authority`.
    pub fn new(authority: Authority) -> Self {
        Self { authority }
    }

    /// An actor's home provider: the part of the handle after the `@`.
    pub fn home_of(actor: &Actor) -> Self {
        Self::new(actor.domain().clone())
    }

    /// The canonical authority.
    pub fn authority(&self) -> &Authority {
        &self.authority
    }

    /// The host, without the port.
    pub fn host(&self) -> &str {
        self.split().0
    }

    /// The TCP port: the one in the authority, or 443.
    pub fn port(&self) -> u16 {
        self.split().1
    }

    fn split(&self) -> (&str, u16) {
        let s = self.authority.as_str();
        match s.rsplit_once(':') {
            // A canonical authority's port is always valid digits.
            Some((host, port)) => (host, port.parse().unwrap_or(DEFAULT_PORT)),
            None => (s, DEFAULT_PORT),
        }
    }

    /// `https://{authority}{path}`. `path` starts with `/` and is already
    /// percent-encoded where it needs to be (see [`crate::http::path`]).
    pub fn https(&self, path: &str) -> Result<Url, EndpointError> {
        self.url("https", path)
    }

    /// `wss://{authority}/api/ws` (§7.1).
    pub fn ws(&self) -> Result<Url, EndpointError> {
        self.url("wss", "/api/ws")
    }

    fn url(&self, scheme: &str, path: &str) -> Result<Url, EndpointError> {
        Url::parse(&format!("{scheme}://{}{path}", self.authority)).map_err(|source| {
            EndpointError {
                authority: self.authority.to_string(),
                path: path.to_owned(),
                source,
            }
        })
    }
}

impl From<Authority> for Endpoint {
    fn from(authority: Authority) -> Self {
        Self::new(authority)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(s: &str) -> Endpoint {
        Endpoint::new(Authority::canonicalize(s).unwrap())
    }

    #[test]
    fn host_and_port_come_from_the_authority() {
        let e = endpoint("Social.Example");
        assert_eq!((e.host(), e.port()), ("social.example", 443));
        let e = endpoint("127.0.0.1:8443");
        assert_eq!((e.host(), e.port()), ("127.0.0.1", 8443));
    }

    #[test]
    fn urls_keep_the_port_and_the_path_as_given() {
        let e = endpoint("a.example:8443");
        assert_eq!(
            e.https("/api/users/alice%40b.example/profile")
                .unwrap()
                .as_str(),
            "https://a.example:8443/api/users/alice%40b.example/profile"
        );
        assert_eq!(e.ws().unwrap().as_str(), "wss://a.example:8443/api/ws");
        assert_eq!(
            endpoint("a.example").https("/api/me").unwrap().as_str(),
            "https://a.example/api/me"
        );
    }

    #[test]
    fn an_actors_home_is_its_domain() {
        let actor = Actor::canonicalize("alice@A.example").unwrap();
        assert_eq!(Endpoint::home_of(&actor), endpoint("a.example"));
    }
}
