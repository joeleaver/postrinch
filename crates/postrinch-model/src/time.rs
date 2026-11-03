use serde::{Deserialize, Serialize};

/// A moment in time, as milliseconds since the Unix epoch, UTC.
///
/// The store keeps `DateTime` fields in exactly this form, and providers
/// report times in it (Gmail's `internalDate`) or convert to it (a `Date:`
/// header), so the model carries no calendar arithmetic of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Timestamp(i64);

impl Timestamp {
    pub const fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    pub const fn as_millis(self) -> i64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_by_instant() {
        let earlier = Timestamp::from_millis(1_000);
        let later = Timestamp::from_millis(2_000);
        assert!(earlier < later);
        assert_eq!(later.as_millis(), 2_000);
    }
}
