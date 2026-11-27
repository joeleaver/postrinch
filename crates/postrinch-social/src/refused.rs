//! What a provider's refusal means for the account that was refused.

use postrinch_ofscp::http::Client;
use postrinch_ofscp::ofscp_core::wire::ErrorCode;
use postrinch_ofscp::{Error, Identity, SKEW_WINDOW_SECS};
use postrinch_store::SocialAccountState;

use crate::{Social, SocialError, SocialResult};

impl Social {
    /// A signed request's answer, made by `identity` through `client`. A
    /// signature the home provider refuses marks the account with the
    /// state [`account_state`] gives: home is the key's only judge (§4.6).
    /// Another provider's refusal is only an error.
    pub(crate) async fn checked<T>(
        &self,
        identity: &Identity,
        client: &Client,
        result: postrinch_ofscp::Result<T>,
    ) -> SocialResult<T> {
        let error = match result {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        let is_home = client.provider().endpoint().authority() == identity.actor().domain();
        match account_state(client, &error) {
            Some(state) if is_home => {
                let actor = identity.actor();
                tracing::warn!(%actor, ?state, %error, "signature refused");
                let key_id = Some(identity.key_id().to_owned());
                let otherwise = SocialError::Transport(error);
                Err(self.mark(actor, key_id, state, otherwise).await?)
            }
            _ => Err(SocialError::Transport(error)),
        }
    }
}

/// The state a signed request's failure puts its account in, if any.
///
/// A `401` on a signed request means the signature was refused (§4.5).
/// The transport has already signed again on the provider's clock where
/// its `Date` showed the clock was to blame, so what is left is the key:
/// revoked, or unknown to the provider. The person signs in again
/// ([`SocialAccountState::NeedsSignIn`]). But when this computer's clock
/// is itself further from the provider's than signatures allow, every
/// signature rests on the offset learned from the provider; a refusal then
/// is put down to the clock ([`SocialAccountState::ClockSkewed`]), which
/// the person sets right, as a new key would be refused all the same.
pub(crate) fn account_state(client: &Client, error: &Error) -> Option<SocialAccountState> {
    if !error
        .problem()
        .is_some_and(|p| p.is(&ErrorCode::Unauthorized))
    {
        return None;
    }
    let offset = client.provider().clock().offset_secs();
    Some(if offset.abs() >= SKEW_WINDOW_SECS {
        SocialAccountState::ClockSkewed
    } else {
        SocialAccountState::NeedsSignIn
    })
}

/// Whether `error` is a `403`: the caller may not see it.
pub(crate) fn is_forbidden(error: &Error) -> bool {
    error.problem().is_some_and(|p| p.is(&ErrorCode::Forbidden))
}

/// Whether `error` says the thing asked for is not there for this
/// account: a `404`, or a `403` (§11: what the caller may not read).
/// Such a group or channel is left out of a sync, not a reason to stop it.
pub(crate) fn is_out_of_reach(error: &Error) -> bool {
    error
        .problem()
        .is_some_and(|p| p.is(&ErrorCode::NotFound) || p.is(&ErrorCode::Forbidden))
}

/// Whether `error`, the answer to revoking a key through `client`, says
/// the key is gone already: a `404`, or a `401` put down to the key
/// (revoked, so it can no longer sign its own revocation). A `401` put
/// down to the clock is not: the key may well still be registered.
pub(crate) fn key_already_gone(client: &Client, error: &Error) -> bool {
    error.problem().is_some_and(|p| p.is(&ErrorCode::NotFound))
        || account_state(client, error) == Some(SocialAccountState::NeedsSignIn)
}
