//! What a provider offers beyond groups: its tiers (§11.1), its discovery
//! feed (§11.2) and the providers it knows (§8.6).
//!
//! §4.5 lists none of these among the routes that take anonymous readers,
//! so they are signed.

use ofscp_core::wire::{DiscoverResponse, ProvidersResponse, TiersResponse};

use super::Client;
use super::path::{PageRequest, Query};
use crate::Result;

impl Client {
    /// The tiers this provider offers, described for people (§11.1).
    pub async fn tiers(&self) -> Result<TiersResponse> {
        self.get("/api/tiers".to_owned(), Query::new()).await
    }

    /// One page of this provider's discovery feed (§11.2): pointers to
    /// discoverable channels, read live from their hosts. Optional for
    /// providers (`404` if not offered; see
    /// `capabilities.discovery.discoverFeed`).
    pub async fn discover(&self, page: &PageRequest) -> Result<DiscoverResponse> {
        self.get("/api/discover".to_owned(), page.query()).await
    }

    /// The providers this one knows (§8.6). `404` if it keeps no list,
    /// `403` if it declines to share it.
    pub async fn known_providers(&self) -> Result<ProvidersResponse> {
        self.get("/api/providers".to_owned(), Query::new()).await
    }
}
