//! A refusal, answered over REST as RFC 7807 problem details (§2.6, §7.3)
//! or over the WebSocket as an `error` event (§7.3).

use ofscp_core::wire::ErrorCode;

/// Why a request or command was refused. Boxed, so the `Err` side of an
/// [`Outcome`] stays one pointer wide; its fields read through `Deref`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Problem(Box<Fields>);

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Fields {
    pub(crate) code: ErrorCode,
    pub(crate) detail: String,
    /// `Retry-After`, in seconds (§4.5, §7.3).
    pub(crate) retry_after: Option<u64>,
    /// Extra structured detail, such as the refused channels of a
    /// `subscribe` (§7.1).
    pub(crate) details: Option<serde_json::Value>,
}

impl Problem {
    pub(crate) fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self(Box::new(Fields {
            code,
            detail: detail.into(),
            retry_after: None,
            details: None,
        }))
    }

    /// The fields, by value.
    pub(crate) fn into_fields(self) -> Fields {
        *self.0
    }

    pub(crate) fn internal(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::InternalError, detail)
    }

    pub(crate) fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, detail)
    }

    pub(crate) fn unauthorized(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unauthorized, detail)
    }

    pub(crate) fn forbidden(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::Forbidden, detail)
    }

    pub(crate) fn not_found(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, detail)
    }

    pub(crate) fn conflict(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, detail)
    }

    pub(crate) fn unavailable(detail: impl Into<String>, retry_after: Option<u64>) -> Self {
        let mut p = Self::new(ErrorCode::ServiceUnavailable, detail);
        p.0.retry_after = retry_after;
        p
    }

    pub(crate) fn with_details(mut self, details: serde_json::Value) -> Self {
        self.0.details = Some(details);
        self
    }

    /// The HTTP status the registry pairs with the code.
    pub(crate) fn status(&self) -> u16 {
        self.code.status().unwrap_or(500)
    }
}

impl std::ops::Deref for Problem {
    type Target = Fields;

    fn deref(&self) -> &Fields {
        &self.0
    }
}

/// What a handler returns: a value, or a refusal.
pub(crate) type Outcome<T> = Result<T, Problem>;
