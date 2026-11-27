//! Request paths and paging parameters.
//!
//! A signature covers the path and query exactly as sent (§4.4.2), so they
//! are built here once, percent-encoded, and the signer reads them back
//! from the URL that is sent.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

/// Everything but RFC 3986's unreserved characters is encoded in a path
/// segment: an id, a handle (`alice@b.example`) or an object URI.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// `template` with each `{}` replaced by the next of `segments`,
/// percent-encoded.
pub fn path(template: &str, segments: &[&str]) -> String {
    let mut out = String::with_capacity(template.len() + 16 * segments.len());
    let mut parts = template.split("{}");
    out.push_str(parts.next().unwrap_or_default());
    let mut segments = segments.iter();
    for part in parts {
        let segment = segments.next().expect("a segment for every {} in the path");
        out.extend(utf8_percent_encode(segment, SEGMENT));
        out.push_str(part);
    }
    debug_assert!(
        segments.next().is_none(),
        "more segments than {{}} in {template}"
    );
    out
}

/// Which way to page (§7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Newest first (the default).
    Backward,
    /// Oldest first.
    Forward,
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Backward => "backward",
            Self::Forward => "forward",
        }
    }
}

/// The paging parameters of a paged endpoint (§7.2). Left empty, the
/// provider's defaults apply: backward, 50 items.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageRequest {
    /// Continue from this cursor (a `nextCursor` or `prevCursor` the same
    /// endpoint gave).
    pub cursor: Option<String>,
    /// Which way.
    pub direction: Option<Direction>,
    /// How many, at most (the provider clamps it to 100).
    pub limit: Option<u32>,
}

impl PageRequest {
    /// The newest page.
    pub fn newest() -> Self {
        Self::default()
    }

    /// The page after `cursor`, going `direction`.
    pub fn after(cursor: impl Into<String>, direction: Direction) -> Self {
        Self {
            cursor: Some(cursor.into()),
            direction: Some(direction),
            limit: None,
        }
    }

    /// With at most `limit` items.
    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// As query pairs.
    pub(crate) fn query(&self) -> Query {
        let mut q = Query::new();
        q.opt("cursor", self.cursor.as_deref());
        q.opt("direction", self.direction.map(Direction::as_str));
        q.opt("limit", self.limit.map(|l| l.to_string()).as_deref());
        q
    }
}

/// A query string, as `name=value` pairs in the order given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query(Vec<(&'static str, String)>);

impl Query {
    /// No parameters.
    pub fn new() -> Self {
        Self::default()
    }

    /// With `name=value`.
    pub fn with(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.0.push((name, value.into()));
        self
    }

    /// Add `name=value` when there is a value.
    pub fn opt(&mut self, name: &'static str, value: Option<&str>) {
        if let Some(value) = value {
            self.0.push((name, value.to_owned()));
        }
    }

    /// Add all of `other`.
    pub fn extend(mut self, other: Query) -> Self {
        self.0.extend(other.0);
        self
    }

    pub(crate) fn pairs(&self) -> &[(&'static str, String)] {
        &self.0
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_are_encoded() {
        assert_eq!(
            path("/api/users/{}/profile", &["alice@b.example"]),
            "/api/users/alice%40b.example/profile"
        );
        assert_eq!(
            path(
                "/api/me/follows/{}",
                &["ofscp://b.example/groups/g/channels/c"]
            ),
            "/api/me/follows/ofscp%3A%2F%2Fb.example%2Fgroups%2Fg%2Fchannels%2Fc"
        );
        assert_eq!(
            path(
                "/api/groups/{}/channels/{}/messages",
                &["grp_1", "chn-2.x~"]
            ),
            "/api/groups/grp_1/channels/chn-2.x~/messages"
        );
        assert_eq!(path("/api/me", &[]), "/api/me");
    }

    #[test]
    fn paging_becomes_a_query() {
        let q = PageRequest::after("abc", Direction::Forward)
            .limit(20)
            .query();
        assert_eq!(
            q.pairs(),
            &[
                ("cursor", "abc".to_owned()),
                ("direction", "forward".into()),
                ("limit", "20".into())
            ]
        );
        assert!(PageRequest::newest().query().is_empty());
    }
}
