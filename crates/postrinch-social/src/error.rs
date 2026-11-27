//! What can go wrong with a social account.

use postrinch_ofscp::ofscp_core::IdentError;
use postrinch_store::{SocialAccountState, StoreError};
use postrinch_sync_core::KeychainError;

/// A result of the social engine.
pub type SocialResult<T, E = SocialError> = std::result::Result<T, E>;

/// Why a social account could not be added, signed in, synced or removed.
#[derive(Debug, thiserror::Error)]
pub enum SocialError {
    /// What the person typed is not an actor (`handle@domain`, §2.2).
    #[error("{input:?} is not an OFSCP address (handle@domain): {source}")]
    NotAnActor {
        input: String,
        #[source]
        source: IdentError,
    },
    /// The account is already on this computer.
    #[error("{0} is already added")]
    AlreadyAdded(String),
    /// No such account on this computer.
    #[error("no account {0}")]
    NoAccount(String),
    /// The provider refused the handle and password (§4.1.2).
    #[error("{actor}: the handle or password is wrong")]
    WrongPassword { actor: String },
    /// The account cannot talk to its provider until the person acts:
    /// signs in again, or sets this computer's clock.
    #[error("{actor} {}", unusable(*.state))]
    Unusable {
        actor: String,
        state: SocialAccountState,
    },
    /// A provider refused or failed a request.
    #[error(transparent)]
    Transport(#[from] postrinch_ofscp::Error),
    /// A command on a provider's socket got no answer, or was refused.
    #[error("on the socket: {0}")]
    Socket(#[from] postrinch_ofscp::ws::RequestError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Keychain(#[from] KeychainError),
    /// What the keychain holds for an account does not read as a device
    /// key.
    #[error("the device key kept for {actor} is damaged: {reason}")]
    DamagedKey { actor: String, reason: String },
    /// Media larger than the whole media cache: it is not fetched.
    #[error("{url} is {size} bytes, more than the media cache's {limit}")]
    MediaTooLarge { url: String, size: u64, limit: u64 },
    /// Fetched media whose bytes do not match the hash it claims: it is
    /// not kept.
    #[error("{url} does not match its hash")]
    MediaDamaged { url: String },
    /// What the person gave is not an invite link (§5.6).
    #[error("{0:?} is not an invite link (https://provider/invite/…)")]
    NotAnInviteLink(String),
    /// The provider knows no such invite, or it has expired (§5.6 `404`).
    #[error("this invite is not valid: it may have expired or been revoked")]
    InviteNotValid,
    /// Every use of the invite is spent (§5.6 `409`).
    #[error("this invite has been used up")]
    InviteUsedUp,
    /// The group takes members only by invite (§5.7 `403`).
    #[error("{group} takes members by invite only")]
    InviteOnly { group: String },
    /// Not the URI of a group (§2.2 `https://{provider}/api/groups/{id}`).
    #[error("{0:?} is not a group")]
    NotAGroup(String),
    /// Not the URI of a channel (§2.2), or not of one in the group meant.
    #[error("{0:?} is not a channel")]
    NotAChannel(String),
    /// An invite or request id that is not one (§2.2 local ids).
    #[error("{0:?} is not an id")]
    NotAnId(String),
    /// A time, in milliseconds since the epoch, out of range.
    #[error("{0} is not a time")]
    NotATime(i64),
    /// A blocking task (the keychain, the store) did not finish.
    #[error("a background task failed: {0}")]
    Task(String),
}

/// How an account that cannot be used reads.
fn unusable(state: SocialAccountState) -> &'static str {
    match state {
        SocialAccountState::NeedsSignIn => "must sign in again",
        SocialAccountState::ClockSkewed => {
            "cannot sign requests: this computer's clock is too far from its provider's; set the clock right"
        }
        SocialAccountState::Active => "is signed in",
    }
}

impl From<tokio::task::JoinError> for SocialError {
    fn from(e: tokio::task::JoinError) -> Self {
        Self::Task(e.to_string())
    }
}
