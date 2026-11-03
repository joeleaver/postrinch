//! Store keys for objects a provider identifies within one account.

use postrinch_model::EmailAddress;

/// The separator between an account and a provider id in a key: the ASCII
/// unit separator, which appears in neither an address nor a provider id.
const SEP: char = '\u{1f}';

/// The store key for `provider_id` in `account`.
pub(crate) fn scoped(account: &EmailAddress, provider_id: &str) -> String {
    format!("{}{SEP}{provider_id}", account.key())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_differ_by_account_and_ignore_address_case() {
        let a = EmailAddress::parse("Ada@example.org").unwrap();
        let b = EmailAddress::parse("bob@example.org").unwrap();
        assert_eq!(scoped(&a, "m1"), "ada@example.org\u{1f}m1");
        assert_ne!(scoped(&a, "m1"), scoped(&b, "m1"));
    }
}
