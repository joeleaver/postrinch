//! What goes wrong keeping a secret.

/// A result whose error is the keychain's.
pub type KeychainResult<T> = Result<T, KeychainError>;

/// The keychain could not read, keep or remove a secret. The text says
/// why in terms a person can act on, and is shown to them as it is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct KeychainError(String);

impl KeychainError {
    /// An error saying `why`.
    pub fn new(why: impl Into<String>) -> Self {
        Self(why.into())
    }

    /// What went wrong.
    pub fn message(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_its_text_as_it_is() {
        let error = KeychainError::new("the keyring is locked");
        assert_eq!(error.to_string(), "the keyring is locked");
        assert_eq!(error.message(), "the keyring is locked");
    }
}
