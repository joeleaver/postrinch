//! Errors: RFC 7807 problem details and the `errorCode` registry (spec
//! §2.6, §7.3).

use serde::{Deserialize, Serialize};

use super::open_enum;

open_enum! {
    /// The interoperable key of an error (§7.3). Decide how to handle an
    /// error from this and the HTTP status; when it is absent or unknown,
    /// fall back to the status alone (§2.6).
    pub enum ErrorCode {
        /// 400: invalid payload, parameters, or schema mismatch.
        BadRequest = "bad_request",
        /// 401: invalid or missing signature or credentials.
        Unauthorized = "unauthorized",
        /// 403: not permitted, or refused by federation policy.
        Forbidden = "forbidden",
        /// 403: the provider takes no self-registration.
        RegistrationClosed = "registration_closed",
        /// 403: registration needs a usable account invite.
        InviteRequired = "invite_required",
        /// 404: no such resource, or not visible to the caller.
        NotFound = "not_found",
        /// 409: conflicts with the current state.
        Conflict = "conflict",
        /// 409: too many live subscriptions on one socket.
        SubscriptionLimit = "subscription_limit",
        /// 413: over the provider's size limit.
        PayloadTooLarge = "payload_too_large",
        /// 429: rate limited; honour `Retry-After`.
        TooManyRequests = "too_many_requests",
        /// 500: an unexpected provider error.
        InternalError = "internal_error",
        /// 503: temporarily unavailable; honour `Retry-After`.
        ServiceUnavailable = "service_unavailable",
    }
}

/// An `application/problem+json` error body (§2.6). Do not depend on
/// `type`: each provider mints its own (`https://{authority}/problems/...`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProblemDetails {
    /// The provider's URI for the problem.
    #[serde(rename = "type")]
    pub kind: String,
    /// A short human-readable summary.
    pub title: String,
    /// The HTTP status.
    pub status: u16,
    /// The interoperable code. Required by the spec, but a proxy's or an
    /// older provider's answer may lack it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<ErrorCode>,
    /// What went wrong, for people.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The request path it is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}
