//! What came of sending one change, and what a failure means for it.

use std::time::Duration;

use postrinch_ofscp::Error;

use crate::{SocialError, SocialResult};

/// What came of one queued change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Its provider has it: it leaves the outbox.
    Done,
    /// It failed for now (the provider could not be reached, or failed):
    /// tried again later, no sooner than `retry_after` when the provider
    /// gave one, and the changes after it in its place wait.
    Again {
        error: String,
        retry_after: Option<Duration>,
    },
    /// Its provider refused it for good (it is not permitted, or names
    /// what is no longer there): kept for the person to give up, and
    /// holding back nothing, since sending it again would be refused
    /// again.
    Refused(String),
    /// It cannot go yet, and is not tried: it carries files still to be
    /// uploaded. The changes after it in its place wait for it.
    Waiting,
    /// A file it carries could not be uploaded for now: tried again
    /// later, as [`Self::Again`] is, but it was not sent, so it is not
    /// counted as an attempt and can still be taken back.
    NotUploaded {
        error: String,
        retry_after: Option<Duration>,
    },
    /// Not a change this engine sends yet (notices settings, the profile):
    /// it stays queued and untried, holding back nothing.
    NotSentHere,
}

impl Outcome {
    /// Whether the changes after this one in its place must wait.
    pub(crate) fn holds_back(&self) -> bool {
        matches!(
            self,
            Self::Again { .. } | Self::NotUploaded { .. } | Self::Waiting
        )
    }
}

/// What `error`, the failure of a request carrying a change, means for
/// it; an error that is the engine's or the account's (the account can no
/// longer sign, the store failed) ends the pass instead.
pub(crate) fn outcome_of(error: SocialError) -> SocialResult<Outcome> {
    match error {
        SocialError::Transport(error) if is_refusal(&error) => {
            Ok(Outcome::Refused(error.to_string()))
        }
        SocialError::Transport(error) => Ok(Outcome::Again {
            retry_after: error.problem().and_then(|p| p.retry_after),
            error: error.to_string(),
        }),
        SocialError::Socket(error) => Ok(Outcome::Again {
            error: error.to_string(),
            retry_after: None,
        }),
        other => Err(other),
    }
}

/// Whether `error` is the provider refusing the request for what it asks:
/// a `4xx` answer, but for `401` (a signature another provider could not
/// check yet: home is the key's judge, and a `401` there marks the
/// account before this is asked), `408` and `429`, which pass. A request
/// that could not even be made (a body that does not encode, a file to
/// upload that cannot be read) would fail the same way every time.
pub(crate) fn is_refusal(error: &Error) -> bool {
    match error {
        Error::Problem(problem) => {
            (400..500).contains(&problem.status) && ![401, 408, 429].contains(&problem.status)
        }
        Error::Encode(_)
        | Error::Sign(_)
        | Error::Endpoint(_)
        | Error::InvalidUpload(_)
        | Error::Read { .. } => true,
        _ => false,
    }
}

/// Whether `error` is a `404`: what the change is about is not there.
pub(crate) fn is_not_found(error: &SocialError) -> bool {
    matches!(error, SocialError::Transport(Error::Problem(p)) if p.status == 404)
}

#[cfg(test)]
mod tests {
    use super::*;
    use postrinch_ofscp::Problem;

    fn problem(status: u16, retry_after: Option<Duration>) -> SocialError {
        SocialError::Transport(Error::Problem(Box::new(Problem {
            authority: "groups.example".into(),
            status,
            error_code: None,
            title: None,
            detail: None,
            retry_after,
        })))
    }

    #[test]
    fn a_client_error_is_a_refusal_but_for_those_that_pass() {
        for status in [400, 403, 404, 409, 413, 422] {
            assert!(matches!(
                outcome_of(problem(status, None)),
                Ok(Outcome::Refused(_))
            ));
        }
        for status in [401, 408, 429, 500, 503] {
            assert!(matches!(
                outcome_of(problem(status, None)),
                Ok(Outcome::Again { .. })
            ));
        }
    }

    #[test]
    fn a_failure_keeps_the_wait_its_provider_asks_for() {
        let wait = Some(Duration::from_secs(7));
        let Ok(Outcome::Again { retry_after, .. }) = outcome_of(problem(503, wait)) else {
            panic!("a 503 is tried again");
        };
        assert_eq!(retry_after, wait);
    }

    #[test]
    fn the_accounts_own_trouble_ends_the_pass() {
        let unusable = SocialError::NoAccount("ada@groups.example".into());
        assert!(outcome_of(unusable).is_err());
        assert!(is_not_found(&problem(404, None)));
        assert!(!is_not_found(&problem(410, None)));
    }
}
