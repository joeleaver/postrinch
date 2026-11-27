//! Why a connection ended, and what that means for the next one (§7.1
//! "Close codes").

use ofscp_core::ws::{CLOSE_AUTH_FAILED, CLOSE_REVOKED};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

/// How a connection ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    /// `4003`: the device key was revoked, or the account is gone. The
    /// client must never reconnect with this key; the device is signed
    /// out and signs in again (§4.7, §7.1).
    Revoked,
    /// `4001`: authentication failed, federation policy refused the actor,
    /// or the provider could not re-confirm the key in time. The key was
    /// not revoked: keep it, and reconnect later (§7.1).
    AuthFailed,
    /// Another close code from the provider.
    Server {
        /// The code.
        code: u16,
        /// The reason the provider gave, if any.
        reason: String,
    },
    /// This side closed it.
    Client,
    /// The connection broke without a close frame.
    Dropped(String),
    /// The provider fell silent past the idle timeout (§7.1 "Heartbeat").
    TimedOut,
}

impl CloseReason {
    /// Whether to reconnect with the same key, after a pause. Only `4003`
    /// rules it out; a `4001` is retried later (§7.1).
    pub fn may_reconnect(&self) -> bool {
        !matches!(self, Self::Revoked)
    }

    pub(crate) fn from_frame(frame: Option<&CloseFrame>) -> Self {
        let Some(frame) = frame else {
            return Self::Server {
                code: 1005,
                reason: String::new(),
            };
        };
        match u16::from(frame.code) {
            CLOSE_REVOKED => Self::Revoked,
            CLOSE_AUTH_FAILED => Self::AuthFailed,
            code => Self::Server {
                code,
                reason: frame.reason.as_str().to_owned(),
            },
        }
    }
}

impl std::fmt::Display for CloseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Revoked => f.write_str("the device key was revoked (4003)"),
            Self::AuthFailed => f.write_str("authentication failed (4001)"),
            Self::Server { code, reason } if reason.is_empty() => {
                write!(f, "closed by the provider ({code})")
            }
            Self::Server { code, reason } => write!(f, "closed by the provider ({code}): {reason}"),
            Self::Client => f.write_str("closed by this client"),
            Self::Dropped(why) => write!(f, "the connection dropped: {why}"),
            Self::TimedOut => f.write_str("the provider stopped answering"),
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

    use super::*;

    fn frame(code: u16, reason: &str) -> CloseFrame {
        CloseFrame {
            code: CloseCode::from(code),
            reason: reason.to_owned().into(),
        }
    }

    #[test]
    fn the_ofscp_codes_are_told_apart() {
        assert_eq!(
            CloseReason::from_frame(Some(&frame(4003, ""))),
            CloseReason::Revoked
        );
        assert_eq!(
            CloseReason::from_frame(Some(&frame(4001, "bad"))),
            CloseReason::AuthFailed
        );
        assert_eq!(
            CloseReason::from_frame(Some(&frame(1001, "restarting"))),
            CloseReason::Server {
                code: 1001,
                reason: "restarting".into()
            }
        );
        assert_eq!(
            CloseReason::from_frame(None),
            CloseReason::Server {
                code: 1005,
                reason: String::new()
            }
        );
    }

    #[test]
    fn only_a_revoked_key_stays_away() {
        assert!(!CloseReason::Revoked.may_reconnect());
        assert!(CloseReason::AuthFailed.may_reconnect());
        assert!(CloseReason::TimedOut.may_reconnect());
        assert!(CloseReason::Dropped("reset".into()).may_reconnect());
    }
}
