//! Folder names in IMAP's modified UTF-7 (RFC 3501 5.1.3).
//!
//! Written here rather than taken from the `utf7-imap` crate, which
//! panics on a name that is not valid modified UTF-7: a server's folder
//! name must never be able to stop sync.

use base64::Engine;
use base64::alphabet::IMAP_MUTF7;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

/// Modified BASE64: `,` in place of `/`, and no padding.
fn engine() -> GeneralPurpose {
    GeneralPurpose::new(
        &IMAP_MUTF7,
        GeneralPurposeConfig::new()
            .with_encode_padding(false)
            .with_decode_padding_mode(DecodePaddingMode::RequireNone)
            .with_decode_allow_trailing_bits(true),
    )
}

/// A folder name as a person reads it. A name that is not valid modified
/// UTF-7 is shown as the server spells it.
pub(super) fn decode(name: &str) -> String {
    try_decode(name).unwrap_or_else(|| name.to_owned())
}

/// A folder name as the server spells it, for a name the person typed:
/// printable ASCII as it is (`&` as `&-`), every run of other characters
/// as `&`, its UTF-16 in modified BASE64, and `-`.
pub(super) fn encode(name: &str) -> String {
    let engine = engine();
    let mut out = String::with_capacity(name.len());
    let mut run: Vec<u16> = Vec::new();
    let flush = |run: &mut Vec<u16>, out: &mut String| {
        if run.is_empty() {
            return;
        }
        let bytes: Vec<u8> = run.iter().flat_map(|unit| unit.to_be_bytes()).collect();
        out.push('&');
        out.push_str(&engine.encode(bytes));
        out.push('-');
        run.clear();
    };
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut run, &mut out);
            out.push(c);
            if c == '&' {
                out.push('-');
            }
        } else {
            let mut units = [0u16; 2];
            run.extend_from_slice(c.encode_utf16(&mut units));
        }
    }
    flush(&mut run, &mut out);
    out
}

fn try_decode(name: &str) -> Option<String> {
    let engine = engine();
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after.find('-')?;
        let encoded = &after[..end];
        if encoded.is_empty() {
            out.push('&');
        } else {
            let bytes = engine.decode(encoded).ok()?;
            if bytes.len() % 2 != 0 {
                return None;
            }
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_be_bytes(*pair))
                .collect();
            out.push_str(&String::from_utf16(&units).ok()?);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn encodes_rfc_examples_and_others() {
        assert_eq!(
            encode("~peter/mail/台北/日本語"),
            "~peter/mail/&U,BTFw-/&ZeVnLIqe-"
        );
        assert_eq!(encode("Отправленные"), "&BB4EQgQ,BEAEMAQyBDsENQQ9BD0ESwQ1-");
        assert_eq!(encode("Tom & Jerry"), "Tom &- Jerry");
        assert_eq!(encode("Entwürfe"), "Entw&APw-rfe");
        assert_eq!(encode("Plain"), "Plain");
        for name in ["Fächer/Ärger", "😀 Fun", "a&b&", "Tab\there"] {
            assert_eq!(decode(&encode(name)), name, "{name}");
        }
    }

    #[test]
    fn decodes_rfc_examples_and_others() {
        assert_eq!(decode("INBOX"), "INBOX");
        assert_eq!(
            decode("~peter/mail/&U,BTFw-/&ZeVnLIqe-"),
            "~peter/mail/台北/日本語"
        );
        assert_eq!(decode("&BB4EQgQ,BEAEMAQyBDsENQQ9BD0ESwQ1-"), "Отправленные");
        assert_eq!(decode("Tom &- Jerry"), "Tom & Jerry");
        assert_eq!(decode("Entw&APw-rfe"), "Entwürfe");
    }

    #[test]
    fn invalid_names_are_shown_as_spelled() {
        assert_eq!(decode("Broken &AB"), "Broken &AB");
        assert_eq!(decode("Bad &!!!-"), "Bad &!!!-");
        // One byte of UTF-16 is not a character.
        assert_eq!(decode("Odd &AA-"), "Odd &AA-");
    }
}
