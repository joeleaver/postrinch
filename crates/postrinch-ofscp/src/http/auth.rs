//! Discovery (§3.1), accounts and device keys (§4.1–§4.7).

use ofscp_core::Actor;
use ofscp_core::signing::DeviceKey;
use ofscp_core::wire::{
    BootstrapResponse, DeviceKeyCreated, DeviceKeyRegistration, DeviceKeysResponse,
    DiscoveryDocument, KeyAlgorithm, LoginRequest, RegistrationRequest, UserKeysResponse,
};
use rand_core::OsRng;
use reqwest::Method;

use super::path::{Query, path};
use super::send::{Auth, Call};
use super::{Client, Provider};
use crate::{Error, Identity, Result};

impl Provider {
    /// The provider's discovery document (§3.1), checked to be its own:
    /// `provider.domain` must be this provider's canonical authority.
    pub async fn discover(&self) -> Result<DiscoveryDocument> {
        let call = Call::new(Method::GET, "/.well-known/ofscp-provider".to_owned());
        let doc: DiscoveryDocument = self.execute(Auth::Anonymous, call).await?.json()?;
        if &doc.provider.domain != self.endpoint().authority() {
            return Err(Error::WrongDomain {
                authority: self.endpoint().authority().to_string(),
                found: doc.provider.domain.to_string(),
            });
        }
        Ok(doc)
    }

    /// Create an account (§4.1.1). The answer's bootstrap token registers
    /// a device key ([`Provider::register_device`]) within its lifetime.
    pub async fn register(&self, request: &RegistrationRequest) -> Result<BootstrapResponse> {
        let call = Call::new(Method::POST, "/api/auth/register".to_owned()).json(request)?;
        self.execute(Auth::Anonymous, call).await?.json()
    }

    /// Log in with the password (§4.1.2). A wrong handle or password is a
    /// `401`, the same for both.
    pub async fn login(&self, handle: &str, password: &str) -> Result<BootstrapResponse> {
        let request = LoginRequest {
            handle: handle.to_owned(),
            password: password.to_owned(),
        };
        let call = Call::new(Method::POST, "/api/auth/login".to_owned()).json(&request)?;
        self.execute(Auth::Anonymous, call).await?.json()
    }

    /// Register this device's public key with a bootstrap token (§4.3).
    /// The key belongs to the token's handle; the client cannot choose.
    pub async fn register_device_key(
        &self,
        bootstrap_token: &str,
        key: &DeviceKey,
        device_name: &str,
    ) -> Result<DeviceKeyCreated> {
        let request = DeviceKeyRegistration {
            public_key: key.public_key_base64(),
            algorithm: Some(KeyAlgorithm::Ed25519),
            device_name: device_name.to_owned(),
        };
        let call = Call::new(Method::POST, "/api/auth/device-keys".to_owned()).json(&request)?;
        self.execute(Auth::Bearer(bootstrap_token), call)
            .await?
            .json()
    }

    /// Make a key for this device, register it with `bootstrap`, and give
    /// back the identity it signs as: `handle` at this provider.
    pub async fn register_device(
        &self,
        handle: &str,
        bootstrap: &BootstrapResponse,
        device_name: &str,
    ) -> Result<Identity> {
        let actor = Actor::new(handle, self.endpoint().authority().clone())?;
        let key = DeviceKey::generate(&mut OsRng);
        let created = self
            .register_device_key(&bootstrap.bootstrap_token, &key, device_name)
            .await?;
        Ok(Identity::new(actor, created.key_id, key))
    }

    /// Sign in on this device: log in, then register a new device key
    /// (§4.1.2, §4.3). The caller keeps the key's seed.
    pub async fn sign_in(
        &self,
        handle: &str,
        password: &str,
        device_name: &str,
    ) -> Result<Identity> {
        let bootstrap = self.login(handle, password).await?;
        self.register_device(handle, &bootstrap, device_name).await
    }

    /// A user's published device keys (§4.6), unsigned. Unknown handles
    /// answer an empty list.
    pub async fn user_keys(&self, handle: &str) -> Result<UserKeysResponse> {
        let call = Call::new(
            Method::GET,
            path("/.well-known/ofscp/users/{}/keys", &[handle]),
        );
        self.execute(Auth::Anonymous, call).await?.json()
    }
}

impl Client {
    /// The signer's device keys (§4.7), on its home provider.
    pub async fn device_keys(&self) -> Result<DeviceKeysResponse> {
        self.require_home()?;
        self.get("/api/auth/device-keys".to_owned(), Query::new())
            .await
    }

    /// Revoke one of the signer's device keys (§4.7.1). Its sockets close
    /// with `4003`. An unknown key is a `404`.
    pub async fn revoke_device_key(&self, key_id: &str) -> Result<()> {
        self.require_home()?;
        self.send_no_content(
            Method::DELETE,
            path("/api/auth/device-keys/{}", &[key_id]),
            Query::new(),
        )
        .await
    }

    /// Sign this device out (§4.7.2): revoke its own key, signed by that
    /// key. The caller then deletes the private key.
    pub async fn sign_out(&self) -> Result<()> {
        let key_id = self.identity().key_id().to_owned();
        self.revoke_device_key(&key_id).await
    }

    /// Key management is home-only (§4.7).
    pub(crate) fn require_home(&self) -> Result<()> {
        if self.is_home() {
            Ok(())
        } else {
            Err(Error::NotHome {
                authority: self.provider().endpoint().authority().to_string(),
                home: self.identity().actor().domain().to_string(),
            })
        }
    }
}
