//! A provider's REST API (§3.2: everything under `https://{provider}/api`).
//!
//! [`Provider`] makes the calls that need no signature; [`Client`] signs
//! (§4.4). Both come from a [`Transport`](crate::Transport), which keeps
//! each provider's clock, so a client signs with the provider's time.
//!
//! Every non-success answer is an [`Error::Problem`](crate::Error::Problem),
//! decoded from problem details (§2.6) and acted on by its effective code.
//! A `503` is retried with a fresh signature (§4.5); a `401` whose `Date`
//! shows this machine's clock is off is signed once more on the provider's
//! time. Redirects are never followed, but by a media fetch
//! ([`Transport::fetch_media`](crate::Transport::fetch_media)), which is
//! not an OFSCP endpoint.
//!
//! Not covered, on purpose: calls (§9, a later wave), web push and webhook
//! endpoints (§10, for browsers and servers), guest accounts (§4.8) and
//! pre-signed uploads (§5.8, optional; `POST /api/media` is the baseline).

mod auth;
mod directory;
mod dms;
pub(crate) mod fetch;
mod groups;
mod invites;
mod me;
mod media;
mod membership;
mod messages;
mod multipart;
mod notifications;
mod path;
mod response;
mod send;
mod social;
mod watch;

use std::sync::Arc;

use reqwest::Method;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use fetch::FetchedMedia;
pub use media::MediaUpload;
pub use membership::JoinOutcome;
pub use messages::Posted;
pub use multipart::{Progress, ProgressFn, Source};
pub use path::{Direction, PageRequest, Query, path};

use self::response::Reply;
use self::send::{Auth, Call};
use crate::{Endpoint, Identity, ProviderClock, Result, Transport};

/// A provider, for the calls that need no signature.
#[derive(Debug, Clone)]
pub struct Provider {
    transport: Transport,
    endpoint: Endpoint,
    clock: ProviderClock,
}

impl Provider {
    pub(crate) fn new(transport: Transport, endpoint: Endpoint, clock: ProviderClock) -> Self {
        Self {
            transport,
            endpoint,
            clock,
        }
    }

    /// Where it is.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Its clock, as seen from here.
    pub fn clock(&self) -> &ProviderClock {
        &self.clock
    }

    pub(crate) fn transport(&self) -> &Transport {
        &self.transport
    }

    /// `GET` with no signature: an anonymous viewer (§4.5), who sees only
    /// what is public.
    pub async fn get_anonymous<T: DeserializeOwned>(
        &self,
        path: String,
        query: Query,
    ) -> Result<T> {
        let call = Call::new(Method::GET, path).query(query);
        self.execute(Auth::Anonymous, call).await?.json()
    }
}

/// An identity's signed client for one provider (§4.4): its home, or any
/// other it talks to directly (§8). Cheap to clone.
#[derive(Debug, Clone)]
pub struct Client {
    provider: Provider,
    identity: Arc<Identity>,
}

impl Client {
    pub(crate) fn new(provider: Provider, identity: Arc<Identity>) -> Self {
        Self { provider, identity }
    }

    /// The provider.
    pub fn provider(&self) -> &Provider {
        &self.provider
    }

    /// Who signs.
    pub fn identity(&self) -> &Arc<Identity> {
        &self.identity
    }

    /// Whether this is the signer's home provider.
    pub fn is_home(&self) -> bool {
        self.identity.actor().domain() == self.provider.endpoint().authority()
    }

    async fn call(&self, call: Call<'_>) -> Result<Reply> {
        self.provider
            .execute(Auth::Signed(&self.identity), call)
            .await
    }

    /// A signed `GET`.
    pub async fn get<T: DeserializeOwned>(&self, path: String, query: Query) -> Result<T> {
        self.call(Call::new(Method::GET, path).query(query))
            .await?
            .json()
    }

    /// A signed request with a JSON body and a JSON answer.
    pub async fn send_json<B, T>(&self, method: Method, path: String, body: &B) -> Result<T>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        self.call(Call::new(method, path).json(body)?).await?.json()
    }

    /// A signed request with a JSON body and a JSON answer, with the
    /// answer's status, for endpoints whose status means something:
    /// `201` created or `200` already there (§5.7, §7.1).
    pub async fn send_json_status<B, T>(
        &self,
        method: Method,
        path: String,
        body: &B,
    ) -> Result<(u16, T)>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let reply = self.call(Call::new(method, path).json(body)?).await?;
        Ok((reply.status, reply.json()?))
    }

    /// A signed request with no body and a JSON answer.
    pub async fn send_empty<T: DeserializeOwned>(&self, method: Method, path: String) -> Result<T> {
        self.call(Call::new(method, path)).await?.json()
    }

    /// A signed request answered `204 No Content`, such as a `DELETE`.
    pub async fn send_no_content(&self, method: Method, path: String, query: Query) -> Result<()> {
        self.call(Call::new(method, path).query(query))
            .await?
            .expect(204, "204 No Content")?;
        Ok(())
    }

    /// A signed request with a JSON body, answered `204 No Content`.
    pub async fn send_json_no_content<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: String,
        body: &B,
    ) -> Result<()> {
        self.call(Call::new(method, path).json(body)?)
            .await?
            .expect(204, "204 No Content")?;
        Ok(())
    }
}
