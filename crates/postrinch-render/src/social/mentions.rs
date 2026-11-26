//! Mentions in a message's text, found as OFSCP §10.1 finds them: `@handle`
//! or `@handle@domain`, matched case-insensitively and lowercased, where the
//! `@` is not preceded by a letter, a digit, `_` or `@` (so `bob@example.com`
//! in prose is not one), `handle` is the longest following run of handle
//! characters (`[a-z0-9_-]`), and a bare `@handle` names a user of the
//! provider hosting the channel.
//!
//! When a message carries `mentions` (§5.3, in spec PR lost-conn/ofscp#24,
//! <https://github.com/lost-conn/ofscp/pull/24>), that list is the message's
//! mentions, and only the `@…` references that name someone on it are
//! marked; otherwise every reference is.
//!
//! Two things the spec leaves to the reader, decided here for showing only
//! (what a provider notifies does not depend on them):
//!
//! - A run of handle characters that cannot be a handle (§2.2.1: 3 to 32 of
//!   them) names no one, so it is not marked.
//! - §10.1 does not spell out `@domain`. It is read as §2.2 writes a domain:
//!   DNS labels of letters, digits and `-`, an optional `:port`, a sentence's
//!   full stop after it left out. A domain run straight into other letters
//!   (`@bob@münchen.de`, whose ASCII form is `xn--mnchen-3ya.de`) is not a
//!   mention at all, rather than a bare `@bob` of the wrong provider.

use std::ops::Range;

/// One `@…` reference in a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Reference {
    /// Where it is in the text, `@` included.
    pub range: Range<usize>,
    /// Whom it names: `handle@domain`, lowercased.
    pub actor: String,
}

/// The longest handle §2.2.1 allows, and the shortest.
const HANDLE_LENGTHS: std::ops::RangeInclusive<usize> = 3..=32;

/// Every `@…` reference in `text`, in order. A bare `@handle` names a user
/// of `domain` (the provider hosting the channel).
pub(super) fn references(text: &str, domain: &str) -> Vec<Reference> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut previous: Option<char> = None;
    let mut skip_to = 0;
    for (at, c) in text.char_indices() {
        let before = previous.replace(c);
        if at < skip_to || c != '@' {
            continue;
        }
        if before.is_some_and(|b| b.is_alphanumeric() || b == '_' || b == '@') {
            continue;
        }
        let handle_end = at + 1 + run(&bytes[at + 1..], is_handle_byte);
        let handle = &text[at + 1..handle_end];
        if !HANDLE_LENGTHS.contains(&handle.len()) {
            continue;
        }
        let (end, host) = match bytes.get(handle_end) {
            Some(b'@') => match domain_at(text, handle_end + 1) {
                Domain::Found(end) => (end, &text[handle_end + 1..end]),
                Domain::Absent => (handle_end, domain),
                Domain::Unreadable => continue,
            },
            _ if followed_by_word(text, handle_end) => continue,
            _ => (handle_end, domain),
        };
        found.push(Reference {
            range: at..end,
            actor: format!("{handle}@{host}").to_ascii_lowercase(),
        });
        skip_to = end;
    }
    found
}

fn is_handle_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// How many bytes at the start of `bytes` satisfy `keep`.
fn run(bytes: &[u8], keep: impl Fn(u8) -> bool) -> usize {
    bytes.iter().take_while(|&&b| keep(b)).count()
}

/// Whether the text goes on in letters at `at`: a handle run that stops at
/// a letter outside ASCII (`@renée`) is not a whole handle.
fn followed_by_word(text: &str, at: usize) -> bool {
    text[at..].chars().next().is_some_and(char::is_alphanumeric)
}

enum Domain {
    /// A domain ending here.
    Found(usize),
    /// No domain at all after the second `@` (`@bob@ ` or the text's end):
    /// the reference is a bare `@handle`.
    Absent,
    /// Something that starts like a domain but is not one.
    Unreadable,
}

/// The domain starting at `start`, as §2.2 spells one.
fn domain_at(text: &str, start: usize) -> Domain {
    let bytes = text.as_bytes();
    let mut end = start
        + run(&bytes[start..], |b| {
            b.is_ascii_alphanumeric() || b == b'.' || b == b'-'
        });
    // A full stop (or several) closing the sentence is not part of it.
    while end > start && bytes[end - 1] == b'.' {
        end -= 1;
    }
    if end == start {
        return if followed_by_word(text, start) {
            Domain::Unreadable
        } else {
            Domain::Absent
        };
    }
    let labels_ok = text[start..end].split('.').all(|label| {
        (1..=63).contains(&label.len()) && !label.starts_with('-') && !label.ends_with('-')
    });
    if !labels_ok {
        return Domain::Unreadable;
    }
    if bytes.get(end) == Some(&b':') {
        let digits = run(&bytes[end + 1..], |b| b.is_ascii_digit());
        if (1..=5).contains(&digits) {
            end += 1 + digits;
        }
    }
    if followed_by_word(text, end) || bytes.get(end).is_some_and(|&b| is_handle_byte(b)) {
        return Domain::Unreadable;
    }
    Domain::Found(end)
}

#[cfg(test)]
mod tests {
    use super::references;

    fn actors(text: &str) -> Vec<String> {
        references(text, "a.example")
            .into_iter()
            .map(|r| r.actor)
            .collect()
    }

    #[test]
    fn bare_and_qualified_handles() {
        assert_eq!(
            actors("hi @Bob and @carol@B.Example, thanks"),
            ["bob@a.example", "carol@b.example"]
        );
        assert_eq!(
            actors("ping @dave@localhost:8080."),
            ["dave@localhost:8080"]
        );
    }

    #[test]
    fn an_address_in_prose_is_not_a_mention() {
        assert!(actors("write to bob@example.com or x_@bob").is_empty());
        assert!(actors("@@bob").is_empty());
    }

    #[test]
    fn a_run_that_cannot_be_a_handle_names_no_one() {
        assert!(actors("@ab @").is_empty());
        assert!(actors(&format!("@{}", "a".repeat(33))).is_empty());
        assert_eq!(actors(&format!("@{}", "a".repeat(32))).len(), 1);
    }

    #[test]
    fn the_sentence_goes_on_around_a_mention() {
        let found = references("(@bob@b.example). @eve!", "a.example");
        assert_eq!(found[0].range, 1..15);
        assert_eq!(found[1].actor, "eve@a.example");
        // A second @ with nothing after it leaves a bare handle.
        assert_eq!(actors("@bob@ is here"), ["bob@a.example"]);
    }

    #[test]
    fn letters_past_ascii_are_not_read_as_a_shorter_name() {
        assert!(actors("@renée and @bob@münchen.de").is_empty());
        assert!(actors("@bob@-bad.example @bob@b..example").is_empty());
    }
}
