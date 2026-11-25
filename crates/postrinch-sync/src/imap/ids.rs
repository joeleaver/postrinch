//! The ids Postrinch gives an IMAP account's messages and folders
//! (docs/imap.md, "A message's identity" and "One model, two providers").
//!
//! IMAP names a copy of a message by its folder, UIDVALIDITY and UID, which
//! change on every move. A message's provider id must not, so it comes from
//! the `Message-ID` header instead.

use postrinch_model::{EmailAddress, ProviderMessageId};
use sha2::{Digest, Sha256};

/// The separator between the parts of what is hashed: the ASCII unit
/// separator, which appears in no address, header or label id.
const SEP: u8 = 0x1f;

/// How many hex digits of a hash an id keeps: 96 bits.
const HASH_DIGITS: usize = 24;

/// The first [`HASH_DIGITS`] hex digits of the SHA-256 of `parts`, joined
/// by [`SEP`].
pub(super) fn short_hash(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            hasher.update([SEP]);
        }
        hasher.update(part);
    }
    hex(&hasher.finalize()[..HASH_DIGITS / 2])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A `Message-ID` (or a `References` or `In-Reply-To` entry) as it is
/// hashed: without surrounding whitespace and angle brackets, as
/// [`postrinch_model::MessageHeaders`] keeps it, so a header read raw and
/// one read parsed give the same id. Its case is kept: a `Message-ID`'s left
/// part is case-sensitive.
pub(super) fn bare(message_id: &str) -> &str {
    let id = message_id.trim();
    let id = id.strip_prefix('<').unwrap_or(id);
    let id = id.strip_suffix('>').unwrap_or(id);
    id.trim()
}

/// The provider id of the message whose `Message-ID` header is
/// `header_message_id`, in `account`: `m` and 24 hex digits of the SHA-256
/// of the account's key and the header. Every copy of the message, in any
/// folder, has this id, and moving one does not change it.
///
/// The header must not be blank; a message without one is named by
/// [`message_id_without_header`].
pub fn message_id(account: &EmailAddress, header_message_id: &str) -> ProviderMessageId {
    let hash = short_hash(&[account.key().as_bytes(), bare(header_message_id).as_bytes()]);
    ProviderMessageId(format!("m{hash}"))
}

/// Whether a message's id comes from its `Message-ID` header
/// ([`message_id`]), so a copy of it can be found by that header; one
/// named by its place ([`message_id_without_header`]) cannot.
pub fn named_by_message_id(id: &ProviderMessageId) -> bool {
    id.0.starts_with('m')
}

/// The provider id of a message without a `Message-ID` header (rare: some
/// automated mail), from the one copy it was found as: `u` and a hash of
/// the folder's label id, its UIDVALIDITY and the UID. Moving such a
/// message looks like deleting it and finding a new one; nothing is lost.
pub fn message_id_without_header(
    uidvalidity: u32,
    folder_label_id: &str,
    uid: u32,
) -> ProviderMessageId {
    let hash = short_hash(&[
        folder_label_id.as_bytes(),
        uidvalidity.to_string().as_bytes(),
        uid.to_string().as_bytes(),
    ]);
    ProviderMessageId(format!("u{hash}"))
}

/// A new label id for a user folder Postrinch sees for the first time: `f`
/// and 12 random hex digits. It is kept with the folder's state for the
/// folder's life, so renaming the folder from Postrinch keeps its messages,
/// prefs and the rules naming it attached.
pub fn mint_folder_label_id() -> String {
    let mut bytes = [0u8; 6];
    rand::fill(&mut bytes[..]);
    format!("f{}", hex(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(s: &str) -> EmailAddress {
        EmailAddress::parse(s).unwrap()
    }

    #[test]
    fn a_message_id_is_its_header_hashed_in_its_account() {
        let ada = account("ada@example.org");
        let id = message_id(&ada, "<abc.123@mail.example.org>");
        assert!(id.0.starts_with('m'));
        assert_eq!(id.0.len(), 1 + HASH_DIGITS);
        assert!(id.0[1..].chars().all(|c| c.is_ascii_hexdigit()));
        // Raw or parsed, and the account's address in any case: one id.
        assert_eq!(message_id(&ada, " abc.123@mail.example.org "), id);
        assert_eq!(
            message_id(&account("Ada@Example.org"), "abc.123@mail.example.org"),
            id
        );
        // Another account's copy is another message.
        assert_ne!(
            message_id(&account("bob@example.org"), "abc.123@mail.example.org"),
            id
        );
        // A Message-ID's left part is case-sensitive.
        assert_ne!(message_id(&ada, "ABC.123@mail.example.org"), id);
    }

    #[test]
    fn a_message_id_is_stable() {
        // Changing this breaks every stored IMAP message's id.
        assert_eq!(
            message_id(&account("ada@example.org"), "abc@example.org").0,
            format!("m{}", short_hash(&[b"ada@example.org", b"abc@example.org"]))
        );
        assert_eq!(
            short_hash(&[b"a", b"b"]),
            hex(&Sha256::digest(b"a\x1fb")[..12])
        );
    }

    #[test]
    fn without_a_header_the_copy_names_it() {
        let id = message_id_without_header(7, "INBOX", 42);
        assert!(id.0.starts_with('u'));
        assert_eq!(id.0.len(), 1 + HASH_DIGITS);
        assert_eq!(message_id_without_header(7, "INBOX", 42), id);
        assert_ne!(message_id_without_header(8, "INBOX", 42), id);
        assert_ne!(message_id_without_header(7, "SENT", 42), id);
        assert_ne!(message_id_without_header(7, "INBOX", 43), id);
        // The parts cannot run into each other.
        assert_ne!(
            message_id_without_header(1, "f1", 23),
            message_id_without_header(12, "f1", 3)
        );
    }

    #[test]
    fn minted_folder_ids_are_f_and_twelve_hex_digits() {
        let id = mint_folder_label_id();
        assert_eq!(id.len(), 13);
        assert!(id.starts_with('f'));
        assert!(id[1..].chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(mint_folder_label_id(), id);
    }
}
