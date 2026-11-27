//! What can go wrong talking to a provider.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use ofscp_core::signing::CanonicalError;
use ofscp_core::wire::{ErrorCode, ProblemDetails};

use crate::{EndpointError, TlsError};

/// A result of the transport.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Why a request to a provider failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The provider could not be reached, or the connection broke.
    #[error("cannot reach {authority}: {source}")]
    Network {
        /// The provider.
        authority: String,
        /// What reqwest saw.
        #[source]
        source: reqwest::Error,
    },
    /// The provider answered with an error (§2.6, §7.3).
    #[error("{0}")]
    Problem(Box<Problem>),
    /// A success response whose body is not what the spec says.
    #[error("{authority} sent a {expected} that does not decode: {source}")]
    Decode {
        /// The provider.
        authority: String,
        /// What was expected.
        expected: &'static str,
        /// Why it did not decode.
        #[source]
        source: serde_json::Error,
    },
    /// A response larger than this transport reads.
    #[error("{authority} sent a response over {limit} bytes")]
    TooLarge {
        /// The provider.
        authority: String,
        /// The limit.
        limit: usize,
    },
    /// A redirect. OFSCP endpoints are at fixed paths on the provider
    /// itself (§3.2) and a signature is bound to its authority (§4.4.2),
    /// so a redirect is never followed.
    #[error("{authority} redirected ({status}) to {location:?}")]
    Redirect {
        /// The provider.
        authority: String,
        /// The `3xx` status.
        status: u16,
        /// Where it pointed.
        location: Option<String>,
    },
    /// A success status the endpoint does not define.
    #[error("{authority} answered {status} where the spec gives {expected}")]
    UnexpectedStatus {
        /// The provider.
        authority: String,
        /// What it answered.
        status: u16,
        /// What the spec gives.
        expected: &'static str,
    },
    /// A discovery document for another provider (§3.1: `provider.domain`
    /// must be the provider's own canonical authority).
    #[error("{authority}'s discovery document says it is {found}")]
    WrongDomain {
        /// The provider asked.
        authority: String,
        /// The domain the document names.
        found: String,
    },
    /// No URL could be made.
    #[error(transparent)]
    Endpoint(#[from] EndpointError),
    /// A request could not be signed (a component held a line feed).
    #[error("cannot sign the request: {0}")]
    Sign(#[from] CanonicalError),
    /// A request body could not be serialized.
    #[error("cannot encode the request: {0}")]
    Encode(#[source] serde_json::Error),
    /// A file to upload could not be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        source: std::io::Error,
    },
    /// An upload that cannot be sent as given.
    #[error("cannot upload this: {0}")]
    InvalidUpload(&'static str),
    /// A call that only the signer's home provider takes, sent elsewhere:
    /// key management (§4.7), and uploads, which §5.8 sends to the home
    /// provider whatever they are for, naming that provider as `target`.
    #[error("this goes to {home}, the signer's home provider, not {authority}")]
    NotHome {
        /// The provider it was sent to.
        authority: String,
        /// The uploader's home provider.
        home: String,
    },
    /// A handle or identifier that is not valid (§2.2).
    #[error("{0}")]
    Ident(#[from] ofscp_core::IdentError),
    /// The TLS settings could not be made.
    #[error(transparent)]
    Tls(#[from] TlsError),
}

impl Error {
    /// The provider's problem, when it answered with one.
    pub fn problem(&self) -> Option<&Problem> {
        match self {
            Self::Problem(p) => Some(p),
            _ => None,
        }
    }

    /// The effective error code of a provider's answer (see
    /// [`Problem::effective_code`]).
    pub fn code(&self) -> Option<ErrorCode> {
        self.problem().and_then(Problem::effective_code)
    }
}

/// A provider's error answer: a problem details document (§2.6), or as
/// much of one as the response gave.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    /// The provider.
    pub authority: String,
    /// The HTTP status.
    pub status: u16,
    /// The `errorCode`, if the answer had one. A proxy's answer may not.
    pub error_code: Option<ErrorCode>,
    /// The `title`.
    pub title: Option<String>,
    /// The `detail`, for people.
    pub detail: Option<String>,
    /// The `Retry-After`, if any (§4.1.5, §4.5, §7.3).
    pub retry_after: Option<Duration>,
}

impl Problem {
    pub(crate) fn new(authority: &str, status: u16, details: Option<ProblemDetails>) -> Self {
        let (error_code, title, detail) = match details {
            Some(d) => (d.error_code, d.title, d.detail),
            None => (None, None, None),
        };
        Self {
            authority: authority.to_owned(),
            status,
            error_code,
            title,
            detail,
            retry_after: None,
        }
    }

    /// The code to act on (§2.6): the `errorCode` when it is one this
    /// version knows, otherwise the registry's code for the status (§7.3),
    /// as the spec requires when the code is missing or not recognised.
    /// `None` when neither is in the registry.
    pub fn effective_code(&self) -> Option<ErrorCode> {
        match &self.error_code {
            Some(code) if code.is_known() => Some(code.clone()),
            _ => code_for_status(self.status),
        }
    }

    /// Whether the effective code is `code`.
    pub fn is(&self, code: &ErrorCode) -> bool {
        self.effective_code().as_ref() == Some(code)
    }
}

/// The registry's general code for a status (§7.3).
fn code_for_status(status: u16) -> Option<ErrorCode> {
    Some(match status {
        400 => ErrorCode::BadRequest,
        401 => ErrorCode::Unauthorized,
        403 => ErrorCode::Forbidden,
        404 => ErrorCode::NotFound,
        409 => ErrorCode::Conflict,
        413 => ErrorCode::PayloadTooLarge,
        429 => ErrorCode::TooManyRequests,
        500 => ErrorCode::InternalError,
        503 => ErrorCode::ServiceUnavailable,
        _ => return None,
    })
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} answered {}", self.authority, self.status)?;
        if let Some(code) = &self.error_code {
            write!(f, " {code}")?;
        }
        if let Some(text) = self.detail.as_ref().or(self.title.as_ref()) {
            write!(f, ": {text}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(status: u16, code: Option<&str>) -> Problem {
        Problem {
            error_code: code.map(ErrorCode::from),
            ..Problem::new("a.example", status, None)
        }
    }

    #[test]
    fn a_known_code_is_used_as_it_is() {
        let p = problem(403, Some("invite_required"));
        assert_eq!(p.effective_code(), Some(ErrorCode::InviteRequired));
        assert!(p.is(&ErrorCode::InviteRequired));
    }

    #[test]
    fn a_missing_or_unknown_code_falls_back_to_the_status() {
        assert_eq!(
            problem(503, None).effective_code(),
            Some(ErrorCode::ServiceUnavailable)
        );
        assert_eq!(
            problem(404, Some("gone_fishing")).effective_code(),
            Some(ErrorCode::NotFound)
        );
        assert_eq!(problem(418, None).effective_code(), None);
    }

    #[test]
    fn it_reads_well() {
        let mut p = problem(403, Some("forbidden"));
        p.detail = Some("Not permitted to post".into());
        assert_eq!(
            p.to_string(),
            "a.example answered 403 forbidden: Not permitted to post"
        );
    }
}
